//! VFPv2 decoding (the ARM11 MPCore's VFP11): coprocessors 10 (single
//! precision) and 11 (double precision). Encodings follow ARM ARM DDI
//! 0406C A7 ("Advanced SIMD and Floating-point instruction encoding"),
//! restricted to VFPv2: 32 single / 16 double registers (D bit must be 0
//! for doubles), no VMOV immediate, no fixed-point or half-precision
//! conversions (those are VFPv3).
//!
//! Register numbers in [`Op`]s are indices in the space the instruction
//! names: Sn for single, Dn for double; conversions mix the two (see
//! [`VfpOp`]).

use crate::{bit, bits, reg, Op, PC};

/// Data-processing operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfpOp {
    /// d = d + n*m (two roundings, not fused)
    Mla,
    /// d = d - n*m
    Mls,
    /// d = -d - n*m
    Nmla,
    /// d = -d + n*m
    Nmls,
    Mul,
    /// d = -(n*m)
    Nmul,
    Add,
    Sub,
    Div,
    /// The register copies and sign operations: bit operations, no
    /// flushing or NaN processing.
    Mov,
    Abs,
    Neg,
    Sqrt,
    /// VCMP{E}: sets FPSCR.NZCV. `zero`: compare with +0. `signal`: VCMPE
    /// (quiet NaNs are invalid too; matters only for exception flags).
    Cmp {
        signal: bool,
        zero: bool,
    },
    /// VCVT between precisions. `double` names the *source*: d is single
    /// when converting from double, and double otherwise.
    Precision,
    /// VCVT from a 32-bit integer in Sm to Sd/Dd.
    FromInt {
        signed: bool,
    },
    /// VCVT{R} from Sm/Dm to a 32-bit integer in Sd. `toward_zero`: VCVT
    /// (C cast semantics); otherwise FPSCR's rounding mode (VCVTR).
    ToInt {
        signed: bool,
        toward_zero: bool,
    },
}

impl VfpOp {
    /// Operations that become short-vector operations when FPSCR.LEN > 0.
    pub fn vectorizable(self) -> bool {
        !matches!(self, VfpOp::Cmp { .. } | VfpOp::Precision | VfpOp::FromInt { .. } | VfpOp::ToInt { .. })
    }
}

fn single(raw: u32, hi: u32, lo: u32, extra: u32) -> u8 {
    ((bits(raw, hi, lo) << 1) | bits(raw, extra, extra)) as u8
}

fn double(raw: u32, hi: u32, lo: u32, extra: u32) -> Option<u8> {
    // VFPv2 has D0-D15: the extra (top) bit must be clear
    (!bit(raw, extra)).then_some(bits(raw, hi, lo) as u8)
}

/// Sd/Dd, Sn/Dn and Sm/Dm in the precision `dbl` selects.
fn dnm(raw: u32, dbl: bool) -> Option<(u8, u8, u8)> {
    if dbl {
        Some((double(raw, 15, 12, 22)?, double(raw, 19, 16, 7)?, double(raw, 3, 0, 5)?))
    } else {
        Some((single(raw, 15, 12, 22), single(raw, 19, 16, 7), single(raw, 3, 0, 5)))
    }
}

/// VFP data processing (coprocessor 10/11, CDP form).
pub(crate) fn data_processing(raw: u32) -> Op {
    let dbl = bit(raw, 8);
    let op6 = bit(raw, 6);
    let opc1 = (bits(raw, 23, 23) << 2) | bits(raw, 21, 20);
    let data = |op: VfpOp| match dnm(raw, dbl) {
        Some((d, n, m)) => Op::VfpData { op, double: dbl, d, n, m },
        None => Op::Unsupported,
    };
    match (opc1, op6) {
        (0b000, false) => data(VfpOp::Mla),
        (0b000, true) => data(VfpOp::Mls),
        (0b001, false) => data(VfpOp::Nmls),
        (0b001, true) => data(VfpOp::Nmla),
        (0b010, false) => data(VfpOp::Mul),
        (0b010, true) => data(VfpOp::Nmul),
        (0b011, false) => data(VfpOp::Add),
        (0b011, true) => data(VfpOp::Sub),
        (0b100, false) => data(VfpOp::Div),
        (0b111, _) => other(raw, dbl),
        _ => Op::Unsupported, // fused multiply-add and the rest are VFPv4
    }
}

/// The "other" data-processing group (opc1 = 1x11), selected by opc2 (the
/// Vn field) and bits 7:6. Bit 6 must be 1 here; 0 is VMOV immediate (VFPv3).
fn other(raw: u32, dbl: bool) -> Op {
    if !bit(raw, 6) {
        return Op::Unsupported;
    }
    let opc2 = bits(raw, 19, 16);
    let top = bit(raw, 7);
    // bit 7 is part of the opcode here, not the N bit: only d and m
    let same = |op: VfpOp| {
        let (d, m) = if dbl {
            (double(raw, 15, 12, 22), double(raw, 3, 0, 5))
        } else {
            (Some(single(raw, 15, 12, 22)), Some(single(raw, 3, 0, 5)))
        };
        match (d, m) {
            (Some(d), Some(m)) => Op::VfpData { op, double: dbl, d, n: 0, m },
            _ => Op::Unsupported,
        }
    };
    match opc2 {
        0b0000 => same(if top { VfpOp::Abs } else { VfpOp::Mov }),
        0b0001 => same(if top { VfpOp::Sqrt } else { VfpOp::Neg }),
        0b0100 => same(VfpOp::Cmp { signal: top, zero: false }),
        // VCMP with zero requires Vm = 0
        0b0101 if bits(raw, 3, 0) == 0 && !bit(raw, 5) => same(VfpOp::Cmp { signal: top, zero: true }),
        // VCVT between precisions: the destination is in the other precision
        0b0111 if top => {
            let d = if dbl { Some(single(raw, 15, 12, 22)) } else { double(raw, 15, 12, 22) };
            let m = if dbl { double(raw, 3, 0, 5) } else { Some(single(raw, 3, 0, 5)) };
            match (d, m) {
                (Some(d), Some(m)) => Op::VfpData { op: VfpOp::Precision, double: dbl, d, n: 0, m },
                _ => Op::Unsupported,
            }
        }
        // from integer: Sm holds the integer, bit 7 = signed
        0b1000 => {
            let d = if dbl { double(raw, 15, 12, 22) } else { Some(single(raw, 15, 12, 22)) };
            match d {
                Some(d) => Op::VfpData { op: VfpOp::FromInt { signed: top }, double: dbl, d, n: 0, m: single(raw, 3, 0, 5) },
                None => Op::Unsupported,
            }
        }
        // to integer in Sd: bit 16 = signed, bit 7 = round toward zero
        0b1100 | 0b1101 => {
            let m = if dbl { double(raw, 3, 0, 5) } else { Some(single(raw, 3, 0, 5)) };
            match m {
                Some(m) => Op::VfpData {
                    op: VfpOp::ToInt { signed: bit(raw, 16), toward_zero: top },
                    double: dbl,
                    d: single(raw, 15, 12, 22),
                    n: 0,
                    m,
                },
                None => Op::Unsupported,
            }
        }
        _ => Op::Unsupported, // half precision, fixed point: VFPv3
    }
}

/// VLDR/VSTR/VLDM/VSTM (coprocessor 10/11, LDC/STC form), excluding the
/// 64-bit transfers (`transfer64`).
pub(crate) fn load_store(raw: u32) -> Op {
    let (pre, add, writeback, load) = (bit(raw, 24), bit(raw, 23), bit(raw, 21), bit(raw, 20));
    let dbl = bit(raw, 8);
    let rn = reg(raw, 16);
    let imm8 = bits(raw, 7, 0);
    let first = if dbl { double(raw, 15, 12, 22) } else { Some(single(raw, 15, 12, 22)) };
    let Some(first) = first else { return Op::Unsupported };
    if pre && !writeback {
        return Op::VfpMem { load, double: dbl, d: first, rn, offset: imm8 * 4, add };
    }
    // multiple: IA (P=0, U=1, W optional) or DB (P=1, U=0, W=1)
    if pre == add || (writeback && rn == PC) {
        return Op::Unsupported;
    }
    let (count, limit) = if dbl { (imm8 / 2, 16) } else { (imm8, 32) };
    // an odd imm8 for doubles is FLDMX/FSTMX (implementation-defined format)
    if count == 0 || u32::from(first) + count > limit || (dbl && imm8 % 2 == 1) {
        return Op::Unsupported;
    }
    Op::VfpBlock { load, double: dbl, first, count: count as u8, rn, increment: add, writeback }
}

/// VMOV between two core registers and a double or two singles
/// (cond 1100 010x ... 101x 00M1 Vm).
pub(crate) fn transfer64(raw: u32) -> Op {
    let (to_core, rt2, rt) = (bit(raw, 20), reg(raw, 16), reg(raw, 12));
    let dbl = bit(raw, 8);
    if bits(raw, 7, 6) != 0 || !bit(raw, 4) || rt == PC || rt2 == PC || (to_core && rt == rt2) {
        return Op::Unsupported;
    }
    let m = if dbl { double(raw, 3, 0, 5) } else { Some(single(raw, 3, 0, 5)) };
    match m {
        Some(m) if dbl || m < 31 => Op::VmovPair { to_core, rt, rt2, double: dbl, m },
        _ => Op::Unsupported,
    }
}

/// Single-register transfers (coprocessor 10/11, MRC/MCR form): VMOV
/// Sn<->Rt, VMOV Dn[x]<->Rt, VMRS, VMSR.
pub(crate) fn register_transfer(raw: u32) -> Op {
    let (opc1, to_core, rt) = (bits(raw, 23, 21), bit(raw, 20), reg(raw, 12));
    if bits(raw, 6, 5) != 0 || bits(raw, 3, 0) != 0 {
        return Op::Unsupported;
    }
    match (bit(raw, 8), opc1) {
        (false, 0b000) if rt != PC => Op::VmovSingle { to_core, rt, s: single(raw, 19, 16, 7) },
        // VMRS/VMSR: only FPSCR (reg 0001) is accessible to user code here;
        // VMRS to R15 copies FPSCR.NZCV to the APSR
        (false, 0b111) if bits(raw, 19, 16) == 0b0001 => {
            if to_core {
                Op::Vmrs { rt }
            } else if rt != PC {
                Op::Vmsr { rt }
            } else {
                Op::Unsupported
            }
        }
        (true, 0b000 | 0b001) if rt != PC => match double(raw, 19, 16, 7) {
            Some(d) => Op::VmovHalf { to_core, rt, d, high: bit(raw, 21) },
            None => Op::Unsupported,
        },
        _ => Op::Unsupported,
    }
}
