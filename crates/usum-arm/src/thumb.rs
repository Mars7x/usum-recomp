//! Thumb (16-bit, ARMv6K) decoding onto the same [`Op`]s as ARM, wherever
//! the semantics are identical (ARM ARM DDI 0406C, A6.2, restricted to the
//! encodings ARMv6K has). Outside an IT block, which ARMv6K lacks, the
//! low-register data-processing forms always set flags.

use crate::{bits, DataOp, Instr, Offset, Op, Operand, RevKind, Size, AL, LR, PC};

const SP: u8 = 13;

fn bit16(hw: u16, n: u32) -> bool {
    hw >> n & 1 == 1
}

fn f(hw: u16, hi: u32, lo: u32) -> u32 {
    bits(u32::from(hw), hi, lo)
}

fn r3(hw: u16, lo: u32) -> u8 {
    f(hw, lo + 2, lo) as u8
}

fn plain(rm: u8) -> Operand {
    Operand::ImmShift { rm, kind: 0, amount: 0 }
}

fn imm(value: u32) -> Operand {
    Operand::Imm { value, carry: None }
}

fn data(op: DataOp, s: bool, rd: u8, rn: u8, operand: Operand) -> Op {
    Op::Data { op, s, rd, rn, operand }
}

fn mem(load: bool, size: Size, signed: bool, rt: u8, rn: u8, offset: Offset) -> Op {
    Op::Mem { load, size, signed, rt, rn, offset, add: true, pre: true, writeback: false }
}

/// Decodes the Thumb instruction at `address`. `second` is the following
/// halfword, needed only for the BL/BLX pair; pass None at the end of code.
pub fn decode_thumb(first: u16, second: Option<u16>, address: u32) -> Instr {
    let pc = address.wrapping_add(4);
    let aligned = pc & !3;
    let mut instr = Instr { raw: u32::from(first), cond: AL, op: Op::Unsupported, thumb: true, size: 2, pc };
    let hw = first;
    instr.op = match f(hw, 15, 11) {
        k @ 0b00000..=0b00010 => {
            data(DataOp::Mov, true, r3(hw, 0), 0, Operand::ImmShift { rm: r3(hw, 3), kind: k as u8, amount: f(hw, 10, 6) as u8 })
        }
        0b00011 => {
            let op = if bit16(hw, 9) { DataOp::Sub } else { DataOp::Add };
            let operand = if bit16(hw, 10) { imm(f(hw, 8, 6)) } else { plain(r3(hw, 6)) };
            data(op, true, r3(hw, 0), r3(hw, 3), operand)
        }
        0b00100 => data(DataOp::Mov, true, r3(hw, 8), 0, imm(f(hw, 7, 0))),
        0b00101 => data(DataOp::Cmp, true, 0, r3(hw, 8), imm(f(hw, 7, 0))),
        0b00110 => data(DataOp::Add, true, r3(hw, 8), r3(hw, 8), imm(f(hw, 7, 0))),
        0b00111 => data(DataOp::Sub, true, r3(hw, 8), r3(hw, 8), imm(f(hw, 7, 0))),
        0b01000 if !bit16(hw, 10) => data_processing(hw),
        0b01000 => special(hw),
        0b01001 => {
            instr.pc = aligned;
            mem(true, Size::Word, false, r3(hw, 8), PC, Offset::Imm(f(hw, 7, 0) * 4))
        }
        0b01010 | 0b01011 => {
            let (rt, rn, rm) = (r3(hw, 0), r3(hw, 3), Offset::Reg { rm: r3(hw, 6), kind: 0, amount: 0 });
            match f(hw, 11, 9) {
                0b000 => mem(false, Size::Word, false, rt, rn, rm),
                0b001 => mem(false, Size::Half, false, rt, rn, rm),
                0b010 => mem(false, Size::Byte, false, rt, rn, rm),
                0b011 => mem(true, Size::Byte, true, rt, rn, rm),
                0b100 => mem(true, Size::Word, false, rt, rn, rm),
                0b101 => mem(true, Size::Half, false, rt, rn, rm),
                0b110 => mem(true, Size::Byte, false, rt, rn, rm),
                _ => mem(true, Size::Half, true, rt, rn, rm),
            }
        }
        k @ (0b01100..=0b10001) => {
            let (load, (size, scale)) = (
                bit16(hw, 11),
                match k {
                    0b01100 | 0b01101 => (Size::Word, 4),
                    0b01110 | 0b01111 => (Size::Byte, 1),
                    _ => (Size::Half, 2),
                },
            );
            mem(load, size, false, r3(hw, 0), r3(hw, 3), Offset::Imm(f(hw, 10, 6) * scale))
        }
        0b10010 | 0b10011 => mem(bit16(hw, 11), Size::Word, false, r3(hw, 8), SP, Offset::Imm(f(hw, 7, 0) * 4)),
        0b10100 => {
            instr.pc = aligned;
            data(DataOp::Add, false, r3(hw, 8), PC, imm(f(hw, 7, 0) * 4))
        }
        0b10101 => data(DataOp::Add, false, r3(hw, 8), SP, imm(f(hw, 7, 0) * 4)),
        0b10110 | 0b10111 => miscellaneous(hw),
        0b11000 | 0b11001 => {
            let (load, rn, list) = (bit16(hw, 11), r3(hw, 8), f(hw, 7, 0) as u16);
            let in_list = list & (1 << rn) != 0;
            if list == 0 || (!load && in_list && list.trailing_zeros() as u8 != rn) {
                Op::Unsupported
            } else {
                // LDM writes back unless the base is loaded; STM always does
                Op::Block { load, rn, list, before: false, increment: true, writeback: !(load && in_list) }
            }
        }
        0b11010 | 0b11011 => match f(hw, 11, 8) {
            0b1110 => Op::Unsupported, // UDF
            0b1111 => Op::Svc { imm: f(hw, 7, 0) },
            cond => {
                instr.cond = cond as u8;
                let offset = (((f(hw, 7, 0) << 24) as i32) >> 23) as u32;
                Op::Branch { link: false, target: pc.wrapping_add(offset), exchange: false }
            }
        },
        0b11100 => {
            let offset = (((f(hw, 10, 0) << 21) as i32) >> 20) as u32;
            Op::Branch { link: false, target: pc.wrapping_add(offset), exchange: false }
        }
        0b11110 => match second {
            // BL / BLX prefix: the high part of the offset
            Some(low) if matches!(f(low, 15, 11), 0b11111 | 0b11101) => {
                instr.size = 4;
                instr.raw = u32::from(first) | (u32::from(low) << 16);
                let high = (((f(hw, 10, 0) << 21) as i32) >> 9) as u32;
                let target = pc.wrapping_add(high).wrapping_add(f(low, 10, 0) << 1);
                if f(low, 15, 11) == 0b11111 {
                    Op::Branch { link: true, target, exchange: false }
                } else if f(low, 0, 0) == 1 {
                    Op::Unsupported
                } else {
                    Op::Branch { link: true, target: target & !3, exchange: true }
                }
            }
            _ => Op::Unsupported,
        },
        _ => Op::Unsupported, // a lone BL/BLX suffix
    };
    instr
}

fn data_processing(hw: u16) -> Op {
    let (rdn, rm) = (r3(hw, 0), r3(hw, 3));
    let shift = |kind| data(DataOp::Mov, true, rdn, 0, Operand::RegShift { rm: rdn, kind, rs: rm });
    match f(hw, 9, 6) {
        0x0 => data(DataOp::And, true, rdn, rdn, plain(rm)),
        0x1 => data(DataOp::Eor, true, rdn, rdn, plain(rm)),
        0x2 => shift(0),
        0x3 => shift(1),
        0x4 => shift(2),
        0x5 => data(DataOp::Adc, true, rdn, rdn, plain(rm)),
        0x6 => data(DataOp::Sbc, true, rdn, rdn, plain(rm)),
        0x7 => shift(3),
        0x8 => data(DataOp::Tst, true, 0, rdn, plain(rm)),
        0x9 => data(DataOp::Rsb, true, rdn, rm, imm(0)),
        0xA => data(DataOp::Cmp, true, 0, rdn, plain(rm)),
        0xB => data(DataOp::Cmn, true, 0, rdn, plain(rm)),
        0xC => data(DataOp::Orr, true, rdn, rdn, plain(rm)),
        0xD => Op::Mul { accumulate: false, s: true, rd: rdn, rn: 0, rm, rs: rdn },
        0xE => data(DataOp::Bic, true, rdn, rdn, plain(rm)),
        _ => data(DataOp::Mvn, true, rdn, 0, plain(rm)),
    }
}

/// ADD/CMP/MOV with high registers, BX and BLX.
fn special(hw: u16) -> Op {
    let rdn = (f(hw, 7, 7) << 3) as u8 | r3(hw, 0);
    let rm = f(hw, 6, 3) as u8;
    match f(hw, 9, 8) {
        0b00 => data(DataOp::Add, false, rdn, rdn, plain(rm)),
        0b01 if rdn == PC || rm == PC || (rdn < 8 && rm < 8) => Op::Unsupported,
        0b01 => data(DataOp::Cmp, true, 0, rdn, plain(rm)),
        0b10 => data(DataOp::Mov, false, rdn, 0, plain(rm)),
        _ if f(hw, 2, 0) != 0 || (bit16(hw, 7) && rm == PC) => Op::Unsupported,
        _ => Op::Bx { link: bit16(hw, 7), rm },
    }
}

fn miscellaneous(hw: u16) -> Op {
    let (rd, rm) = (r3(hw, 0), r3(hw, 3));
    match f(hw, 11, 8) {
        0b0000 => {
            let op = if bit16(hw, 7) { DataOp::Sub } else { DataOp::Add };
            data(op, false, SP, SP, imm(f(hw, 6, 0) * 4))
        }
        0b0010 => {
            let k = f(hw, 7, 6);
            Op::Extend { signed: k < 2, half: k.is_multiple_of(2), rd, rm, rotate: 0 }
        }
        0b0100 | 0b0101 => {
            let list = f(hw, 7, 0) as u16 | if bit16(hw, 8) { 1 << LR } else { 0 };
            if list == 0 {
                return Op::Unsupported;
            }
            Op::Block { load: false, rn: SP, list, before: true, increment: false, writeback: true }
        }
        0b1100 | 0b1101 => {
            let list = f(hw, 7, 0) as u16 | if bit16(hw, 8) { 1 << PC } else { 0 };
            if list == 0 {
                return Op::Unsupported;
            }
            Op::Block { load: true, rn: SP, list, before: false, increment: true, writeback: true }
        }
        0b1010 => match f(hw, 7, 6) {
            0b00 => Op::Rev { kind: RevKind::Word, rd, rm },
            0b01 => Op::Rev { kind: RevKind::Halves, rd, rm },
            0b11 => Op::Rev { kind: RevKind::SignedHalf, rd, rm },
            _ => Op::Unsupported,
        },
        _ => Op::Unsupported, // CPS, SETEND, BKPT
    }
}
