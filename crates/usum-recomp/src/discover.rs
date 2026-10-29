//! Finding the code: recursive descent from entry points.
//!
//! A *function* starts at an entry point or a call target, in one
//! instruction set: functions are keyed by address with bit 0 set for
//! Thumb, as the host looks them up. Within a function, everything
//! reachable without following calls is explored; the state only changes
//! through BX/BLX, which leave the function or call another one.
//!
//! *Labels* (block starts) are the places execution can arrive other than
//! by falling through: the function start, branch targets, the instruction
//! after a call (the return point), the instruction after an SVC (where
//! the host resumes), and jump-table targets. Every label is also an entry
//! in the host's table, which is what lets the host resume anywhere a call
//! or SVC unwound to.
//!
//! Code reached only through computed addresses is found from *pointer
//! seeds*: aligned words anywhere in the image, and byte-packed words in data,
//! whose value lies in the code
//! range (odd values: Thumb); *self-relative* words outside the code range
//! whose own address plus their value does (the compiler's tables of
//! static constructors store `target - entry`; RESEARCH.md R-025); and
//! addresses that ARM `adr` (`add`/`sub rd, pc, #imm`) computes. Each seed whose first instruction decodes starts a
//! function. False seeds only cost unused output; missed ones fail loudly
//! at run time.

use std::collections::{BTreeMap, BTreeSet};

use usum_arm::{decode, decode_thumb, DataOp, Instr, Offset, Op, Operand, Size, AL, PC};

/// A flat image: bytes as loaded at `base`, of which `code` holds the
/// executable addresses (for a 3DS title, the exheader's text segment).
pub struct Image<'a> {
    pub base: u32,
    pub bytes: &'a [u8],
    pub code: std::ops::Range<u32>,
}

impl Image<'_> {
    fn bytes_at<const N: usize>(&self, address: u32) -> Option<[u8; N]> {
        let offset = address.checked_sub(self.base)? as usize;
        self.bytes.get(offset..offset.checked_add(N)?)?.try_into().ok()
    }

    pub fn word(&self, address: u32) -> Option<u32> {
        self.bytes_at(address).map(u32::from_le_bytes)
    }

    pub fn half(&self, address: u32) -> Option<u16> {
        self.bytes_at(address).map(u16::from_le_bytes)
    }

    /// The instruction at `address` in the given state, if it is in the image.
    pub fn fetch(&self, address: u32, thumb: bool) -> Option<Instr> {
        if thumb {
            let first = self.half(address)?;
            Some(decode_thumb(first, self.half(address.wrapping_add(2)), address))
        } else {
            self.word(address).map(|raw| decode(raw, address))
        }
    }
}

/// A function key: the address, with bit 0 set for Thumb.
pub fn key(address: u32, thumb: bool) -> u32 {
    address | u32::from(thumb)
}

/// What made discovery start a function.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    #[default]
    Entry,
    Call,
    Pointer,
}

#[derive(Debug, Default)]
pub struct Function {
    pub start: u32,
    pub thumb: bool,
    pub origin: Origin,
    /// Every reachable instruction, by address.
    pub instrs: BTreeMap<u32, Instr>,
    pub labels: BTreeSet<u32>,
    /// Calls out of this function, as function keys.
    pub calls: BTreeSet<u32>,
    /// Places execution may continue at that are not in the image.
    pub exits: BTreeSet<u32>,
}

impl Function {
    pub fn key(&self) -> u32 {
        key(self.start, self.thumb)
    }
}

#[derive(Debug, Default)]
pub struct Analysis {
    /// By function key.
    pub functions: BTreeMap<u32, Function>,
    /// How many functions each kind of seed started.
    pub from_entries: usize,
    pub from_calls: usize,
    pub from_pointers: usize,
}

/// Recognizes the jump table GCC emits for a dense `switch` in ARM code:
///
/// ```text
///     cmp   rX, #n
///     ldrls pc, [pc, rX, lsl #2]
///     b     default
///     .word case0, ..., case_n
/// ```
///
/// or its branch-table form, where the table holds `b case_i` instructions:
///
/// ```text
///     cmp   rX, #n
///     addls pc, pc, rX, lsl #2
///     b     default
///     b case0 ... b case_n
/// ```
///
/// Returns the table's targets. The CMP must be in the same function,
/// within the four instructions before.
fn jump_table(image: &Image, f: &Function, address: u32, instr: &Instr) -> Option<Vec<u32>> {
    if instr.thumb {
        return None;
    }
    let (rm, words) = match instr.op {
        Op::Mem {
            load: true,
            size: Size::Word,
            rt: PC,
            rn: PC,
            offset: Offset::Reg { rm, kind: 0, amount: 2 },
            add: true,
            pre: true,
            writeback: false,
            ..
        } => (rm, true),
        Op::Data { op: DataOp::Add, s: false, rd: PC, rn: PC, operand: Operand::ImmShift { rm, kind: 0, amount: 2 } } => {
            (rm, false)
        }
        _ => return None,
    };
    if instr.cond != 0x9 {
        return None; // LS: index <= n
    }
    let bound = (1..=4).find_map(|back| {
        let earlier = f.instrs.get(&address.wrapping_sub(4 * back))?;
        match earlier.op {
            Op::Data { op: DataOp::Cmp, rn, operand: Operand::Imm { value, .. }, .. } if rn == rm && earlier.cond == AL => {
                Some(value)
            }
            _ => None,
        }
    })?;
    // a bound larger than the code is not a real table
    if bound > 4096 {
        return None;
    }
    if words {
        (0..=bound).map(|i| image.word(instr.pc.wrapping_add(4 * i))).collect()
    } else {
        // the table entries are themselves code (branches), reached by computed PC
        Some((0..=bound).map(|i| instr.pc.wrapping_add(4 * i)).collect())
    }
}

/// Recognizes a table of offsets relative to its own address, used by
/// hand-written ARM code (Ultra Sun's video decoder around 0x00133600,
/// R-050):
///
/// ```text
///     sub   rB, pc, #n        @ or add: the table's address
///     ldr   rX, [rB, rI, lsl #2]
///     add   pc, rX, rB        @ or rB, rX
/// ```
///
/// Each entry plus the table's address is a target. The table has no
/// bound in the code, so entries are taken while they land on decodable,
/// word-aligned code in the image (at most 256).
fn relative_table(image: &Image, f: &Function, address: u32, instr: &Instr) -> Option<Vec<u32>> {
    if instr.thumb || instr.cond != AL {
        return None;
    }
    let Op::Data { op: DataOp::Add, s: false, rd: PC, rn, operand: Operand::ImmShift { rm, kind: 0, amount: 0 } } = instr.op
    else {
        return None;
    };
    let earlier = |back: u32| f.instrs.get(&address.wrapping_sub(4 * back)).filter(|i| i.cond == AL);
    // the load of the entry, into one of the two registers, from rB
    let (back, base) = (1..=8).find_map(|back| match earlier(back)?.op {
        Op::Mem {
            load: true,
            size: Size::Word,
            rt,
            rn: base,
            offset: Offset::Reg { .. },
            add: true,
            pre: true,
            writeback: false,
            ..
        } if (rt == rn && base == rm) || (rt == rm && base == rn) => Some((back, base)),
        _ => None,
    })?;
    // the table's address, from the PC (`pc` is already the value the PC
    // reads as: the address + 8)
    // (the decoder at 0x00134448 sets it 8 instructions before the jump)
    let table = (back + 1..=back + 12).find_map(|b| {
        let i = earlier(b)?;
        match i.op {
            Op::Data { op: DataOp::Add, rd, rn: PC, operand: Operand::Imm { value, .. }, .. } if rd == base => {
                Some(i.pc.wrapping_add(value))
            }
            Op::Data { op: DataOp::Sub, rd, rn: PC, operand: Operand::Imm { value, .. }, .. } if rd == base => {
                Some(i.pc.wrapping_sub(value))
            }
            _ => None,
        }
    })?;
    let targets: Vec<u32> = (0..256)
        .map_while(|k| {
            let target = table.wrapping_add(image.word(table.wrapping_add(4 * k))?);
            (target % 4 == 0 && image.fetch(target, false).is_some()).then_some(target)
        })
        .collect();
    (!targets.is_empty()).then_some(targets)
}

/// The value of `reg` just before `address`, when the instructions before
/// it compute it from the PC with immediate adds and subtracts only, as
/// hand-written ARM does for a jump target (Ultra Sun's video decoder:
/// `add r4, pc, #0x2400; add r4, r4, #0x248; movle pc, r4`, R-050). Looks
/// back at most `depth` instructions; any other write to `reg` gives None.
fn pc_constant(f: &Function, address: u32, reg: u8, depth: u32) -> Option<u32> {
    for back in 1..=depth {
        let at = address.wrapping_sub(4 * back);
        let i = f.instrs.get(&at)?;
        let Op::Data { op, rd, rn, operand, .. } = i.op else {
            continue;
        };
        if rd != reg {
            continue;
        }
        if i.cond != AL {
            return None;
        }
        let Operand::Imm { value, .. } = operand else { return None };
        let base = match rn {
            PC => i.pc,
            r if r == reg => pc_constant(f, at, reg, depth - back)?,
            _ => return None,
        };
        return match op {
            DataOp::Add => Some(base.wrapping_add(value)),
            DataOp::Sub => Some(base.wrapping_sub(value)),
            _ => None,
        };
    }
    None
}

fn explore(image: &Image, start: u32, thumb: bool) -> Function {
    let mut f = Function { start, thumb, ..Function::default() };
    f.labels.insert(start);
    let mut pending = vec![start];
    while let Some(mut address) = pending.pop() {
        loop {
            if f.instrs.contains_key(&address) {
                break;
            }
            let Some(instr) = image.fetch(address, thumb) else {
                f.exits.insert(address);
                break;
            };
            f.instrs.insert(address, instr);
            let next = address.wrapping_add(u32::from(instr.size));
            let go = |target: u32, f: &mut Function, pending: &mut Vec<u32>| {
                f.labels.insert(target);
                pending.push(target);
            };
            match instr.op {
                Op::Branch { link: false, target, .. } => go(target, &mut f, &mut pending),
                Op::Branch { link: true, target, exchange } => {
                    f.calls.insert(key(target, thumb != exchange));
                    if !thumb && !exchange && returns_into_table(image, target) {
                        // the helper reads a word after the call and returns
                        // to one of the four words after that (R-034)
                        for k in 0..4 {
                            go(next + 4 + 4 * k, &mut f, &mut pending);
                        }
                    } else {
                        go(next, &mut f, &mut pending);
                    }
                }
                Op::Bx { link: true, .. } | Op::Svc { .. } => go(next, &mut f, &mut pending),
                _ => {}
            }
            // A return address set by hand (`add lr, pc, #n`, possibly
            // followed by more immediate adds/subtracts to lr, before a
            // branch): execution continues there when the callee returns
            // (Ultra Sun's video decoder, R-050). Reuse the same bounded
            // constant recovery as computed PC jumps so chained forms are
            // followed too.
            if matches!(instr.op, Op::Data { op: DataOp::Add | DataOp::Sub, rd: usum_arm::LR, .. }) {
                let target = pc_constant(&f, next, usum_arm::LR, 8);
                if let Some(target) = target.filter(|target| !thumb && target % 4 == 0 && image.fetch(*target, false).is_some()) {
                    go(target, &mut f, &mut pending);
                }
            }
            // `mov pc, rX` with rX computed from the PC
            if let Op::Data { op: DataOp::Mov, rd: PC, operand: Operand::ImmShift { rm, kind: 0, amount: 0 }, .. } = instr.op {
                if let Some(target) = (!thumb).then(|| pc_constant(&f, address, rm, 8)).flatten() {
                    if target % 4 == 0 && image.fetch(target, false).is_some() {
                        go(target, &mut f, &mut pending);
                    }
                }
            }
            if let Some(targets) = jump_table(image, &f, address, &instr).or_else(|| relative_table(image, &f, address, &instr)) {
                for target in targets {
                    go(target, &mut f, &mut pending);
                }
            }
            if instr.always_leaves() {
                break;
            }
            address = next;
        }
    }
    f
}

/// Code addresses written in the image or computed by ARM `adr`, as keys.
/// An ARM helper that starts `ldr rX, [lr], #4`: it reads a word placed
/// after the call and returns past it into a table indexed by what it
/// computes (`add ip, lr, rX, lsl #2; bx ip`). The ARM compiler's
/// floating-point compare helpers do this; Ultra Sun has one at
/// 0x005B9590, whose callers continue at call + 8 + 4k (k = 0..3).
fn returns_into_table(image: &Image, target: u32) -> bool {
    image.fetch(target, false).is_some_and(|i| {
        matches!(
            i.op,
            Op::Mem { load: true, size: Size::Word, rn: usum_arm::LR, offset: Offset::Imm(4), add: true, pre: false, .. }
        ) && i.cond == AL
    })
}

fn pointer_seeds(image: &Image, analysis: &Analysis, data_words: bool) -> BTreeSet<u32> {
    let in_code = |a: u32| image.code.contains(&(a & !1));
    let candidate = |value: u32| in_code(value) && (value & 1 == 1 || value.is_multiple_of(4));
    let mut seeds = BTreeSet::new();
    let words = if data_words { image.bytes.len() as u32 / 4 } else { 0 };
    for i in 0..words {
        let address = image.base + 4 * i;
        let Some(value) = image.word(address) else { continue };
        if candidate(value) {
            seeds.insert(value);
        }
        // self-relative entries live in data, never in the code itself
        let relative = address.wrapping_add(value);
        if !image.code.contains(&address) && value != 0 && candidate(relative) {
            seeds.insert(relative);
        }
    }
    // Absolute pointers in byte-packed data tables need not be aligned to
    // either a word or a halfword. Keep instruction-target alignment checks;
    // only the location storing the pointer may have any byte alignment.
    for i in 0..words {
        for offset in [1, 2, 3] {
            let address = image.base + 4 * i + offset;
            if image.code.contains(&address) {
                continue;
            }
            if let Some(value) = image.word(address).filter(|&v| candidate(v)) {
                seeds.insert(value);
            }
        }
    }
    for instr in analysis.functions.values().filter(|f| !f.thumb).flat_map(|f| f.instrs.values()) {
        if let Op::Data { op: op @ (DataOp::Add | DataOp::Sub), rn: PC, rd, operand: Operand::Imm { value, .. }, .. } = instr.op {
            let target = if op == DataOp::Add { instr.pc.wrapping_add(value) } else { instr.pc.wrapping_sub(value) };
            // an odd result is a Thumb address: armlink's ARM-to-Thumb veneer
            // is `add r12, pc, #1; bx r12` in front of the Thumb function
            if rd != PC && candidate(target) {
                seeds.insert(target);
            }
        }
    }
    // position-independent addresses: `ldr rX, [pc, #n]` (an offset from
    // the literal pool) then, within a few instructions, `add rX, pc` /
    // `add rX, pc, rX`: the target is the literal plus that add's PC
    for f in analysis.functions.values() {
        let instrs: Vec<&Instr> = f.instrs.values().collect();
        for (i, load) in instrs.iter().enumerate() {
            let Op::Mem {
                load: true,
                size: Size::Word,
                rt,
                rn: PC,
                offset: Offset::Imm(off),
                add,
                pre: true,
                writeback: false,
                ..
            } = load.op
            else {
                continue;
            };
            let literal = if add { load.pc.wrapping_add(off) } else { load.pc.wrapping_sub(off) };
            let Some(value) = image.word(literal) else { continue };
            for next in instrs.iter().skip(i + 1).take(4) {
                let adds_pc = match next.op {
                    Op::Data { op: DataOp::Add, rd, rn, operand: Operand::ImmShift { rm, kind: 0, amount: 0 }, .. }
                        if rd == rt =>
                    {
                        (rn == rt && rm == PC) || (rn == PC && rm == rt)
                    }
                    _ => false,
                };
                if adds_pc {
                    let target = value.wrapping_add(next.pc);
                    if candidate(target) {
                        seeds.insert(target);
                    }
                    break;
                }
            }
        }
    }
    seeds.retain(|&k| image.fetch(k & !1, k & 1 == 1).is_some_and(|i| i.op != Op::Unsupported));
    seeds
}

/// Explores from `entries` (function keys), following calls to new
/// functions, then from pointer seeds, until no new seeds appear.
pub fn analyze(image: &Image, entries: &[u32]) -> Analysis {
    analyze_with(image, entries, true)
}

/// `analyze`; `data_words` false leaves out seeds from words in the image
/// (absolute and self-relative pointers). A CRO module as stored is not
/// relocated yet, so its words do not hold addresses: its seeds come from
/// its relocation and export tables instead (ADR-0007). Seeds computed
/// from code (ADR, PC-relative literals) stay.
pub fn analyze_with(image: &Image, entries: &[u32], data_words: bool) -> Analysis {
    let mut analysis = Analysis::default();
    let explore_all = |analysis: &mut Analysis, starts: Vec<u32>, kind: usize| {
        let mut pending: Vec<(u32, usize)> = starts.into_iter().map(|s| (s, kind)).collect();
        while let Some((k, kind)) = pending.pop() {
            let (start, thumb) = (k & !1, k & 1 == 1);
            if analysis.functions.contains_key(&k) || image.fetch(start, thumb).is_none() {
                continue;
            }
            let mut f = explore(image, start, thumb);
            f.origin = [Origin::Entry, Origin::Call, Origin::Pointer][kind.min(2)];
            pending.extend(f.calls.iter().map(|&c| (c, 1)));
            analysis.functions.insert(k, f);
            match kind {
                0 => analysis.from_entries += 1,
                1 => analysis.from_calls += 1,
                _ => analysis.from_pointers += 1,
            }
        }
    };
    explore_all(&mut analysis, entries.to_vec(), 0);
    loop {
        let fresh: Vec<u32> =
            pointer_seeds(image, &analysis, data_words).into_iter().filter(|s| !analysis.functions.contains_key(s)).collect();
        if fresh.is_empty() {
            break;
        }
        explore_all(&mut analysis, fresh, 2);
    }
    analysis
}

/// Where each label's block ends: the next label, or just after an
/// instruction that always leaves, or a gap in the explored code.
pub fn blocks(f: &Function) -> Vec<(u32, Vec<(u32, Instr)>)> {
    let mut out = Vec::new();
    for &label in &f.labels {
        let mut body = Vec::new();
        let mut address = label;
        while let Some(instr) = f.instrs.get(&address) {
            body.push((address, *instr));
            address = address.wrapping_add(u32::from(instr.size));
            if instr.always_leaves() || f.labels.contains(&address) {
                break;
            }
        }
        out.push((label, body));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assembled by GNU as from (base 0):
    ///
    /// ```text
    /// 00 start:  push {r4, lr}        2c  b out
    /// 04         bl leaf              30 case2: svc #1
    /// 08         cmp r0, #2           34  mov r0, #12
    /// 0c         ldrls pc, [pc, r0, lsl #2]   38  b out
    /// 10         b default            3c default: mov r0, #0
    /// 14         .word case0..case2   40 out: pop {r4, pc}
    /// 20 case0:  mov r0, #10          44 leaf: mov r0, #1
    /// 24         b out                48  bx lr
    /// 28 case1:  mov r0, #11          4c  .word 0xdeadbeef
    /// ```
    const PROGRAM: [u32; 20] = [
        0xe92d4010, 0xeb00000e, 0xe3500002, 0x979ff100, 0xea000009, 0x00000020, 0x00000028, 0x00000030, 0xe3a0000a, 0xea000005,
        0xe3a0000b, 0xea000003, 0xef000001, 0xe3a0000c, 0xea000000, 0xe3a00000, 0xe8bd8010, 0xe3a00001, 0xe12fff1e, 0xdeadbeef,
    ];

    fn image() -> Vec<u8> {
        PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn finds_functions_labels_and_jump_table() {
        let bytes = image();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0 }, &[0]);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [0x00, 0x44]);
        let main = &analysis.functions[&0];
        let labels: Vec<u32> = main.labels.iter().copied().collect();
        // start, return point, table targets, after-svc, default, out
        assert_eq!(labels, [0x00, 0x08, 0x20, 0x28, 0x30, 0x34, 0x3c, 0x40]);
        // the table words themselves are not decoded as code
        assert!(!main.instrs.contains_key(&0x14) && !main.instrs.contains_key(&0x1c));
        let leaf = &analysis.functions[&0x44];
        assert!(!leaf.instrs.contains_key(&0x4c), "literal after bx lr must not be decoded");
    }

    #[test]
    fn blocks_end_at_labels_and_leaving_instructions() {
        let bytes = image();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0 }, &[0]);
        let blocks = blocks(&analysis.functions[&0]);
        let shape: Vec<(u32, usize)> = blocks.iter().map(|(l, b)| (*l, b.len())).collect();
        // 0x08 block: cmp, ldrls pc (conditional, falls through), b default
        assert_eq!(shape, [(0x00, 2), (0x08, 3), (0x20, 2), (0x28, 2), (0x30, 1), (0x34, 2), (0x3c, 1), (0x40, 1)]);
    }

    #[test]
    fn code_running_off_the_image_is_an_exit_not_a_panic() {
        let bytes = 0xe3a00001u32.to_le_bytes(); // mov r0, #1, then nothing
        let analysis = analyze(&Image { base: 0x1000, bytes: &bytes, code: 0x1000..0x1004 }, &[0x1000, 0x9000]);
        let f = &analysis.functions[&0x1000];
        assert_eq!(f.exits.iter().copied().collect::<Vec<_>>(), [0x1004]);
        assert!(!analysis.functions.contains_key(&0x9000), "entry outside the image is ignored");
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::*;

    #[test]
    fn packed_data_pointers_seed_code_at_every_byte_alignment_including_image_tail() {
        for offset in 0..4 {
            // An indirect entry and an otherwise unreachable ARM function.
            // The packed pointer is the final complete word in the image.
            let mut bytes = vec![0xa5; 20 + offset];
            for (at, word) in [0xe12fff10u32, 0xe3a00007, 0xe12fff1e].into_iter().enumerate() {
                bytes[at * 4..at * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            bytes[16 + offset..20 + offset].copy_from_slice(&0x8004u32.to_le_bytes());
            let image = Image { base: 0x8000, bytes: &bytes, code: 0x8000..0x800c };
            assert!(analyze(&image, &[0x8000]).functions.contains_key(&0x8004), "pointer offset {offset}");
            assert!(
                !analyze_with(&image, &[0x8000], false).functions.contains_key(&0x8004),
                "unrelocated module data must not create absolute-pointer seeds"
            );
        }
    }

    #[test]
    fn code_pointers_in_data_seed_functions() {
        // 0x00: bx r0 (only entry)   0x04: mov r0, #7 / 0x08: bx lr (reached by pointer)
        // 0x0c: .word 0x04 (a pointer)  0x10: .word 0x0b (odd: a possible Thumb
        // pointer, so a seed; in real code this kind of false seed only costs
        // unused output)  0x14: .word 0x0c (outside the code range)
        let words = [0xe12fff10u32, 0xe3a00007, 0xe12fff1e, 0x04, 0x0b, 0x0c];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x0c }, &[0]);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [0x00, 0x04, 0x0b]);
        assert_eq!((analysis.from_entries, analysis.from_pointers), (1, 2));
        assert!(analysis.functions[&0x0b].thumb);
    }

    #[test]
    fn arm_to_thumb_veneers_seed_thumb_functions() {
        // 0x00: bl 0x08 (to the veneer)  0x04: bx lr
        // 0x08: add r12, pc, #1  0x0c: bx r12  0x10: Thumb bx lr (0x4770)
        let words = [0xeb000000u32, 0xe12fff1e, 0xe28fc001, 0xe12fff1c, 0x0000_4770];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x14 }, &[0]);
        assert!(analysis.functions.contains_key(&0x11), "the Thumb function behind the veneer");
    }

    #[test]
    fn self_relative_tables_seed_functions() {
        // 0x00: bx r0 (entry)  0x04: mov r0, #7; 0x08: bx lr (reached only
        // through the table)  data at 0x0c: .word 0x04 - 0x0c (= -8)
        let words = [0xe12fff10u32, 0xe3a00007, 0xe12fff1e, 0xFFFF_FFF8];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x0c }, &[0]);
        assert!(analysis.functions.contains_key(&0x04));
    }

    #[test]
    fn offset_tables_relative_to_the_pc_are_followed() {
        // 0x00: add r5, pc, #4 (r5 = 0x0c)  0x04: ldr r6, [r5, r0, lsl #2]
        // 0x08: add pc, r6, r5   0x0c: .word 0x08, 0x0c (-> 0x14, 0x18)
        // 0x14: mov r0, #1; 0x18: bx lr (both reached only through the table)
        let words = [0xe28f5004u32, 0xe7956100, 0xe086f005, 0x08, 0x0c, 0xe3a00001, 0xe12fff1e];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x1c }, &[0]);
        let f = &analysis.functions[&key(0, false)];
        assert!(f.labels.contains(&0x14) && f.labels.contains(&0x18), "{:x?}", f.labels);
        assert!(!f.instrs.contains_key(&0x0c), "the table is not decoded as code");
    }

    #[test]
    fn jump_targets_computed_from_the_pc_are_followed() {
        // 0x00: add r4, pc, #4 (r4 = 0x0c)  0x04: add r4, r4, #8 (0x14)
        // 0x08: mov pc, r4   0x0c-0x10: data   0x14: bx lr
        let words = [0xe28f4004u32, 0xe2844008, 0xe1a0f004, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xe12fff1e];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x18 }, &[0]);
        let f = &analysis.functions[&key(0, false)];
        assert!(f.labels.contains(&0x14), "{:x?}", f.labels);
    }

    #[test]
    fn return_addresses_set_by_hand_are_followed() {
        // 0x00: add lr, pc, #0 (lr = 0x08), then add lr, lr, #4 (0x0c)
        // 0x08: data  0x0c: bx lr (reached on return)  0x10: mov pc, lr
        let words = [0xe28fe000u32, 0xe28ee004, 0xFFFF_FFFF, 0xe12fff1e, 0xe1a0f00e];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..0x14 }, &[0]);
        let f = &analysis.functions[&key(0, false)];
        assert!(f.labels.contains(&0x0c), "{:x?}", f.labels);
    }

    #[test]
    fn arm_blx_calls_a_thumb_function() {
        // 0x00: blx 0x08 (ARM, H=0)   0x04: bx lr
        // 0x08: Thumb movs r0, #1 (0x2001); bx lr (0x4770)
        let words = [0xfa000000u32, 0xe12fff1e, 0x4770_2001];
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let analysis = analyze(&Image { base: 0, bytes: &bytes, code: 0..12 }, &[0]);
        assert_eq!(analysis.functions.keys().copied().collect::<Vec<_>>(), [0x00, 0x09]);
        let thumb = &analysis.functions[&0x09];
        assert!(thumb.thumb);
        assert_eq!(thumb.instrs.keys().copied().collect::<Vec<_>>(), [0x08, 0x0a]);
    }
}
