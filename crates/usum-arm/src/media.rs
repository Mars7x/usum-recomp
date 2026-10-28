//! The ARMv6 media instructions (cond 011x xxxx ... xxx1 xxxx; ARM ARM DDI
//! 0406C A5.4) and the signed halfword multiplies and saturating
//! arithmetic of the miscellaneous space (A5.2.7, A5.2.12). PC in any
//! register position is UNPREDICTABLE for all of these and decodes to
//! `Unsupported`.

use crate::{bit, bits, reg, Op, RevKind, PC};

/// Signed multiply forms of the media space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualMul {
    /// SMUAD / SMLAD: lo*lo + hi*hi (+ Ra)
    Add,
    /// SMUSD / SMLSD: lo*lo - hi*hi (+ Ra)
    Sub,
    /// SMLALD: 64-bit RdHi:RdLo += lo*lo + hi*hi
    AddLong,
    /// SMLSLD
    SubLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalfMul {
    /// SMLAxy: Rd = Rn.x * Rm.y + Ra (Q on overflow)
    Mla,
    /// SMULxy
    Mul,
    /// SMLAWy: Rd = (Rn * Rm.y) >> 16 + Ra (Q on overflow)
    Mlaw,
    /// SMULWy
    Mulw,
    /// SMLALxy: RdHi:RdLo += Rn.x * Rm.y
    Mlal,
}

fn no_pc(regs: &[u8], op: Op) -> Op {
    if regs.contains(&PC) {
        Op::Unsupported
    } else {
        op
    }
}

/// The media space (bits 27:25 = 011, bit 4 = 1).
pub(crate) fn media(raw: u32) -> Op {
    let (op1, op2) = (bits(raw, 24, 20), bits(raw, 7, 5));
    let (rn, rd, rm) = (reg(raw, 16), reg(raw, 12), reg(raw, 0));
    match op1 >> 3 {
        0b00 => parallel(raw, op1, op2, rn, rd, rm),
        0b01 => pack_saturate_extend(raw, op1, op2, rn, rd, rm),
        0b10 => signed_multiply(raw, op1, op2),
        _ if op1 == 0b11000 && op2 == 0 => {
            let (rd, ra, rm, rn) = (reg(raw, 16), reg(raw, 12), reg(raw, 8), reg(raw, 0));
            let ra = (ra != PC).then_some(ra);
            no_pc(&[rd, rm, rn], Op::Usad { rd, ra, rn, rm })
        }
        _ => Op::Unsupported,
    }
}

fn parallel(raw: u32, op1: u32, op2: u32, rn: u8, rd: u8, rm: u8) -> Op {
    let prefix = match op1 & 0b111 {
        0b001 => 0,
        0b010 => 1,
        0b011 => 2,
        0b101 => 3,
        0b110 => 4,
        0b111 => 5,
        _ => return Op::Unsupported,
    };
    let op = match op2 {
        0b000..=0b100 => op2 as u8,
        0b111 => 5,
        _ => return Op::Unsupported,
    };
    if bits(raw, 11, 8) != 0xF {
        return Op::Unsupported;
    }
    no_pc(&[rn, rd, rm], Op::Parallel { prefix, op, rd, rn, rm })
}

fn pack_saturate_extend(raw: u32, op1: u32, op2: u32, rn: u8, rd: u8, rm: u8) -> Op {
    let rotate = 8 * bits(raw, 11, 10) as u8;
    let add = (rn != PC).then_some(rn);
    // extends must have bits 9:8 clear
    let extend = |signed: bool, kind: u8| {
        if bits(raw, 9, 8) != 0 {
            return Op::Unsupported;
        }
        // without an addend, the byte and halfword forms are the plain extends
        let op = match (add, kind) {
            (None, 0 | 1) => Op::Extend { signed, half: kind == 1, rd, rm, rotate },
            _ => Op::ExtendAdd { signed, kind, rd, rn: add, rm, rotate },
        };
        no_pc(&[rd, rm], op)
    };
    match (op1, op2) {
        (0b01000, o) if o & 1 == 0 => {
            let top = bit(raw, 6);
            no_pc(&[rn, rd, rm], Op::Pack { top, rd, rn, rm, shift: bits(raw, 11, 7) as u8 })
        }
        (0b01000, 0b011) => extend(true, 2),
        (0b01000, 0b101) if bits(raw, 11, 8) == 0xF => no_pc(&[rn, rd, rm], Op::Sel { rd, rn, rm }),
        (0b01010 | 0b01011 | 0b01110 | 0b01111, o) if o & 1 == 0 => {
            // SSAT/USAT: saturate position in 20:16, shift in 11:7 (bit 6: ASR)
            let signed = !bit(raw, 22);
            let sat = bits(raw, 20, 16) as u8;
            no_pc(
                &[rd, rm],
                Op::Saturate { signed, half: false, sat, rd, rn: rm, asr: bit(raw, 6), amount: bits(raw, 11, 7) as u8 },
            )
        }
        (0b01010 | 0b01110, 0b001) if bits(raw, 11, 8) == 0xF => {
            let signed = op1 == 0b01010;
            no_pc(&[rd, rm], Op::Saturate { signed, half: true, sat: bits(raw, 19, 16) as u8, rd, rn: rm, asr: false, amount: 0 })
        }
        (0b01010, 0b011) => extend(true, 0),
        (0b01011, 0b011) => extend(true, 1),
        (0b01100, 0b011) => extend(false, 2),
        (0b01110, 0b011) => extend(false, 0),
        (0b01111, 0b011) => extend(false, 1),
        (0b01011, 0b001) | (0b01011, 0b101) | (0b01111, 0b101) if bits(raw, 19, 16) == 0xF && bits(raw, 11, 8) == 0xF => {
            let kind = match (op1, op2) {
                (0b01011, 0b001) => RevKind::Word,
                (0b01011, _) => RevKind::Halves,
                _ => RevKind::SignedHalf,
            };
            no_pc(&[rd, rm], Op::Rev { kind, rd, rm })
        }
        _ => Op::Unsupported,
    }
}

fn signed_multiply(raw: u32, op1: u32, op2: u32) -> Op {
    let (rd, ra, rm, rn) = (reg(raw, 16), reg(raw, 12), reg(raw, 8), reg(raw, 0));
    let swap = bit(raw, 5);
    match (op1, op2 >> 1) {
        (0b10000, 0b00 | 0b01) => {
            let kind = if op2 >> 1 == 0 { DualMul::Add } else { DualMul::Sub };
            let ra = (ra != PC).then_some(ra);
            no_pc(&[rd, rm, rn], Op::DualMul { kind, swap, rd, ra, rn, rm })
        }
        (0b10100, 0b00 | 0b01) => {
            let kind = if op2 >> 1 == 0 { DualMul::AddLong } else { DualMul::SubLong };
            // rd = RdHi, ra = RdLo
            if rd == ra {
                return Op::Unsupported;
            }
            no_pc(&[rd, ra, rm, rn], Op::DualMul { kind, swap, rd, ra: Some(ra), rn, rm })
        }
        (0b10101, 0b00) => {
            let ra = (ra != PC).then_some(ra);
            no_pc(&[rd, rm, rn], Op::MostSignificant { subtract: false, round: swap, rd, ra, rn, rm })
        }
        (0b10101, 0b11) if ra != PC => {
            no_pc(&[rd, rm, rn], Op::MostSignificant { subtract: true, round: swap, rd, ra: Some(ra), rn, rm })
        }
        _ => Op::Unsupported,
    }
}

/// Signed halfword multiplies: cond 0001 0op0 Rd Ra Rm 1yx0 Rn.
pub(crate) fn halfword_multiply(raw: u32) -> Op {
    let (rd, ra, rm, rn) = (reg(raw, 16), reg(raw, 12), reg(raw, 8), reg(raw, 0));
    let (x, y) = (bit(raw, 5), bit(raw, 6));
    let (kind, uses_ra) = match bits(raw, 22, 21) {
        0b00 => (HalfMul::Mla, true),
        0b01 if x => (HalfMul::Mulw, false),
        0b01 => (HalfMul::Mlaw, true),
        0b10 => (HalfMul::Mlal, true),
        _ => (HalfMul::Mul, false),
    };
    if kind == HalfMul::Mlal && rd == ra {
        return Op::Unsupported;
    }
    let regs: &[u8] = if uses_ra { &[rd, ra, rm, rn] } else { &[rd, rm, rn] };
    no_pc(regs, Op::HalfMul { kind, x, y, rd, ra, rn, rm })
}

/// QADD/QSUB/QDADD/QDSUB: cond 0001 0op0 Rn Rd 0000 0101 Rm.
pub(crate) fn saturating(raw: u32) -> Op {
    if bits(raw, 11, 8) != 0 {
        return Op::Unsupported;
    }
    let (rn, rd, rm) = (reg(raw, 16), reg(raw, 12), reg(raw, 0));
    no_pc(&[rn, rd, rm], Op::SatArith { op: bits(raw, 22, 21) as u8, rd, rn, rm })
}

/// UMAAL: cond 0000 0100 RdHi RdLo Rm 1001 Rn.
pub(crate) fn umaal(raw: u32) -> Op {
    let (hi, lo, rm, rn) = (reg(raw, 16), reg(raw, 12), reg(raw, 8), reg(raw, 0));
    if hi == lo {
        return Op::Unsupported;
    }
    no_pc(&[hi, lo, rm, rn], Op::Umaal { lo, hi, rn, rm })
}
