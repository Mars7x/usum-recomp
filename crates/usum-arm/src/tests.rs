//! Decoder tests. Encodings come from GNU as 2.42 (`arm-none-eabi-as
//! -mcpu=mpcore`), an independent encoder, with the source line beside
//! each; the expected decodings are written from the ARM ARM. Semantic
//! correctness is tested separately, against QEMU (crate usum-difftest).

use super::*;
use crate::VfpOp;

fn op(raw: u32) -> Op {
    decode(raw, 0).op
}

fn data(op: DataOp, s: bool, rd: u8, rn: u8, operand: Operand) -> Op {
    Op::Data { op, s, rd, rn, operand }
}

#[test]
fn data_processing() {
    // adds r1, r2, #0xFF000000: imm8 0xFF rotated by 8, carry = bit 31
    assert_eq!(op(0xe29214ff), data(DataOp::Add, true, 1, 2, Operand::Imm { value: 0xFF00_0000, carry: Some(true) }));
    // mov r0, #255: no rotation, carry unchanged
    assert_eq!(op(0xe3a000ff), data(DataOp::Mov, false, 0, 0, Operand::Imm { value: 255, carry: None }));
    // subne r3, r4, r5, lsl #3
    let i = decode(0x10443185, 0);
    assert_eq!(i.cond, 0x1);
    assert_eq!(i.op, data(DataOp::Sub, false, 3, 4, Operand::ImmShift { rm: 5, kind: 0, amount: 3 }));
    // rsbs r6, r7, r8, lsr #32: encoded as LSR #0
    assert_eq!(op(0xe0776028), data(DataOp::Rsb, true, 6, 7, Operand::ImmShift { rm: 8, kind: 1, amount: 0 }));
    // orr r9, r10, r11, asr r12
    assert_eq!(op(0xe18a9c5b), data(DataOp::Orr, false, 9, 10, Operand::RegShift { rm: 11, kind: 2, rs: 12 }));
    // movs r0, r1, rrx: ROR #0
    assert_eq!(op(0xe1b00061), data(DataOp::Mov, true, 0, 0, Operand::ImmShift { rm: 1, kind: 3, amount: 0 }));
    // cmp r2, #4: always sets flags
    assert_eq!(op(0xe3520004), data(DataOp::Cmp, true, 0, 2, Operand::Imm { value: 4, carry: None }));
    // add r0, pc, #8 (adr): PC as Rn is allowed
    assert_eq!(op(0xe28f0008), data(DataOp::Add, false, 0, PC, Operand::Imm { value: 8, carry: None }));
    // mov pc, lr: a branch
    let i = decode(0xe1a0f00e, 0);
    assert!(i.writes_pc() && i.always_leaves());
}

#[test]
fn multiplies_and_clz() {
    assert_eq!(op(0xe0010392), Op::Mul { accumulate: false, s: false, rd: 1, rn: 0, rm: 2, rs: 3 });
    assert_eq!(op(0xe0347695), Op::Mul { accumulate: true, s: true, rd: 4, rn: 7, rm: 5, rs: 6 });
    assert_eq!(op(0xe0810392), Op::MulLong { signed: false, accumulate: false, s: false, lo: 0, hi: 1, rm: 2, rs: 3 });
    assert_eq!(op(0xe0f54796), Op::MulLong { signed: true, accumulate: true, s: true, lo: 4, hi: 5, rm: 6, rs: 7 });
    assert_eq!(op(0xe16f8f19), Op::Clz { rd: 8, rm: 9 });
}

#[allow(clippy::too_many_arguments)]
fn mem(load: bool, size: Size, signed: bool, rt: u8, rn: u8, offset: Offset, add: bool, pre: bool, writeback: bool) -> Op {
    Op::Mem { load, size, signed, rt, rn, offset, add, pre, writeback }
}

#[test]
fn loads_and_stores() {
    use Size::*;
    assert_eq!(op(0xe5910004), mem(true, Word, false, 0, 1, Offset::Imm(4), true, true, false));
    // ldrb r2, [r3, #-1]!
    assert_eq!(op(0xe5732001), mem(true, Byte, false, 2, 3, Offset::Imm(1), false, true, true));
    // str r4, [r5], #8: post-indexed always writes back
    assert_eq!(op(0xe4854008), mem(false, Word, false, 4, 5, Offset::Imm(8), true, false, true));
    assert_eq!(op(0xe7976108), mem(true, Word, false, 6, 7, Offset::Reg { rm: 8, kind: 0, amount: 2 }, true, true, false));
    // ldr pc, [sp], #4: a return
    assert_eq!(op(0xe49df004), mem(true, Word, false, PC, 13, Offset::Imm(4), true, false, true));
    assert!(decode(0xe49df004, 0).always_leaves());
    assert_eq!(op(0xe1d100b2), mem(true, Half, false, 0, 1, Offset::Imm(2), true, true, false));
    assert_eq!(op(0xe19320d4), mem(true, Byte, true, 2, 3, Offset::Reg { rm: 4, kind: 0, amount: 0 }, true, true, false));
    assert_eq!(op(0xe16650b6), mem(false, Half, false, 5, 6, Offset::Imm(6), false, true, true));
    assert_eq!(op(0xe1c200d8), mem(true, Double, false, 0, 2, Offset::Imm(8), true, true, false));
}

#[test]
fn block_transfers() {
    // push {r4-r11, lr} = stmdb sp!
    assert_eq!(op(0xe92d4ff0), Op::Block { load: false, rn: 13, list: 0x4FF0, before: true, increment: false, writeback: true });
    let pop = decode(0xe8bd8ff0, 0);
    assert_eq!(pop.op, Op::Block { load: true, rn: 13, list: 0x8FF0, before: false, increment: true, writeback: true });
    assert!(pop.always_leaves());
    assert_eq!(op(0xe9900006), Op::Block { load: true, rn: 0, list: 0b110, before: true, increment: true, writeback: false });
    assert_eq!(op(0xe8230030), Op::Block { load: false, rn: 3, list: 0x30, before: false, increment: false, writeback: true });
}

#[test]
fn branches_and_system() {
    // b . at 0x6c lands on itself
    assert_eq!(decode(0xeafffffe, 0x6c).op, Op::Branch { link: false, target: 0x6c, exchange: false });
    // bl .+8 at 0x70: offset 0 means address + 8
    assert_eq!(decode(0xeb000000, 0x70).op, Op::Branch { link: true, target: 0x78, exchange: false });
    // blx .+0x100 at 0x10 (cond 1111): a call into Thumb
    assert_eq!(decode(0xfa00003e, 0x10).op, Op::Branch { link: true, target: 0x110, exchange: true });
    assert_eq!(op(0xe12fff1e), Op::Bx { link: false, rm: LR });
    assert_eq!(op(0xe12fff33), Op::Bx { link: true, rm: 3 });
    assert_eq!(op(0xe10f0000), Op::Mrs { rd: 0 });
    assert_eq!(op(0xe128f001), Op::Msr { mask: 0b1000, imm: None, rm: 1 });
    assert_eq!(op(0xe32cf20f), Op::Msr { mask: 0b1100, imm: Some(0xF000_0000), rm: 0 });
    assert_eq!(op(0xef123456), Op::Svc { imm: 0x123456 });
    assert_eq!(op(0xe320f000), Op::Nop);
}

#[test]
fn unpredictable_and_unknown_encodings_are_unsupported() {
    // ldr r1, [r1], #4 (as warns: destination same as write-back base)
    assert_eq!(op(0xe4911004), Op::Unsupported);
    // movs pc, lr (exception return)
    assert_eq!(op(0xe1b0f00e), Op::Unsupported);
    // ldm r0!, {r0, r1}
    assert_eq!(op(0xe8b00003), Op::Unsupported);
    // the rest of the unconditional space (here CPS), and VFPv3's
    // vmov.f32 s0, #1.0, which VFP11 lacks
    assert_eq!(op(0xf1020013), Op::Unsupported);
    assert_eq!(op(0xeeb70a00), Op::Unsupported);
    // orr r0, pc, r1, lsl r2: PC in a register-shifted form
    assert_eq!(op(0xe18f0211), Op::Unsupported);
    // unsupported VFP never branches, so discovery continues past it;
    // an unsupported form that might write the PC ends the block
    assert!(!decode(0xeeb70a00, 0).always_leaves(), "unsupported VFP falls through");
    assert!(decode(0xe1b0f00e, 0).always_leaves(), "movs pc, lr may branch");
}

/// Every 32-bit word decodes without panicking (sampled densely).
#[test]
fn decoding_is_total() {
    let mut x = 0x9E37_79B9u32;
    for _ in 0..2_000_000 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let _ = decode(x, 0);
        let _ = decode_thumb(x as u16, Some((x >> 16) as u16), 0);
    }
}

#[test]
fn media_extends_and_reversals() {
    // sxtb r0, r1, ror #8 / uxth r2, r3 / rev16 r4, r5 / revsh r6, r7
    assert_eq!(op(0xe6af0471), Op::Extend { signed: true, half: false, rd: 0, rm: 1, rotate: 8 });
    assert_eq!(op(0xe6ff2073), Op::Extend { signed: false, half: true, rd: 2, rm: 3, rotate: 0 });
    assert_eq!(op(0xe6bf4fb5), Op::Rev { kind: RevKind::Halves, rd: 4, rm: 5 });
    assert_eq!(op(0xe6ff6fb7), Op::Rev { kind: RevKind::SignedHalf, rd: 6, rm: 7 });
}

/// Thumb vectors, decoded at the address GNU as assembled them at.
fn t(hw: u16, address: u32) -> Instr {
    decode_thumb(hw, None, address)
}

#[test]
fn thumb_data_processing() {
    let d = |op, s, rd, rn, operand| Op::Data { op, s, rd, rn, operand };
    let sh = |rm, kind, amount| Operand::ImmShift { rm, kind, amount };
    let imm = |value| Operand::Imm { value, carry: None };
    assert_eq!(t(0x00d1, 0).op, d(DataOp::Mov, true, 1, 0, sh(2, 0, 3)), "lsls r1, r2, #3");
    assert_eq!(t(0x0823, 2).op, d(DataOp::Mov, true, 3, 0, sh(4, 1, 0)), "lsrs r3, r4, #32");
    assert_eq!(t(0x1888, 6).op, d(DataOp::Add, true, 0, 1, sh(2, 0, 0)), "adds r0, r1, r2");
    assert_eq!(t(0x1fe3, 8).op, d(DataOp::Sub, true, 3, 4, imm(7)), "subs r3, r4, #7");
    assert_eq!(t(0x25c8, 0xa).op, d(DataOp::Mov, true, 5, 0, imm(200)), "movs r5, #200");
    assert_eq!(t(0x2e0a, 0xc).op, d(DataOp::Cmp, true, 0, 6, imm(10)), "cmp r6, #10");
    assert_eq!(t(0x37ff, 0xe).op, d(DataOp::Add, true, 7, 7, imm(255)), "adds r7, #255");
    assert_eq!(t(0x4011, 0x12).op, d(DataOp::And, true, 1, 1, sh(2, 0, 0)), "ands r1, r2");
    assert_eq!(t(0x40a3, 0x14).op, d(DataOp::Mov, true, 3, 0, Operand::RegShift { rm: 3, kind: 0, rs: 4 }), "lsls r3, r4");
    assert_eq!(t(0x4247, 0x18).op, d(DataOp::Rsb, true, 7, 0, imm(0)), "negs r7, r0");
    assert_eq!(t(0x4351, 0x1a).op, Op::Mul { accumulate: false, s: true, rd: 1, rn: 0, rm: 2, rs: 1 }, "muls r1, r2, r1");
    assert_eq!(t(0x43e3, 0x1c).op, d(DataOp::Mvn, true, 3, 0, sh(4, 0, 0)), "mvns r3, r4");
    assert_eq!(t(0x4488, 0x20).op, d(DataOp::Add, false, 8, 8, sh(1, 0, 0)), "add r8, r1");
    assert_eq!(t(0x46d1, 0x22).op, d(DataOp::Mov, false, 9, 0, sh(10, 0, 0)), "mov r9, r10");
    assert_eq!(t(0x45d8, 0x24).op, d(DataOp::Cmp, true, 0, 8, sh(11, 0, 0)), "cmp r8, r11");
    assert!(t(0x46f7, 0x26).always_leaves(), "mov pc, lr");
}

#[test]
fn thumb_memory_and_pc_relative() {
    let m = |load, size, signed, rt, rn, offset| Op::Mem {
        load,
        size,
        signed,
        rt,
        rn,
        offset,
        add: true,
        pre: true,
        writeback: false,
    };
    let reg = |rm| Offset::Reg { rm, kind: 0, amount: 0 };
    // ldr r0, [pc, #16] at 0x2c: PC = Align(0x30, 4)
    let lit = t(0x4804, 0x2c);
    assert_eq!((lit.op, lit.pc), (m(true, Size::Word, false, 0, PC, Offset::Imm(16)), 0x30));
    assert_eq!(t(0x50d1, 0x2e).op, m(false, Size::Word, false, 1, 2, reg(3)));
    assert_eq!(t(0x5fac, 0x30).op, m(true, Size::Half, true, 4, 5, reg(6)));
    assert_eq!(t(0x6fd1, 0x32).op, m(true, Size::Word, false, 1, 2, Offset::Imm(124)));
    assert_eq!(t(0x77e3, 0x34).op, m(false, Size::Byte, false, 3, 4, Offset::Imm(31)));
    assert_eq!(t(0x8ff5, 0x36).op, m(true, Size::Half, false, 5, 6, Offset::Imm(62)));
    assert_eq!(t(0x97ff, 0x38).op, m(false, Size::Word, false, 7, 13, Offset::Imm(1020)));
    // adr r0, lit at 0x3a: Align(0x3e, 4) + 44 = 0x68, where as put lit
    let adr = t(0xa00b, 0x3a);
    assert_eq!(adr.op, Op::Data { op: DataOp::Add, s: false, rd: 0, rn: PC, operand: Operand::Imm { value: 44, carry: None } });
    assert_eq!(adr.pc + 44, 0x68);
    assert_eq!(
        t(0xb07f, 0x3e).op,
        Op::Data { op: DataOp::Add, s: false, rd: 13, rn: 13, operand: Operand::Imm { value: 508, carry: None } }
    );
    assert_eq!(
        t(0xb081, 0x40).op,
        Op::Data { op: DataOp::Sub, s: false, rd: 13, rn: 13, operand: Operand::Imm { value: 4, carry: None } }
    );
}

#[test]
fn thumb_misc_blocks_and_branches() {
    assert_eq!(t(0xb208, 0x42).op, Op::Extend { signed: true, half: true, rd: 0, rm: 1, rotate: 0 }, "sxth");
    assert_eq!(t(0xb2da, 0x44).op, Op::Extend { signed: false, half: false, rd: 2, rm: 3, rotate: 0 }, "uxtb");
    assert_eq!(t(0xba2c, 0x46).op, Op::Rev { kind: RevKind::Word, rd: 4, rm: 5 });
    assert_eq!(t(0xbafe, 0x48).op, Op::Rev { kind: RevKind::SignedHalf, rd: 6, rm: 7 });
    assert_eq!(
        t(0xb5f0, 0x4a).op,
        Op::Block { load: false, rn: 13, list: 0x40F0, before: true, increment: false, writeback: true }
    );
    let pop = t(0xbd01, 0x4c);
    assert_eq!(pop.op, Op::Block { load: true, rn: 13, list: 0x8001, before: false, increment: true, writeback: true });
    assert!(pop.always_leaves());
    assert_eq!(t(0xc806, 0x4e).op, Op::Block { load: true, rn: 0, list: 0b110, before: false, increment: true, writeback: true });
    // ldmia r3, {r1, r3}: base loaded, so no writeback
    assert_eq!(
        t(0xcb0a, 0x50).op,
        Op::Block { load: true, rn: 3, list: 0b1010, before: false, increment: true, writeback: false }
    );
    assert_eq!(t(0xc460, 0x52).op, Op::Block { load: false, rn: 4, list: 0x60, before: false, increment: true, writeback: true });
    let beq = t(0xd002, 0x54);
    assert_eq!((beq.cond, beq.op), (0x0, Op::Branch { link: false, target: 0x5c, exchange: false }));
    assert_eq!(t(0xdce0, 0x56).op, Op::Branch { link: false, target: 0x1a, exchange: false }, "bgt backwards");
    assert_eq!(t(0xdf2a, 0x58).op, Op::Svc { imm: 42 });
    assert_eq!(t(0xe030, 0x5a).op, Op::Branch { link: false, target: 0xbe, exchange: false });
    let bl = decode_thumb(0xf000, Some(0xfffe), 0x5c);
    assert_eq!((bl.op, bl.size), (Op::Branch { link: true, target: 0x105c, exchange: false }, 4));
    let blx = decode_thumb(0xf002, Some(0xe800), 0x60);
    assert_eq!(blx.op, Op::Branch { link: true, target: 0x2064, exchange: true });
    assert_eq!(t(0x4770, 0x28).op, Op::Bx { link: false, rm: LR });
    assert_eq!(t(0x4798, 0x2a).op, Op::Bx { link: true, rm: 3 });
    assert_eq!(decode_thumb(0xf000, None, 0).op, Op::Unsupported, "BL prefix at the end of code");
    assert_eq!(t(0xde00, 0).op, Op::Unsupported, "UDF");
}

/// Encodings as GNU objdump disassembles them (M4 analysis).
#[test]
fn system_and_exclusive() {
    assert_eq!(op(0xee1d4f70), Op::ReadTls { rd: 4 }, "mrc p15, 0, r4, c13, c0, 3");
    assert_eq!(op(0xee077fba), Op::Nop, "mcr p15, 0, r7, c7, c10, 5 (DMB)");
    assert_eq!(op(0xf5d0f020), Op::Nop, "pld [r0, #32]");
    assert_eq!(op(0xf57ff01f), Op::ClearExclusive);
    assert_eq!(op(0xe1901f9f), Op::Exclusive { load: true, size: Size::Word, status: 0, rt: 1, rn: 0 }, "ldrex r1, [r0]");
    assert_eq!(op(0xe180cf91), Op::Exclusive { load: false, size: Size::Word, status: 12, rt: 1, rn: 0 }, "strex r12, r1, [r0]");
    // strex r1, r1, [r0]: status register equal to the value register
    assert_eq!(op(0xe1801f91), Op::Unsupported);
    // other CP15 registers stay unsupported
    assert_eq!(op(0xee110f10), Op::Unsupported, "mrc p15, 0, r0, c1, c0, 0 (SCTLR)");
}

/// VFPv2 encodings from GNU as (`-mfpu=vfpv2`), with the source beside each.
#[test]
fn vfp_data_processing() {
    let d = |op, double, d, n, m| Op::VfpData { op, double, d, n, m };
    assert_eq!(op(0xee000a81), d(VfpOp::Mla, false, 0, 1, 2), "vmla.f32 s0, s1, s2");
    assert_eq!(op(0xee021b43), d(VfpOp::Mls, true, 1, 2, 3), "vmls.f64 d1, d2, d3");
    assert_eq!(op(0xee5ffa6e), d(VfpOp::Nmla, false, 31, 30, 29), "vnmla.f32 s31, s30, s29");
    assert_eq!(op(0xee122a83), d(VfpOp::Nmls, false, 4, 5, 6), "vnmls.f32 s4, s5, s6");
    assert_eq!(op(0xee200a20), d(VfpOp::Mul, false, 0, 0, 1), "vmul.f32 s0, s0, s1");
    assert_eq!(op(0xee200a48), d(VfpOp::Nmul, false, 0, 0, 16), "vnmul.f32 s0, s0, s16");
    assert_eq!(op(0xee3efb0d), d(VfpOp::Add, true, 15, 14, 13), "vadd.f64 d15, d14, d13");
    assert_eq!(op(0xee731ae5), d(VfpOp::Sub, false, 3, 7, 11), "vsub.f32 s3, s7, s11");
    assert_eq!(op(0xee810b02), d(VfpOp::Div, true, 0, 1, 2), "vdiv.f64 d0, d1, d2");
    assert_eq!(op(0xeeb08a40), d(VfpOp::Mov, false, 16, 0, 0), "vmov.f32 s16, s0");
    assert_eq!(op(0xeeb04bc5), d(VfpOp::Abs, true, 4, 0, 5), "vabs.f64 d4, d5");
    assert_eq!(op(0xeef10a60), d(VfpOp::Neg, false, 1, 0, 1), "vneg.f32 s1, s1");
    assert_eq!(op(0xeeb11ae1), d(VfpOp::Sqrt, false, 2, 0, 3), "vsqrt.f32 s2, s3");
    assert_eq!(op(0xeeb40a60), d(VfpOp::Cmp { signal: false, zero: false }, false, 0, 0, 1), "vcmp.f32 s0, s1");
    assert_eq!(op(0xeeb40bc1), d(VfpOp::Cmp { signal: true, zero: false }, true, 0, 0, 1), "vcmpe.f64 d0, d1");
    assert_eq!(op(0xeef52a40), d(VfpOp::Cmp { signal: false, zero: true }, false, 5, 0, 0), "vcmp.f32 s5, #0");
    assert_eq!(op(0xeeb72ae2), d(VfpOp::Precision, false, 2, 0, 5), "vcvt.f64.f32 d2, s5");
    assert_eq!(op(0xeef73bc3), d(VfpOp::Precision, true, 7, 0, 3), "vcvt.f32.f64 s7, d3");
    assert_eq!(op(0xeeb80ac0), d(VfpOp::FromInt { signed: true }, false, 0, 0, 0), "vcvt.f32.s32 s0, s0");
    assert_eq!(op(0xeeb81b64), d(VfpOp::FromInt { signed: false }, true, 1, 0, 9), "vcvt.f64.u32 d1, s9");
    assert_eq!(op(0xeebc0ac0), d(VfpOp::ToInt { signed: false, toward_zero: true }, false, 0, 0, 0), "vcvt.u32.f32 s0, s0");
    assert_eq!(op(0xeebd1bc4), d(VfpOp::ToInt { signed: true, toward_zero: true }, true, 2, 0, 4), "vcvt.s32.f64 s2, d4");
    assert_eq!(op(0xeefd1a42), d(VfpOp::ToInt { signed: true, toward_zero: false }, false, 3, 0, 4), "vcvtr.s32.f32 s3, s4");
    // vadd.f64 d16, d0, d1 needs D16 (VFPv3-D32): not on VFP11
    assert_eq!(op(0xee700b01), Op::Unsupported);
}

#[test]
fn vfp_memory_and_transfers() {
    assert_eq!(
        op(0xed908a01),
        Op::VfpMem { load: true, double: false, d: 16, rn: 0, offset: 4, add: true },
        "vldr s16, [r0, #4]"
    );
    assert_eq!(
        op(0xed0d8b04),
        Op::VfpMem { load: false, double: true, d: 8, rn: 13, offset: 16, add: false },
        "vstr d8, [sp, #-16]"
    );
    let lit = decode(0xed9f0b02, 0x100);
    assert_eq!((lit.op, lit.pc), (Op::VfpMem { load: true, double: true, d: 0, rn: PC, offset: 8, add: true }, 0x108));
    let block = |load, double, first, count, rn, increment, writeback| Op::VfpBlock {
        load,
        double,
        first,
        count,
        rn,
        increment,
        writeback,
    };
    assert_eq!(op(0xed2d8b04), block(false, true, 8, 2, 13, false, true), "vpush {{d8-d9}}");
    assert_eq!(op(0xecbd8b02), block(true, true, 8, 1, 13, true, true), "vpop {{d8}}");
    assert_eq!(op(0xecb10a04), block(true, false, 0, 4, 1, true, true), "vldmia r1!, {{s0-s3}}");
    assert_eq!(op(0xed220b06), block(false, true, 0, 3, 2, false, true), "vstmdb r2!, {{d0-d2}}");
    assert_eq!(op(0xec932a01), block(true, false, 4, 1, 3, true, false), "vldmia r3, {{s4}}");
    assert_eq!(op(0xee000a10), Op::VmovSingle { to_core: false, rt: 0, s: 0 }, "vmov s0, r0");
    assert_eq!(op(0xee1f1a90), Op::VmovSingle { to_core: true, rt: 1, s: 31 }, "vmov r1, s31");
    assert_eq!(op(0xec410b10), Op::VmovPair { to_core: false, rt: 0, rt2: 1, double: true, m: 0 }, "vmov d0, r0, r1");
    assert_eq!(op(0xec532b1f), Op::VmovPair { to_core: true, rt: 2, rt2: 3, double: true, m: 15 }, "vmov r2, r3, d15");
    assert_eq!(op(0xec476a12), Op::VmovPair { to_core: false, rt: 6, rt2: 7, double: false, m: 4 }, "vmov s4, s5, r6, r7");
    assert_eq!(op(0xee234b10), Op::VmovHalf { to_core: false, rt: 4, d: 3, high: true }, "vmov.32 d3[1], r4");
    assert_eq!(op(0xee135b10), Op::VmovHalf { to_core: true, rt: 5, d: 3, high: false }, "vmov.32 r5, d3[0]");
    assert_eq!(op(0xeef1fa10), Op::Vmrs { rt: PC }, "vmrs APSR_nzcv, fpscr");
    assert_eq!(op(0xeef10a10), Op::Vmrs { rt: 0 }, "vmrs r0, fpscr");
    assert_eq!(op(0xeee11a10), Op::Vmsr { rt: 1 }, "vmsr fpscr, r1");
    assert!(!decode(0xeef1fa10, 0).writes_pc(), "VMRS APSR_nzcv sets flags, it does not branch");
}

/// ARMv6 media and DSP encodings from GNU as (`-mcpu=mpcore`).
#[test]
fn media_and_dsp() {
    use crate::{DualMul, HalfMul};
    let par = |prefix, op, rd, rn, rm| Op::Parallel { prefix, op, rd, rn, rm };
    assert_eq!(op(0xe6621ff3), par(4, 5, 1, 2, 3), "uqsub8 r1, r2, r3");
    assert_eq!(op(0xe6766f9c), par(5, 4, 6, 6, 12), "uhadd8 r6, r6, r12");
    assert_eq!(op(0xe6110f12), par(0, 0, 0, 1, 2), "sadd16 r0, r1, r2");
    assert_eq!(op(0xe6254f36), par(1, 1, 4, 5, 6), "qasx r4, r5, r6");
    assert_eq!(op(0xe6387f59), par(2, 2, 7, 8, 9), "shsax r7, r8, r9");
    assert_eq!(op(0xe65baf7c), par(3, 3, 10, 11, 12), "usub16 r10, r11, r12");
    assert_eq!(op(0xe6816fb6), Op::Sel { rd: 6, rn: 1, rm: 6 }, "sel r6, r1, r6");
    assert_eq!(op(0xe6810412), Op::Pack { top: false, rd: 0, rn: 1, rm: 2, shift: 8 }, "pkhbt r0, r1, r2, lsl #8");
    assert_eq!(op(0xe6843855), Op::Pack { top: true, rd: 3, rn: 4, rm: 5, shift: 16 }, "pkhtb asr #16");
    assert_eq!(op(0xe6843055), Op::Pack { top: true, rd: 3, rn: 4, rm: 5, shift: 0 }, "pkhtb asr #32");
    let sat = |signed, half, sat, rd, rn, asr, amount| Op::Saturate { signed, half, sat, rd, rn, asr, amount };
    assert_eq!(op(0xe6a70211), sat(true, false, 7, 0, 1, false, 4), "ssat r0, #8, r1, lsl #4");
    assert_eq!(op(0xe6e72153), sat(false, false, 7, 2, 3, true, 2), "usat r2, #7, r3, asr #2");
    assert_eq!(op(0xe6ab4f35), sat(true, true, 11, 4, 5, false, 0), "ssat16 r4, #12, r5");
    assert_eq!(op(0xe6e36f37), sat(false, true, 3, 6, 7, false, 0), "usat16 r6, #3, r7");
    let ext = |signed, kind, rd, rn, rm, rotate| Op::ExtendAdd { signed, kind, rd, rn, rm, rotate };
    assert_eq!(op(0xe6cf1073), ext(false, 2, 1, None, 3, 0), "uxtb16 r1, r3");
    assert_eq!(op(0xe6832474), ext(true, 2, 2, Some(3), 4, 8), "sxtab16 r2, r3, r4, ror #8");
    assert_eq!(op(0xe6e65077), ext(false, 0, 5, Some(6), 7, 0), "uxtab r5, r6, r7");
    assert_eq!(op(0xe6b9887a), ext(true, 1, 8, Some(9), 10, 16), "sxtah r8, r9, r10, ror #16");
    let dual = |kind, swap, rd, ra, rn, rm| Op::DualMul { kind, swap, rd, ra, rn, rm };
    assert_eq!(op(0xe70cf612), dual(DualMul::Add, false, 12, None, 2, 6), "smuad r12, r2, r6");
    assert_eq!(op(0xe7095c1a), dual(DualMul::Add, false, 9, Some(5), 10, 12), "smlad r9, r10, r12, r5");
    assert_eq!(op(0xe70afc7a), dual(DualMul::Sub, true, 10, None, 10, 12), "smusdx r10, r10, r12");
    assert_eq!(op(0xe7410312), dual(DualMul::AddLong, false, 1, Some(0), 2, 3), "smlald r0, r1, r2, r3");
    assert_eq!(op(0xe754f635), Op::MostSignificant { subtract: false, round: true, rd: 4, ra: None, rn: 5, rm: 6 }, "smmulr");
    assert_eq!(op(0xe757a9d8), Op::MostSignificant { subtract: true, round: false, rd: 7, ra: Some(10), rn: 8, rm: 9 }, "smmls");
    assert_eq!(op(0xe7814312), Op::Usad { rd: 1, ra: Some(4), rn: 2, rm: 3 }, "usada8 r1, r2, r3, r4");
    let hm = |kind, x, y, rd, ra, rn, rm| Op::HalfMul { kind, x, y, rd, ra, rn, rm };
    assert_eq!(op(0xe1600081), hm(HalfMul::Mul, false, false, 0, 0, 1, 0), "smulbb r0, r1, r0");
    assert_eq!(op(0xe10254a3), hm(HalfMul::Mla, true, false, 2, 5, 3, 4), "smlatb r2, r3, r4, r5");
    assert_eq!(op(0xe12608e7), hm(HalfMul::Mulw, true, true, 6, 0, 7, 8), "smulwt r6, r7, r8");
    assert_eq!(op(0xe129cb8a), hm(HalfMul::Mlaw, false, false, 9, 12, 10, 11), "smlawb r9, r10, r11, r12");
    assert_eq!(op(0xe14103c2), hm(HalfMul::Mlal, false, true, 1, 0, 2, 3), "smlalbt r0, r1, r2, r3");
    assert_eq!(op(0xe1664055), Op::SatArith { op: 3, rd: 4, rn: 6, rm: 5 }, "qdsub r4, r5, r6");
    assert_eq!(op(0xe0410392), Op::Umaal { lo: 0, hi: 1, rn: 2, rm: 3 }, "umaal r0, r1, r2, r3");
}
