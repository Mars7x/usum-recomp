//! Rust emission. Each guest function becomes
//!
//! ```text
//! pub fn f_XXXXXXXX(cpu: &mut Cpu, mem: &mut Memory) -> Result<Exit, Fault> {   // t_ for Thumb
//!     let mut pc = cpu.r[15];
//!     loop { match pc { <one arm per block> _ => { cpu.r[15] = pc; return Ok(Exit::Jump) } } }
//! }
//! ```
//!
//! so it can be entered at any of its labels. Control flow inside the
//! function sets `pc` and loops. Anything else (returns, indirect jumps,
//! tail calls) leaves with `Exit::Jump`, and the caller or host continues
//! at `cpu.r[15]`. BL becomes a Rust call, depth-limited. Generated code
//! uses only `usum_guest`'s public interface and contains no `unsafe`.

use std::fmt::Write;

use usum_arm::{DataOp, DualMul, HalfMul, Instr, Offset, Op, Operand, RevKind, Size, VfpOp, AL, LR, PC};

use crate::discover::{blocks, key, Analysis, Function};

/// Emitted lines are collected here; `writeln!` into a String cannot fail.
struct Out(String);

macro_rules! line {
    ($out:expr, $($arg:tt)*) => { writeln!($out.0, $($arg)*).expect("writing to a String") };
}

thread_local! {
    /// Set while emitting module code (ADR-0007): code addresses that the
    /// guest can see are then `delta + virtual address`, where `delta`
    /// (`cpu.module_delta`, read at function entry) is the module's load
    /// address minus its virtual base. The function's own `pc` stays
    /// virtual, so labels and branches are constants as for the executable.
    static RELOCATABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn relocatable() -> bool {
    RELOCATABLE.with(|r| r.get())
}

/// A code address as the guest sees it.
fn a(v: u32) -> String {
    if relocatable() {
        format!("delta.wrapping_add({v:#010x})")
    } else {
        hex(v)
    }
}

/// A guest-visible code address (an expression) as the function's `pc`.
fn to_pc(expression: &str) -> String {
    if relocatable() {
        format!("({expression}).wrapping_sub(delta)")
    } else {
        expression.to_owned()
    }
}

/// The function's `pc` as the guest sees it.
fn from_pc() -> &'static str {
    if relocatable() {
        "pc.wrapping_add(delta)"
    } else {
        "pc"
    }
}

/// A register read; the PC reads as the constant the decoder computed
/// (`Instr::pc`), passed here as `pc`.
fn r(n: u8, pc: u32) -> String {
    if n == PC {
        if relocatable() {
            format!("delta.wrapping_add({pc:#010x}u32)")
        } else {
            format!("{pc:#010x}u32")
        }
    } else {
        format!("cpu.r[{n}]")
    }
}

fn hex(v: u32) -> String {
    format!("{v:#010x}")
}

/// Code after an interworking jump to `pc`: continue in this function if
/// the state is still this function's, else leave for the host.
fn bx_tail(thumb: bool) -> String {
    let state = if thumb { "!cpu.thumb" } else { "cpu.thumb" };
    format!("if {state} {{ cpu.r[15] = {}; return Ok(Exit::Jump); }} continue;", from_pc())
}

/// The Rust name of the function with this key.
pub fn name(key: u32) -> String {
    if key & 1 == 1 {
        format!("t_{:08x}", key & !1)
    } else {
        format!("f_{key:08x}")
    }
}

fn operand(out: &mut Out, pc: u32, operand: Operand, need_carry: bool) {
    match operand {
        Operand::Imm { value, carry } => {
            line!(out, "let b = {}u32;", hex(value));
            if need_carry {
                match carry {
                    Some(c) => line!(out, "let sc = {c};"),
                    None => line!(out, "let sc = cpu.c;"),
                }
            }
        }
        Operand::ImmShift { rm, kind: 0, amount: 0 } => {
            line!(out, "let b = {};", r(rm, pc));
            if need_carry {
                line!(out, "let sc = cpu.c;");
            }
        }
        Operand::ImmShift { rm, kind, amount } => {
            line!(out, "let (b, sc) = shift_imm({}, {kind}, {amount}, cpu.c);", r(rm, pc));
        }
        Operand::RegShift { rm, kind, rs } => {
            line!(out, "let (b, sc) = shift_reg(cpu.r[{rm}], {kind}, cpu.r[{rs}], cpu.c);");
        }
    }
}

/// One expression for the common forms that need no flags or carry: no S
/// bit, an immediate or unshifted register operand.
fn simple(op: DataOp, pc: u32, rn: u8, operand: Operand) -> Option<String> {
    let b = match operand {
        Operand::Imm { value, .. } => format!("{}u32", hex(value)),
        Operand::ImmShift { rm, kind: 0, amount: 0 } => r(rm, pc),
        _ => return None,
    };
    let a = r(rn, pc);
    Some(match op {
        DataOp::And => format!("{a} & {b}"),
        DataOp::Eor => format!("{a} ^ {b}"),
        DataOp::Orr => format!("{a} | {b}"),
        DataOp::Bic => format!("{a} & !{b}"),
        DataOp::Mov => b,
        DataOp::Mvn => format!("!{b}"),
        DataOp::Add => format!("{a}.wrapping_add({b})"),
        DataOp::Sub => format!("{a}.wrapping_sub({b})"),
        DataOp::Rsb => format!("{b}.wrapping_sub({a})"),
        _ => return None, // carry-using and flag-only operations
    })
}

#[allow(clippy::too_many_arguments)]
fn data(out: &mut Out, pc: u32, thumb: bool, op: DataOp, s: bool, rd: u8, rn: u8, operand_: Operand) {
    if !s && rd != PC {
        if let Some(expression) = simple(op, pc, rn, operand_) {
            line!(out, "cpu.r[{rd}] = {expression};");
            return;
        }
    }
    let logical_flags = s && op.is_logical();
    operand(out, pc, operand_, logical_flags);
    if op.uses_rn() {
        line!(out, "let a = {};", r(rn, pc));
    }
    let arith = |x: &str, y: &str, c: &str| format!("add_with_carry({x}, {y}, {c})");
    let value = match op {
        DataOp::And | DataOp::Tst => "a & b".to_owned(),
        DataOp::Eor | DataOp::Teq => "a ^ b".to_owned(),
        DataOp::Orr => "a | b".to_owned(),
        DataOp::Bic => "a & !b".to_owned(),
        DataOp::Mov => "b".to_owned(),
        DataOp::Mvn => "!b".to_owned(),
        DataOp::Add | DataOp::Cmn => arith("a", "b", "false"),
        DataOp::Adc => arith("a", "b", "cpu.c"),
        DataOp::Sub | DataOp::Cmp => arith("a", "!b", "true"),
        DataOp::Sbc => arith("a", "!b", "cpu.c"),
        DataOp::Rsb => arith("b", "!a", "true"),
        DataOp::Rsc => arith("b", "!a", "cpu.c"),
    };
    let result = if op.is_logical() {
        line!(out, "let v = {value};");
        if s {
            line!(out, "nz(cpu, v); cpu.c = sc;");
        }
        "v"
    } else if s {
        line!(out, "let v = nzcv(cpu, {value});");
        "v"
    } else {
        line!(out, "let v = {value}.0;");
        "v"
    };
    if op.is_test() {
        return;
    }
    if rd == PC {
        // ARMv6 BranchWritePC: no interworking for data-processing writes
        line!(
            out,
            "cpu.from = {}; pc = {}; continue;",
            a(pc.wrapping_sub(if thumb { 4 } else { 8 })),
            to_pc(&format!("{result} & {}", if thumb { "!1" } else { "!3" }))
        );
    } else {
        line!(out, "cpu.r[{rd}] = {result};");
    }
}

fn mem_access(out: &mut Out, pc: u32, thumb: bool, instr: &Op) {
    let Op::Mem { load, size, signed, rt, rn, offset, add, pre, writeback } = *instr else { unreachable!() };
    line!(out, "let base = {};", r(rn, pc));
    match offset {
        Offset::Imm(v) => line!(out, "let off = {}u32;", hex(v)),
        Offset::Reg { rm, kind: 0, amount: 0 } => line!(out, "let off = cpu.r[{rm}];"),
        Offset::Reg { rm, kind, amount } => line!(out, "let off = shift_imm(cpu.r[{rm}], {kind}, {amount}, cpu.c).0;"),
    }
    line!(out, "let moved = base.{}(off);", if add { "wrapping_add" } else { "wrapping_sub" });
    line!(out, "let address = {};", if pre { "moved" } else { "base" });
    let wb = |out: &mut Out| {
        if writeback {
            line!(out, "cpu.r[{rn}] = moved;");
        }
    };
    if load {
        match (size, signed) {
            (Size::Word, _) => line!(out, "let v = mem.read_u32(address)?;"),
            (Size::Byte, false) => line!(out, "let v = u32::from(mem.read_u8(address)?);"),
            (Size::Byte, true) => line!(out, "let v = mem.read_u8(address)? as i8 as i32 as u32;"),
            (Size::Half, false) => line!(out, "let v = u32::from(mem.read_u16(address)?);"),
            (Size::Half, true) => line!(out, "let v = mem.read_u16(address)? as i16 as i32 as u32;"),
            (Size::Double, _) => {
                line!(out, "let v = read_aligned(mem, address)?;");
                line!(out, "let v2 = read_aligned(mem, address.wrapping_add(4))?;");
            }
        }
        wb(out);
        if rt == PC {
            line!(
                out,
                "cpu.from = {}; pc = {}; {}",
                a(pc.wrapping_sub(if thumb { 4 } else { 8 })),
                to_pc("bx_target(cpu, v)"),
                bx_tail(thumb)
            );
        } else {
            line!(out, "cpu.r[{rt}] = v;");
            if size == Size::Double {
                line!(out, "cpu.r[{}] = v2;", rt + 1);
            }
        }
    } else {
        let value = r(rt, pc);
        match size {
            Size::Word => line!(out, "mem.write_u32(address, {value})?;"),
            Size::Byte => line!(out, "mem.write_u8(address, {value} as u8)?;"),
            Size::Half => line!(out, "mem.write_u16(address, {value} as u16)?;"),
            Size::Double => {
                line!(out, "write_aligned(mem, address, cpu.r[{rt}])?;");
                line!(out, "write_aligned(mem, address.wrapping_add(4), cpu.r[{}])?;", rt + 1);
            }
        }
        wb(out);
    }
}

fn block_transfer(out: &mut Out, pc: u32, thumb: bool, instr: &Op) {
    let Op::Block { load, rn, list, before, increment, writeback } = *instr else { unreachable!() };
    let count = list.count_ones();
    line!(out, "let base = cpu.r[{rn}];");
    let start = match (before, increment) {
        (false, true) => "base".to_owned(),
        (true, true) => "base.wrapping_add(4)".to_owned(),
        (false, false) => format!("base.wrapping_sub({}).wrapping_add(4)", 4 * count),
        (true, false) => format!("base.wrapping_sub({})", 4 * count),
    };
    line!(out, "let start = {start};");
    let new_base =
        if increment { format!("base.wrapping_add({})", 4 * count) } else { format!("base.wrapping_sub({})", 4 * count) };
    let regs: Vec<u8> = (0..16u8).filter(|i| list & (1 << i) != 0).collect();
    if load {
        // read everything first: a fault leaves the registers untouched
        for (k, reg) in regs.iter().enumerate() {
            line!(out, "let l{reg} = read_aligned(mem, start.wrapping_add({}))?;", 4 * k);
        }
        if writeback {
            line!(out, "cpu.r[{rn}] = {new_base};");
        }
        for reg in regs.iter().filter(|&&reg| reg != PC) {
            line!(out, "cpu.r[{reg}] = l{reg};");
        }
        if list & (1 << PC) != 0 {
            line!(
                out,
                "cpu.from = {}; pc = {}; {}",
                a(pc.wrapping_sub(if thumb { 4 } else { 8 })),
                to_pc("bx_target(cpu, l15)"),
                bx_tail(thumb)
            );
        }
    } else {
        for (k, &reg) in regs.iter().enumerate() {
            // a base in the list stores its original value (decoder ensures it is lowest)
            let value = if reg == rn { "base".to_owned() } else { r(reg, pc) };
            line!(out, "write_aligned(mem, start.wrapping_add({}), {value})?;", 4 * k);
        }
        if writeback {
            line!(out, "cpu.r[{rn}] = {new_base};");
        }
    }
}

/// Read and write expressions for a VFP register in its precision.
fn vreg(double: bool, n: u8) -> String {
    if double {
        format!("vfp::d(cpu, {n})")
    } else {
        format!("cpu.s[{n}]")
    }
}

fn vset(out: &mut Out, double: bool, n: u8, value: &str) {
    if double {
        line!(out, "let v = {value}; vfp::set_d(cpu, {n}, v);");
    } else {
        line!(out, "let v = {value}; cpu.s[{n}] = v;");
    }
}

fn vfp_data(out: &mut Out, address: u32, op: VfpOp, double: bool, d: u8, n: u8, m: u8) {
    let w = if double { "64" } else { "32" };
    let sign = if double { "0x8000_0000_0000_0000u64" } else { "0x8000_0000u32" };
    let (rd, rn, rm) = (vreg(double, d), vreg(double, n), vreg(double, m));
    // the scalar form and, for vectorizable ops, the slow-path selector
    let (scalar, slow): (String, Option<(&str, String)>) = match op {
        VfpOp::Add | VfpOp::Sub | VfpOp::Mul | VfpOp::Div => {
            let a = match op {
                VfpOp::Add => "Add",
                VfpOp::Sub => "Sub",
                VfpOp::Mul => "Mul",
                _ => "Div",
            };
            (
                format!("vfp::arith{w}(cpu, vfp::Arith::{a}, {rn}, {rm})"),
                Some(("vfp::SLOW_BITS", format!("vfp::Vector::Arith(vfp::Arith::{a})"))),
            )
        }
        VfpOp::Mla | VfpOp::Mls | VfpOp::Nmla | VfpOp::Nmls => {
            let (np, nd) = match op {
                VfpOp::Mla => (false, false),
                VfpOp::Mls => (true, false),
                VfpOp::Nmla => (true, true),
                _ => (false, true),
            };
            let vector = format!("vfp::Vector::Mla {{ negate_product: {np}, negate_d: {nd} }}");
            (format!("vfp::mla{w}(cpu, {rd}, {rn}, {rm}, {np}, {nd})"), Some(("vfp::SLOW_BITS", vector)))
        }
        VfpOp::Nmul => (
            format!("vfp::arith{w}(cpu, vfp::Arith::Mul, {rn}, {rm}) ^ {sign}"),
            Some(("vfp::SLOW_BITS", "vfp::Vector::Nmul".to_owned())),
        ),
        VfpOp::Sqrt => (format!("vfp::sqrt{w}(cpu, {rm})"), Some(("vfp::SLOW_BITS", "vfp::Vector::Sqrt".to_owned()))),
        VfpOp::Mov => (rm.clone(), Some(("vfp::VECTOR_BITS", "vfp::Vector::Mov".to_owned()))),
        VfpOp::Abs => (format!("{rm} & !{sign}"), Some(("vfp::VECTOR_BITS", "vfp::Vector::Abs".to_owned()))),
        VfpOp::Neg => (format!("{rm} ^ {sign}"), Some(("vfp::VECTOR_BITS", "vfp::Vector::Neg".to_owned()))),
        VfpOp::Cmp { zero, .. } => {
            let b = if zero { format!("0u{w}") } else { rm.clone() };
            line!(out, "let (a, b) = ({rd}, {b}); vfp::cmp{w}(cpu, a, b);");
            return;
        }
        VfpOp::Precision => {
            if double {
                line!(out, "let v = vfp::f64_to_f32(cpu, vfp::d(cpu, {m})); cpu.s[{d}] = v;");
            } else {
                line!(out, "let v = vfp::f32_to_f64(cpu, cpu.s[{m}]); vfp::set_d(cpu, {d}, v);");
            }
            return;
        }
        VfpOp::FromInt { signed } => {
            if double {
                line!(out, "let v = vfp::int_to_f64(cpu.s[{m}], {signed}); vfp::set_d(cpu, {d}, v);");
            } else {
                line!(out, "let v = vfp::int_to_f32(cpu, cpu.s[{m}], {signed}); cpu.s[{d}] = v;");
            }
            return;
        }
        VfpOp::ToInt { signed, toward_zero } => {
            line!(out, "let v = vfp::f{w}_to_int(cpu, {rm}, {signed}, {toward_zero}); cpu.s[{d}] = v;");
            return;
        }
    };
    let (mask, vector) = slow.expect("vectorizable");
    line!(out, "if cpu.fpscr & {mask} == 0 {{");
    vset(out, double, d, &scalar);
    line!(out, "}} else {{ vfp::slow(cpu, {vector}, {double}, {d}, {n}, {m}, {})?; }}", a(address));
}

fn vfp_transfer(out: &mut Out, pc: u32, op: &Op) {
    match *op {
        Op::VfpMem { load, double, d, rn, offset, add } => {
            line!(
                out,
                "let address = aligned({}.{}({}), 4)?;",
                r(rn, pc),
                if add { "wrapping_add" } else { "wrapping_sub" },
                hex(offset)
            );
            let first = if double { 2 * d } else { d };
            let words = if double { 2 } else { 1 };
            if load {
                for k in 0..words {
                    line!(out, "let w{k} = mem.read_u32(address.wrapping_add({}))?;", 4 * k);
                }
                for k in 0..words {
                    line!(out, "cpu.s[{}] = w{k};", first + k);
                }
            } else {
                for k in 0..words {
                    line!(out, "mem.write_u32(address.wrapping_add({}), cpu.s[{}])?;", 4 * k, first + k);
                }
            }
        }
        Op::VfpBlock { load, double, first, count, rn, increment, writeback } => {
            let words = u32::from(count) * if double { 2 } else { 1 };
            let first = u32::from(first) * if double { 2 } else { 1 };
            line!(out, "let base = cpu.r[{rn}];");
            let start = if increment { "base".to_owned() } else { format!("base.wrapping_sub({})", 4 * words) };
            line!(out, "let start = aligned({start}, 4)?;");
            let new_base =
                if increment { format!("base.wrapping_add({})", 4 * words) } else { format!("base.wrapping_sub({})", 4 * words) };
            if load {
                for k in 0..words {
                    line!(out, "let w{k} = mem.read_u32(start.wrapping_add({}))?;", 4 * k);
                }
                if writeback {
                    line!(out, "cpu.r[{rn}] = {new_base};");
                }
                for k in 0..words {
                    line!(out, "cpu.s[{}] = w{k};", first + k);
                }
            } else {
                for k in 0..words {
                    line!(out, "mem.write_u32(start.wrapping_add({}), cpu.s[{}])?;", 4 * k, first + k);
                }
                if writeback {
                    line!(out, "cpu.r[{rn}] = {new_base};");
                }
            }
        }
        Op::VmovSingle { to_core: true, rt, s } => line!(out, "cpu.r[{rt}] = cpu.s[{s}];"),
        Op::VmovSingle { to_core: false, rt, s } => line!(out, "cpu.s[{s}] = cpu.r[{rt}];"),
        Op::VmovPair { to_core, rt, rt2, double, m } => {
            let lo = if double { 2 * m } else { m };
            if to_core {
                line!(out, "cpu.r[{rt}] = cpu.s[{lo}]; cpu.r[{rt2}] = cpu.s[{}];", lo + 1);
            } else {
                line!(out, "let (a, b) = (cpu.r[{rt}], cpu.r[{rt2}]); cpu.s[{lo}] = a; cpu.s[{}] = b;", lo + 1);
            }
        }
        Op::VmovHalf { to_core, rt, d, high } => {
            let s = 2 * d + u8::from(high);
            if to_core {
                line!(out, "cpu.r[{rt}] = cpu.s[{s}];");
            } else {
                line!(out, "cpu.s[{s}] = cpu.r[{rt}];");
            }
        }
        Op::Vmrs { rt: PC } => line!(out, "vfp::flags_to_apsr(cpu);"),
        Op::Vmrs { rt } => line!(out, "cpu.r[{rt}] = cpu.fpscr;"),
        Op::Vmsr { rt } => line!(out, "cpu.fpscr = cpu.r[{rt}] & vfp::WRITABLE;"),
        _ => unreachable!("not a VFP transfer"),
    }
}

/// The ARMv6 media and DSP instructions; semantics in `usum_guest::ops`.
fn media(out: &mut Out, op: &Op) {
    match *op {
        Op::Parallel { prefix, op, rd, rn, rm } => {
            line!(out, "let (a, b) = (cpu.r[{rn}], cpu.r[{rm}]); let v = parallel(cpu, {prefix}, {op}, a, b); cpu.r[{rd}] = v;")
        }
        Op::Sel { rd, rn, rm } => line!(out, "let v = sel(cpu, cpu.r[{rn}], cpu.r[{rm}]); cpu.r[{rd}] = v;"),
        Op::Pack { top: false, rd, rn, rm, shift } => {
            line!(out, "cpu.r[{rd}] = (cpu.r[{rn}] & 0xFFFF) | (cpu.r[{rm}] << {shift} & 0xFFFF_0000);")
        }
        Op::Pack { top: true, rd, rn, rm, shift } => {
            // ASR #0 encodes ASR #32
            let amount = if shift == 0 { 31 } else { shift };
            line!(out, "cpu.r[{rd}] = (cpu.r[{rn}] & 0xFFFF_0000) | ((cpu.r[{rm}] as i32 >> {amount}) as u32 & 0xFFFF);")
        }
        Op::Saturate { signed, half: false, sat, rd, rn, asr, amount } => {
            let operand = match (asr, amount) {
                (false, a) => format!("i64::from((cpu.r[{rn}] << {a}) as i32)"),
                (true, 0) => format!("i64::from(cpu.r[{rn}] as i32 >> 31)"),
                (true, a) => format!("i64::from(cpu.r[{rn}] as i32 >> {a})"),
            };
            let call = if signed { format!("signed_sat(cpu, x, {})", sat + 1) } else { format!("unsigned_sat(cpu, x, {sat})") };
            line!(out, "let x = {operand}; let v = {call}; cpu.r[{rd}] = v;");
        }
        Op::Saturate { signed, half: true, sat, rd, rn, .. } => {
            let bits = if signed { sat + 1 } else { sat };
            line!(out, "let x = cpu.r[{rn}]; let v = sat16(cpu, x, {bits}, {signed}); cpu.r[{rd}] = v;");
        }
        Op::ExtendAdd { signed, kind, rd, rn, rm, rotate } => {
            line!(out, "let x = cpu.r[{rm}].rotate_right({rotate});");
            let byte = |e: &str| if signed { format!("({e} as u8 as i8 as i32 as u32)") } else { format!("({e} as u8 as u32)") };
            match kind {
                0 | 1 => {
                    let value = match (kind, signed) {
                        (0, _) => byte("x"),
                        (_, true) => "(x as u16 as i16 as i32 as u32)".to_owned(),
                        _ => "(x as u16 as u32)".to_owned(),
                    };
                    let base = rn.map_or("0u32".to_owned(), |n| format!("cpu.r[{n}]"));
                    line!(out, "cpu.r[{rd}] = {base}.wrapping_add({value});");
                }
                _ => {
                    line!(out, "let (lo, hi) = ({}, {});", byte("x"), byte("(x >> 16)"));
                    match rn {
                        Some(n) => line!(
                            out,
                            "let a = cpu.r[{n}]; cpu.r[{rd}] = (a.wrapping_add(lo) & 0xFFFF) | ((a >> 16).wrapping_add(hi) << 16);"
                        ),
                        None => line!(out, "cpu.r[{rd}] = (lo & 0xFFFF) | (hi << 16);"),
                    }
                }
            }
        }
        Op::DualMul { kind, swap, rd, ra, rn, rm } => {
            let m = if swap { format!("cpu.r[{rm}].rotate_right(16)") } else { format!("cpu.r[{rm}]") };
            line!(out, "let (n, m) = (cpu.r[{rn}], {m}); let p1 = half(n, false) * half(m, false); let p2 = half(n, true) * half(m, true);");
            let combine = if matches!(kind, DualMul::Add | DualMul::AddLong) { "p1 + p2" } else { "p1 - p2" };
            match (kind, ra) {
                (DualMul::Add | DualMul::Sub, ra) => {
                    let acc = ra.map_or("0i64".to_owned(), |a| format!("i64::from(cpu.r[{a}] as i32)"));
                    line!(out, "let acc = {acc}; let v = add_q(cpu, {combine}, acc); cpu.r[{rd}] = v;");
                }
                (_, Some(lo)) => line!(
                    out,
                    "let acc = ((u64::from(cpu.r[{rd}]) << 32) | u64::from(cpu.r[{lo}])) as i64; let v = acc.wrapping_add({combine}) as u64; cpu.r[{lo}] = v as u32; cpu.r[{rd}] = (v >> 32) as u32;"
                ),
                _ => unreachable!("long forms always accumulate"),
            }
        }
        Op::MostSignificant { subtract, round, rd, ra, rn, rm } => {
            let acc = ra.map_or("0i128".to_owned(), |a| format!("(i128::from(cpu.r[{a}] as i32) << 32)"));
            let sign = if subtract { "-" } else { "+" };
            let round = if round { " + 0x8000_0000" } else { "" };
            line!(out, "let p = i128::from(cpu.r[{rn}] as i32) * i128::from(cpu.r[{rm}] as i32); cpu.r[{rd}] = (({acc} {sign} p{round}) >> 32) as u32;");
        }
        Op::Usad { rd, ra, rn, rm } => {
            let acc = ra.map_or(String::new(), |a| format!(".wrapping_add(cpu.r[{a}])"));
            line!(out, "cpu.r[{rd}] = usad8(cpu.r[{rn}], cpu.r[{rm}]){acc};");
        }
        Op::HalfMul { kind, x, y, rd, ra, rn, rm } => match kind {
            HalfMul::Mul => line!(out, "cpu.r[{rd}] = (half(cpu.r[{rn}], {x}) * half(cpu.r[{rm}], {y})) as u32;"),
            HalfMul::Mla => line!(
                out,
                "let p = half(cpu.r[{rn}], {x}) * half(cpu.r[{rm}], {y}); let acc = i64::from(cpu.r[{ra}] as i32); let v = add_q(cpu, p, acc); cpu.r[{rd}] = v;"
            ),
            HalfMul::Mulw => line!(out, "cpu.r[{rd}] = ((i64::from(cpu.r[{rn}] as i32) * half(cpu.r[{rm}], {y})) >> 16) as u32;"),
            HalfMul::Mlaw => line!(
                out,
                "let p = (i64::from(cpu.r[{rn}] as i32) * half(cpu.r[{rm}], {y})) >> 16; let acc = i64::from(cpu.r[{ra}] as i32); let v = add_q(cpu, p, acc); cpu.r[{rd}] = v;"
            ),
            HalfMul::Mlal => line!(
                out,
                "let acc = ((u64::from(cpu.r[{rd}]) << 32) | u64::from(cpu.r[{ra}])) as i64; let v = acc.wrapping_add(half(cpu.r[{rn}], {x}) * half(cpu.r[{rm}], {y})) as u64; cpu.r[{ra}] = v as u32; cpu.r[{rd}] = (v >> 32) as u32;"
            ),
        },
        Op::SatArith { op, rd, rn, rm } => line!(out, "let (m, n) = (cpu.r[{rm}], cpu.r[{rn}]); let v = qarith(cpu, {op}, m, n); cpu.r[{rd}] = v;"),
        Op::Umaal { lo, hi, rn, rm } => line!(
            out,
            "let v = u64::from(cpu.r[{rn}]) * u64::from(cpu.r[{rm}]) + u64::from(cpu.r[{hi}]) + u64::from(cpu.r[{lo}]); cpu.r[{lo}] = v as u32; cpu.r[{hi}] = (v >> 32) as u32;"
        ),
        _ => unreachable!("not a media instruction"),
    }
}

fn instruction(out: &mut Out, analysis: &Analysis, callee: &dyn Fn(u32) -> String, address: u32, instr: &Instr) {
    let next = address.wrapping_add(u32::from(instr.size));
    let conditional = instr.cond != AL;
    if conditional {
        line!(out, "if cond(cpu, {}) {{", instr.cond);
    } else {
        line!(out, "{{");
    }
    match instr.op {
        Op::Data { op, s, rd, rn, operand } => data(out, instr.pc, instr.thumb, op, s, rd, rn, operand),
        Op::Mul { accumulate, s, rd, rn, rm, rs } => {
            let acc = if accumulate { format!(".wrapping_add(cpu.r[{rn}])") } else { String::new() };
            line!(out, "let v = cpu.r[{rm}].wrapping_mul(cpu.r[{rs}]){acc};");
            line!(out, "cpu.r[{rd}] = v;");
            if s {
                line!(out, "nz(cpu, v);");
            }
        }
        Op::MulLong { signed, accumulate, s, lo, hi, rm, rs } => {
            if signed {
                line!(out, "let mut v = (i64::from(cpu.r[{rm}] as i32) * i64::from(cpu.r[{rs}] as i32)) as u64;");
            } else {
                line!(out, "let mut v = u64::from(cpu.r[{rm}]) * u64::from(cpu.r[{rs}]);");
            }
            if accumulate {
                line!(out, "v = v.wrapping_add((u64::from(cpu.r[{hi}]) << 32) | u64::from(cpu.r[{lo}]));");
            }
            line!(out, "cpu.r[{lo}] = v as u32; cpu.r[{hi}] = (v >> 32) as u32;");
            if s {
                line!(out, "cpu.n = v >> 63 == 1; cpu.z = v == 0;");
            }
        }
        Op::Clz { rd, rm } => line!(out, "cpu.r[{rd}] = clz(cpu.r[{rm}]);"),
        Op::Mem { .. } => mem_access(out, instr.pc, instr.thumb, &instr.op),
        Op::Block { .. } => block_transfer(out, instr.pc, instr.thumb, &instr.op),
        Op::Extend { signed, half, rd, rm, rotate } => {
            let cast = match (signed, half) {
                (true, true) => "as i16 as i32 as u32",
                (true, false) => "as i8 as i32 as u32",
                (false, true) => "as u16 as u32",
                (false, false) => "as u8 as u32",
            };
            line!(out, "cpu.r[{rd}] = cpu.r[{rm}].rotate_right({rotate}) {cast};");
        }
        Op::Rev { kind, rd, rm } => match kind {
            RevKind::Word => line!(out, "cpu.r[{rd}] = cpu.r[{rm}].swap_bytes();"),
            RevKind::Halves => {
                line!(out, "let x = cpu.r[{rm}]; cpu.r[{rd}] = ((x & 0xFF00FF00) >> 8) | ((x & 0x00FF00FF) << 8);")
            }
            RevKind::SignedHalf => line!(out, "cpu.r[{rd}] = (cpu.r[{rm}] as u16).swap_bytes() as i16 as i32 as u32;"),
        },
        Op::Branch { link: false, target, .. } => line!(out, "pc = {}; continue;", hex(target)),
        Op::Branch { link: true, target, exchange } => {
            let state = instr.thumb != exchange;
            if analysis.functions.contains_key(&key(target, state)) {
                line!(
                    out,
                    "match call(cpu, mem, {}, {}, {state}, {}, {})? {{ Flow::Local(p) => {{ pc = {}; continue; }} Flow::Leave(e) => return Ok(e) }}",
                    a(key(next, instr.thumb)),
                    a(target),
                    instr.thumb,
                    callee(key(target, state)),
                    to_pc("p")
                );
            } else {
                // a call to somewhere not in the image: let the host report it
                line!(out, "cpu.r[{LR}] = {}; cpu.thumb = {state};", a(key(next, instr.thumb)));
                line!(out, "cpu.r[15] = {}; return Ok(Exit::Jump);", a(target));
            }
        }
        Op::Bx { link, rm } => {
            line!(out, "let t = cpu.r[{rm}];");
            if link {
                line!(out, "cpu.r[{LR}] = {};", a(key(next, instr.thumb)));
            }
            line!(out, "cpu.from = {}; pc = {}; {}", a(address), to_pc("bx_target(cpu, t)"), bx_tail(instr.thumb));
        }
        Op::Mrs { rd } => line!(out, "cpu.r[{rd}] = cpsr(cpu);"),
        Op::Msr { mask, imm: Some(value), .. } => line!(out, "msr(cpu, {}, {mask});", hex(value)),
        Op::Msr { mask, imm: None, rm } => line!(out, "msr(cpu, cpu.r[{rm}], {mask});"),
        Op::Svc { imm } => line!(out, "cpu.r[15] = {}; return Ok(Exit::Svc({}));", a(next), hex(imm)),
        Op::Nop => {}
        Op::Parallel { .. }
        | Op::Sel { .. }
        | Op::Pack { .. }
        | Op::Saturate { .. }
        | Op::ExtendAdd { .. }
        | Op::DualMul { .. }
        | Op::MostSignificant { .. }
        | Op::Usad { .. }
        | Op::HalfMul { .. }
        | Op::SatArith { .. }
        | Op::Umaal { .. } => media(out, &instr.op),
        Op::VfpData { op, double, d, n, m } => vfp_data(out, address, op, double, d, n, m),
        Op::VfpMem { .. }
        | Op::VfpBlock { .. }
        | Op::VmovSingle { .. }
        | Op::VmovPair { .. }
        | Op::VmovHalf { .. }
        | Op::Vmrs { .. }
        | Op::Vmsr { .. } => vfp_transfer(out, instr.pc, &instr.op),
        Op::ReadTls { rd } => line!(out, "cpu.r[{rd}] = cpu.tls;"),
        Op::ClearExclusive => line!(out, "cpu.exclusive = None;"),
        Op::Exclusive { load, size, status, rt, rn } => {
            let bytes = match size {
                Size::Byte => 1,
                Size::Half => 2,
                Size::Word => 4,
                Size::Double => 8,
            };
            line!(out, "let address = aligned(cpu.r[{rn}], {bytes})?;");
            if load {
                match size {
                    Size::Byte => line!(out, "let v = u32::from(mem.read_u8(address)?);"),
                    Size::Half => line!(out, "let v = u32::from(mem.read_u16(address)?);"),
                    Size::Word => line!(out, "let v = mem.read_u32(address)?;"),
                    Size::Double => {
                        line!(out, "let v = mem.read_u32(address)?; let v2 = mem.read_u32(address.wrapping_add(4))?;")
                    }
                }
                line!(out, "cpu.exclusive = Some(address); cpu.r[{rt}] = v;");
                if size == Size::Double {
                    line!(out, "cpu.r[{}] = v2;", rt + 1);
                }
            } else {
                line!(out, "if cpu.exclusive == Some(address) {{");
                match size {
                    Size::Byte => line!(out, "mem.write_u8(address, cpu.r[{rt}] as u8)?;"),
                    Size::Half => line!(out, "mem.write_u16(address, cpu.r[{rt}] as u16)?;"),
                    Size::Word => line!(out, "mem.write_u32(address, cpu.r[{rt}])?;"),
                    Size::Double => line!(
                        out,
                        "mem.write_u32(address, cpu.r[{rt}])?; mem.write_u32(address.wrapping_add(4), cpu.r[{}])?;",
                        rt + 1
                    ),
                }
                line!(out, "cpu.r[{status}] = 0; }} else {{ cpu.r[{status}] = 1; }}");
                line!(out, "cpu.exclusive = None;");
            }
        }
        Op::Unsupported => {
            line!(
                out,
                "cpu.r[15] = {}; return Err(Fault::Unsupported {{ address: {}, raw: {} }});",
                a(address),
                a(address),
                hex(instr.raw)
            )
        }
    }
    line!(out, "}}");
}

fn function(out: &mut Out, analysis: &Analysis, callee: &dyn Fn(u32) -> String, f: &Function) {
    line!(out, "pub fn {}(cpu: &mut Cpu, mem: &mut Memory) -> Result<Exit, Fault> {{", name(f.key()));
    if relocatable() {
        line!(out, "let delta = cpu.module_delta;");
    }
    line!(out, "let mut pc = {};", to_pc("cpu.r[15]"));
    line!(out, "loop {{ match pc {{");
    for (label, body) in blocks(f) {
        if body.is_empty() {
            continue; // outside the image: the default arm hands it to the host
        }
        let count = body.len();
        line!(out, "{} => {{", hex(label));
        line!(
            out,
            "cpu.budget -= {count}; if cpu.budget < 0 {{ cpu.budget += {count}; cpu.r[15] = {}; return Ok(Exit::Budget); }}",
            from_pc()
        );
        for (address, instr) in &body {
            instruction(out, analysis, callee, *address, instr);
        }
        let (last_address, last) = body[count - 1];
        if !last.always_leaves() {
            line!(out, "pc = {};", hex(last_address.wrapping_add(u32::from(last.size))));
        }
        line!(out, "}}");
    }
    line!(out, "_ => {{ cpu.r[15] = {}; return Ok(Exit::Jump); }}", from_pc());
    line!(out, "}} }}");
    line!(out, "}}");
}

/// Emits a Rust module body for `analysis`: one function per guest
/// function and `ENTRIES`, the sorted table of every label. The caller
/// wraps it in a module with `#[allow(unused, unreachable_code)]`.
const PRELUDE: &str = "use usum_guest::ops::*;\nuse usum_guest::vfp;\nuse usum_guest::{call, function, spend, Code, Cpu, Exit, Fault, Flow, Memory};\n";

/// Every label is an entry, keyed like functions (bit 0 for Thumb); the
/// function that starts at a label owns it, else the first that has it.
fn entries(analysis: &Analysis) -> std::collections::BTreeMap<u32, u32> {
    let mut entries = std::collections::BTreeMap::new();
    for f in analysis.functions.values() {
        for &label in &f.labels {
            if f.instrs.contains_key(&label) {
                let owner = entries.entry(key(label, f.thumb)).or_insert(f.key());
                if label == f.start {
                    *owner = f.key();
                }
            }
        }
    }
    entries
}

/// Emits a Rust module body for `analysis`: one function per guest
/// function and `ENTRIES`, the sorted table of every label. The caller
/// wraps it in a module with `#[allow(unused, unreachable_code)]`.
pub fn module(analysis: &Analysis) -> String {
    let mut out = Out(PRELUDE.to_owned());
    for f in analysis.functions.values() {
        function(&mut out, analysis, &name, f);
    }
    line!(out, "pub static ENTRIES: &[(u32, Code)] = &[");
    for (entry, owner) in entries(analysis) {
        line!(out, "({}, {}),", hex(entry), name(owner));
    }
    line!(out, "];");
    out.0
}

/// One unit of split output: a crate (or module) of generated functions.
pub struct Unit {
    pub source: String,
    pub functions: usize,
    pub instructions: usize,
}

/// Output split into units of at most about `max_instructions` each, so
/// no single compilation unit exhausts memory (RESEARCH.md R-013). Calls
/// within a unit are direct; calls across units go through the function
/// table (`usum_guest::function`), since units cannot depend on each
/// other. `top` defines `FUNCTIONS` (the table, in index order; install it
/// with `usum_guest::install_functions` before running) and `ENTRIES`,
/// naming unit `i`'s items through `unit_path(i)`.
pub struct Split {
    pub units: Vec<Unit>,
    pub top: String,
}

pub fn split(analysis: &Analysis, max_instructions: usize, unit_path: &dyn Fn(usize) -> String) -> Split {
    split_with(analysis, max_instructions, unit_path, 0, false)
}

/// `split`, with function table indices starting at `index_base` (output
/// for modules follows the executable's in the one table), and
/// `relocatable` for code translated from CRO modules at virtual bases
/// (ADR-0007; see `RELOCATABLE`).
pub fn split_with(
    analysis: &Analysis,
    max_instructions: usize,
    unit_path: &dyn Fn(usize) -> String,
    index_base: usize,
    relocatable: bool,
) -> Split {
    RELOCATABLE.with(|r| r.set(relocatable));
    let split = split_inner(analysis, max_instructions, unit_path, index_base);
    RELOCATABLE.with(|r| r.set(false));
    split
}

fn split_inner(analysis: &Analysis, max_instructions: usize, unit_path: &dyn Fn(usize) -> String, index_base: usize) -> Split {
    let keys: Vec<u32> = analysis.functions.keys().copied().collect();
    let index: std::collections::HashMap<u32, usize> = keys.iter().enumerate().map(|(i, &k)| (k, index_base + i)).collect();
    // A function's unit is the window of `max_instructions * 4` bytes its
    // start lies in, numbered in address order. Fixed windows keep units
    // stable when discovery adds functions: only the windows they fall in
    // change, so only those crates rebuild (RESEARCH.md R-026).
    let window = (max_instructions as u64 * 4).max(4);
    let windows: std::collections::BTreeSet<u64> = analysis.functions.values().map(|f| u64::from(f.start) / window).collect();
    let number: std::collections::HashMap<u64, usize> = windows.iter().enumerate().map(|(i, &w)| (w, i)).collect();
    let unit_of: std::collections::HashMap<u32, usize> =
        analysis.functions.values().map(|f| (f.key(), number[&(u64::from(f.start) / window)])).collect();
    let unit = windows.len().saturating_sub(1);
    let mut units: Vec<Unit> = (0..=unit).map(|_| Unit { source: PRELUDE.to_owned(), functions: 0, instructions: 0 }).collect();
    for f in analysis.functions.values() {
        let here = unit_of[&f.key()];
        let callee = |k: u32| if unit_of[&k] == here { name(k) } else { format!("function({})", index[&k]) };
        let mut out = Out(std::mem::take(&mut units[here].source));
        function(&mut out, analysis, &callee, f);
        units[here].source = out.0;
        units[here].functions += 1;
        units[here].instructions += f.instrs.len();
    }
    let path = |k: u32| format!("{}::{}", unit_path(unit_of[&k]), name(k));
    let mut top = Out(String::from("use usum_guest::Code;\n"));
    line!(top, "pub static FUNCTIONS: &[Code] = &[");
    for &k in &keys {
        line!(top, "{},", path(k));
    }
    line!(top, "];");
    line!(top, "pub static ENTRIES: &[(u32, Code)] = &[");
    for (entry, owner) in entries(analysis) {
        line!(top, "({}, {}),", hex(entry), path(owner));
    }
    line!(top, "];");
    Split { units, top: top.0 }
}
