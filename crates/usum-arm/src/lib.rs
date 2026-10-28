//! Decoding of ARM-state and Thumb-state instructions (ARMv6K, the ARM11
//! MPCore of the 3DS) into [`Instr`], the representation the recompiler
//! emits from. ARMv6K Thumb is Thumb-1 plus the BL/BLX pair: no Thumb-2.
//!
//! Encodings follow the ARM Architecture Reference Manual (DDI 0406C,
//! section A5 "ARM instruction set encoding"). Scope is milestone 3's
//! subset; everything else decodes to [`Op::Unsupported`], as does every
//! encoding the manual calls UNPREDICTABLE, so that generated code fails
//! loudly rather than guessing.
//!
//! Decoding takes the instruction's address, so that PC-relative values
//! are resolved here once: branch targets are absolute, and [`Instr::pc`]
//! is the value the PC reads as for this instruction.

mod classify;
mod media;
mod thumb;
mod vfp;

pub use classify::unsupported_class;
pub use media::{DualMul, HalfMul};
pub use thumb::decode_thumb;
pub use vfp::VfpOp;

/// Data-processing opcodes, in encoding order (bits 24:21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataOp {
    And,
    Eor,
    Sub,
    Rsb,
    Add,
    Adc,
    Sbc,
    Rsc,
    Tst,
    Teq,
    Cmp,
    Cmn,
    Orr,
    Mov,
    Bic,
    Mvn,
}

impl DataOp {
    const ALL: [DataOp; 16] = [
        DataOp::And,
        DataOp::Eor,
        DataOp::Sub,
        DataOp::Rsb,
        DataOp::Add,
        DataOp::Adc,
        DataOp::Sbc,
        DataOp::Rsc,
        DataOp::Tst,
        DataOp::Teq,
        DataOp::Cmp,
        DataOp::Cmn,
        DataOp::Orr,
        DataOp::Mov,
        DataOp::Bic,
        DataOp::Mvn,
    ];

    /// TST, TEQ, CMP, CMN: flags only, no destination.
    pub fn is_test(self) -> bool {
        matches!(self, DataOp::Tst | DataOp::Teq | DataOp::Cmp | DataOp::Cmn)
    }

    /// Logical operations take C from the shifter; arithmetic from the adder.
    pub fn is_logical(self) -> bool {
        matches!(
            self,
            DataOp::And | DataOp::Eor | DataOp::Tst | DataOp::Teq | DataOp::Orr | DataOp::Mov | DataOp::Bic | DataOp::Mvn
        )
    }

    /// MOV and MVN ignore Rn.
    pub fn uses_rn(self) -> bool {
        !matches!(self, DataOp::Mov | DataOp::Mvn)
    }
}

/// The flexible second operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operand {
    /// A rotated immediate; `carry` is the shifter carry-out, `None` when the
    /// rotation is 0 (carry unchanged).
    Imm { value: u32, carry: Option<bool> },
    /// Rm shifted by an encoded immediate (see `usum_guest::ops::shift_imm`).
    ImmShift { rm: u8, kind: u8, amount: u8 },
    /// Rm shifted by the bottom byte of Rs.
    RegShift { rm: u8, kind: u8, rs: u8 },
}

/// The offset of a single load or store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offset {
    Imm(u32),
    /// Rm shifted by an immediate (word/byte forms) or unshifted (LSL #0).
    Reg {
        rm: u8,
        kind: u8,
        amount: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Byte,
    Half,
    Word,
    /// LDRD/STRD: Rt and Rt+1.
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Data {
        op: DataOp,
        s: bool,
        rd: u8,
        rn: u8,
        operand: Operand,
    },
    /// MUL / MLA: rd = rm * rs (+ rn).
    Mul {
        accumulate: bool,
        s: bool,
        rd: u8,
        rn: u8,
        rm: u8,
        rs: u8,
    },
    /// UMULL / UMLAL / SMULL / SMLAL.
    MulLong {
        signed: bool,
        accumulate: bool,
        s: bool,
        lo: u8,
        hi: u8,
        rm: u8,
        rs: u8,
    },
    Clz {
        rd: u8,
        rm: u8,
    },
    Mem {
        load: bool,
        size: Size,
        signed: bool,
        rt: u8,
        rn: u8,
        offset: Offset,
        add: bool,
        pre: bool,
        writeback: bool,
    },
    /// LDM / STM. `before`: the P bit; `increment`: the U bit.
    Block {
        load: bool,
        rn: u8,
        list: u16,
        before: bool,
        increment: bool,
        writeback: bool,
    },
    /// B / BL / BLX (immediate), to an absolute target. `exchange`: the
    /// target is in the other instruction set (BLX).
    Branch {
        link: bool,
        target: u32,
        exchange: bool,
    },
    /// SXTB / SXTH / UXTB / UXTH: rd = extend(rm ror rotate).
    Extend {
        signed: bool,
        half: bool,
        rd: u8,
        rm: u8,
        rotate: u8,
    },
    /// REV / REV16 / REVSH.
    Rev {
        kind: RevKind,
        rd: u8,
        rm: u8,
    },
    /// BX / BLX (register).
    Bx {
        link: bool,
        rm: u8,
    },
    Mrs {
        rd: u8,
    },
    /// MSR CPSR_<mask>; `imm` or the value of `rm`.
    Msr {
        mask: u8,
        imm: Option<u32>,
        rm: u8,
    },
    Svc {
        imm: u32,
    },
    /// LDREX{B,H,D} / STREX{B,H,D}. `status` is STREX's Rd (0 stored, 1 not).
    Exclusive {
        load: bool,
        size: Size,
        status: u8,
        rt: u8,
        rn: u8,
    },
    ClearExclusive,
    /// MRC p15, 0, Rd, c13, c0, 3: the read-only thread ID register
    /// (TPIDRURO), which the 3DS kernel points at the thread's TLS.
    ReadTls {
        rd: u8,
    },
    /// VFP data processing; register numbers are in the spaces `op` names.
    VfpData {
        op: VfpOp,
        double: bool,
        d: u8,
        n: u8,
        m: u8,
    },
    /// VLDR / VSTR: one register at Rn ± offset (PC reads as Align(PC, 4)).
    VfpMem {
        load: bool,
        double: bool,
        d: u8,
        rn: u8,
        offset: u32,
        add: bool,
    },
    /// VLDM / VSTM (and VPUSH / VPOP): `count` consecutive registers.
    VfpBlock {
        load: bool,
        double: bool,
        first: u8,
        count: u8,
        rn: u8,
        increment: bool,
        writeback: bool,
    },
    /// VMOV Rt <-> Sn.
    VmovSingle {
        to_core: bool,
        rt: u8,
        s: u8,
    },
    /// VMOV Rt, Rt2 <-> Dm, or <-> Sm, Sm+1.
    VmovPair {
        to_core: bool,
        rt: u8,
        rt2: u8,
        double: bool,
        m: u8,
    },
    /// VMOV Rt <-> Dd[x]: one half of a double.
    VmovHalf {
        to_core: bool,
        rt: u8,
        d: u8,
        high: bool,
    },
    /// VMRS Rt, FPSCR; Rt = 15 means APSR_nzcv.
    Vmrs {
        rt: u8,
    },
    /// VMSR FPSCR, Rt.
    Vmsr {
        rt: u8,
    },
    /// {S,Q,SH,U,UQ,UH}{ADD16,ASX,SAX,SUB16,ADD8,SUB8}; numbering as in
    /// `usum_guest::ops::PAR_*`.
    Parallel {
        prefix: u8,
        op: u8,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    /// SEL: bytes from Rn where GE is set, else from Rm.
    Sel {
        rd: u8,
        rn: u8,
        rm: u8,
    },
    /// PKHBT (Rn bottom, Rm LSL shift top) / PKHTB (`top`: Rn top, Rm ASR
    /// shift bottom; shift 0 means ASR #32).
    Pack {
        top: bool,
        rd: u8,
        rn: u8,
        rm: u8,
        shift: u8,
    },
    /// SSAT/USAT (Rn shifted by LSL or ASR `amount`) and SSAT16/USAT16
    /// (`half`). `sat` is the encoded field: SSAT saturates to sat + 1 bits,
    /// USAT to sat bits.
    Saturate {
        signed: bool,
        half: bool,
        sat: u8,
        rd: u8,
        rn: u8,
        asr: bool,
        amount: u8,
    },
    /// The extends with an addend, and the dual-byte forms (SXTB16,
    /// UXTAB16...). `kind`: 0 byte, 1 halfword, 2 two bytes into halfwords.
    ExtendAdd {
        signed: bool,
        kind: u8,
        rd: u8,
        rn: Option<u8>,
        rm: u8,
        rotate: u8,
    },
    /// SMUAD/SMLAD/SMUSD/SMLSD/SMLALD/SMLSLD; `swap`: Rm's halves
    /// exchanged (the X suffix). For the long forms rd = RdHi, ra = RdLo.
    DualMul {
        kind: DualMul,
        swap: bool,
        rd: u8,
        ra: Option<u8>,
        rn: u8,
        rm: u8,
    },
    /// SMMUL/SMMLA/SMMLS: the top word of a 64-bit product (and accumulate).
    MostSignificant {
        subtract: bool,
        round: bool,
        rd: u8,
        ra: Option<u8>,
        rn: u8,
        rm: u8,
    },
    /// USAD8 / USADA8.
    Usad {
        rd: u8,
        ra: Option<u8>,
        rn: u8,
        rm: u8,
    },
    /// SMLAxy/SMULxy/SMLAWy/SMULWy/SMLALxy; for SMLAL rd = RdHi, ra = RdLo.
    HalfMul {
        kind: HalfMul,
        x: bool,
        y: bool,
        rd: u8,
        ra: u8,
        rn: u8,
        rm: u8,
    },
    /// QADD/QSUB/QDADD/QDSUB (op 0..=3).
    SatArith {
        op: u8,
        rd: u8,
        rn: u8,
        rm: u8,
    },
    /// UMAAL: RdHi:RdLo = Rn * Rm + RdHi + RdLo.
    Umaal {
        lo: u8,
        hi: u8,
        rn: u8,
        rm: u8,
    },
    /// Hints and barriers with no effect on a single core: NOP, YIELD, PLD,
    /// and the ARMv6 CP15 barriers (DMB c7,c10,5; DSB c7,c10,4; flush
    /// prefetch buffer c7,c5,4).
    Nop,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevKind {
    /// Reverse the four bytes.
    Word,
    /// Reverse the bytes in each halfword.
    Halves,
    /// Reverse the low halfword's bytes and sign-extend it.
    SignedHalf,
}

/// A decoded instruction. `cond` is 0xE (always) for unconditional ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instr {
    /// The encoding; for a Thumb BL/BLX pair, the first halfword in the low half.
    pub raw: u32,
    pub cond: u8,
    pub op: Op,
    pub thumb: bool,
    /// 2 or 4 bytes.
    pub size: u8,
    /// What reading the PC as an operand gives: address + 8 (ARM), + 4
    /// (Thumb), or Align(address + 4, 4) for Thumb literal loads and ADR.
    pub pc: u32,
}

pub const PC: u8 = 15;
pub const LR: u8 = 14;
pub const AL: u8 = 0xE;

fn bits(raw: u32, hi: u32, lo: u32) -> u32 {
    (raw >> lo) & ((1 << (hi - lo + 1)) - 1)
}

fn bit(raw: u32, n: u32) -> bool {
    raw >> n & 1 == 1
}

fn reg(raw: u32, lo: u32) -> u8 {
    bits(raw, lo + 3, lo) as u8
}

impl Instr {
    /// Instruction at `address` that does not fall through when executed:
    /// the next instruction is only reached through a label.
    pub fn always_leaves(&self) -> bool {
        self.cond == AL && self.writes_pc()
    }

    /// Whether executing it (if its condition passes) changes the PC
    /// other than by falling through. BL is a call and returns.
    pub fn writes_pc(&self) -> bool {
        match self.op {
            Op::Data { op, rd, .. } => rd == PC && !op.is_test(),
            Op::Mem { load, rt, .. } => load && rt == PC,
            Op::Block { load, list, .. } => load && list & (1 << PC) != 0,
            Op::Branch { link, .. } => !link,
            Op::Bx { link, .. } => !link,
            // an unsupported instruction of a class that cannot write the PC
            // still falls through, so discovery continues past it; anything
            // that might branch ends the path
            Op::Unsupported => classify::may_branch(self),
            _ => false,
        }
    }
}

/// Decodes the ARM instruction `raw` at `address`.
pub fn decode(raw: u32, address: u32) -> Instr {
    let cond = bits(raw, 31, 28) as u8;
    let pc = address.wrapping_add(8);
    let op = if cond == 0xF { unconditional(raw, pc) } else { decode_conditional(raw, pc) };
    Instr { raw, cond: if cond == 0xF { AL } else { cond }, op, thumb: false, size: 4, pc }
}

/// The cond = 0b1111 space: only BLX (immediate) for now.
fn unconditional(raw: u32, pc: u32) -> Op {
    if bits(raw, 27, 25) == 0b101 {
        // BLX imm: H (bit 24) gives the halfword; the target is Thumb
        let offset = (((bits(raw, 23, 0) << 8) as i32) >> 6) as u32 | (bits(raw, 24, 24) << 1);
        return Op::Branch { link: true, target: pc.wrapping_add(offset), exchange: true };
    }
    // PLD [Rn, #imm] / [Rn, Rm, shift]: a cache hint
    if raw & 0x0D70_F000 == 0x0550_F000 {
        return Op::Nop;
    }
    if raw == 0xF57F_F01F {
        return Op::ClearExclusive;
    }
    Op::Unsupported
}

fn decode_conditional(raw: u32, pc: u32) -> Op {
    match bits(raw, 27, 25) {
        0b000 if bit(raw, 7) && bit(raw, 4) => multiply_or_extra_load(raw),
        0b000 if bits(raw, 24, 23) == 0b10 && !bit(raw, 20) => miscellaneous(raw),
        0b000 => data_processing(raw),
        0b001 if bits(raw, 24, 23) == 0b10 && !bit(raw, 20) => msr_immediate_or_hint(raw),
        0b001 => data_processing(raw),
        0b010 => load_store(raw),
        0b011 if !bit(raw, 4) => load_store(raw),
        0b011 => media::media(raw),
        0b100 => block(raw),
        0b101 => {
            let offset = ((bits(raw, 23, 0) << 8) as i32) >> 6;
            Op::Branch { link: bit(raw, 24), target: pc.wrapping_add(offset as u32), exchange: false }
        }
        0b111 if bit(raw, 24) => Op::Svc { imm: bits(raw, 23, 0) },
        0b111 if bit(raw, 4) && bits(raw, 11, 8) == 15 => cp15(raw),
        0b111 if bits(raw, 11, 9) == 0b101 && bit(raw, 4) => vfp::register_transfer(raw),
        0b111 if bits(raw, 11, 9) == 0b101 => vfp::data_processing(raw),
        0b110 if bits(raw, 11, 9) == 0b101 && bits(raw, 27, 21) == 0b110_0010 => vfp::transfer64(raw),
        0b110 if bits(raw, 11, 9) == 0b101 => vfp::load_store(raw),
        // media instructions, coprocessor (VFP) and the rest: not yet
        _ => Op::Unsupported,
    }
}

fn data_processing(raw: u32) -> Op {
    let op = DataOp::ALL[bits(raw, 24, 21) as usize];
    let s = bit(raw, 20);
    let (rd, rn) = (reg(raw, 12), reg(raw, 16));
    let operand = if bit(raw, 25) {
        let rotation = bits(raw, 11, 8) * 2;
        let value = bits(raw, 7, 0).rotate_right(rotation);
        Operand::Imm { value, carry: (rotation != 0).then_some(value >> 31 == 1) }
    } else if bit(raw, 4) {
        let operand = Operand::RegShift { rm: reg(raw, 0), kind: bits(raw, 6, 5) as u8, rs: reg(raw, 8) };
        // PC in any position of a register-shifted form is UNPREDICTABLE
        if [rd, rn, reg(raw, 0), reg(raw, 8)].contains(&PC) {
            return Op::Unsupported;
        }
        operand
    } else {
        Operand::ImmShift { rm: reg(raw, 0), kind: bits(raw, 6, 5) as u8, amount: bits(raw, 11, 7) as u8 }
    };
    // Writing the PC with S set is an exception return, not for user mode
    if rd == PC && s && !op.is_test() {
        return Op::Unsupported;
    }
    Op::Data { op, s: s || op.is_test(), rd, rn, operand }
}

/// The CP15 accesses user code makes: the TLS register and barriers.
fn cp15(raw: u32) -> Op {
    let (opc1, load, crn, rt, opc2, crm) =
        (bits(raw, 23, 21), bit(raw, 20), bits(raw, 19, 16), reg(raw, 12), bits(raw, 7, 5), bits(raw, 3, 0));
    match (opc1, load, crn, crm, opc2) {
        (0, true, 13, 0, 3) if rt != PC => Op::ReadTls { rd: rt },
        (0, false, 7, 10, 4 | 5) | (0, false, 7, 5, 4) => Op::Nop,
        _ => Op::Unsupported,
    }
}

/// LDREX/STREX and their byte, halfword and doubleword forms (ARMv6K).
fn exclusive(raw: u32) -> Op {
    let (load, rn, rd, rt) = (bit(raw, 20), reg(raw, 16), reg(raw, 12), reg(raw, 0));
    if bits(raw, 11, 8) != 0xF {
        return Op::Unsupported;
    }
    let size = [Size::Word, Size::Double, Size::Byte, Size::Half][bits(raw, 22, 21) as usize];
    if load {
        // LDREX Rt is in bits 15:12; bits 3:0 are 1111
        if rt != PC || rd == PC || rn == PC || (size == Size::Double && (rd % 2 == 1 || rd == LR)) {
            return Op::Unsupported;
        }
        return Op::Exclusive { load, size, status: 0, rt: rd, rn };
    }
    let double_clash = size == Size::Double && (rt % 2 == 1 || rt == LR || rd == rt + 1);
    if [rd, rt, rn].contains(&PC) || rd == rn || rd == rt || double_clash {
        return Op::Unsupported;
    }
    Op::Exclusive { load, size, status: rd, rt, rn }
}

fn multiply_or_extra_load(raw: u32) -> Op {
    if bits(raw, 6, 5) != 0 {
        return extra_load_store(raw);
    }
    if raw & 0x0F80_00F0 == 0x0180_0090 {
        return exclusive(raw);
    }
    if bits(raw, 27, 24) != 0 {
        return Op::Unsupported; // SWP
    }
    let s = bit(raw, 20);
    let (rm, rs) = (reg(raw, 0), reg(raw, 8));
    match bits(raw, 23, 21) {
        a @ (0b000 | 0b001) => {
            let (rd, rn) = (reg(raw, 16), reg(raw, 12));
            if [rd, rm, rs].contains(&PC) || (a == 1 && rn == PC) {
                return Op::Unsupported;
            }
            Op::Mul { accumulate: a == 1, s, rd, rn, rm, rs }
        }
        long @ 0b100..=0b111 => {
            let (lo, hi) = (reg(raw, 12), reg(raw, 16));
            if [lo, hi, rm, rs].contains(&PC) || lo == hi {
                return Op::Unsupported;
            }
            Op::MulLong { signed: long & 0b010 != 0, accumulate: long & 1 != 0, s, lo, hi, rm, rs }
        }
        0b010 => media::umaal(raw),
        _ => Op::Unsupported, // MLS (v6T2) and reserved encodings
    }
}

fn extra_load_store(raw: u32) -> Op {
    let (pre, add, writeback, load) = (bit(raw, 24), bit(raw, 23), bit(raw, 21), bit(raw, 20));
    let (rn, rt) = (reg(raw, 16), reg(raw, 12));
    let offset = if bit(raw, 22) {
        Offset::Imm((bits(raw, 11, 8) << 4) | bits(raw, 3, 0))
    } else {
        if bits(raw, 11, 8) != 0 || reg(raw, 0) == PC {
            return Op::Unsupported;
        }
        Offset::Reg { rm: reg(raw, 0), kind: 0, amount: 0 }
    };
    let (size, signed, load) = match (load, bits(raw, 6, 5)) {
        (true, 0b01) => (Size::Half, false, true),
        (true, 0b10) => (Size::Byte, true, true),
        (true, 0b11) => (Size::Half, true, true),
        (false, 0b01) => (Size::Half, false, false),
        (false, 0b10) => (Size::Double, false, true),
        (false, 0b11) => (Size::Double, false, false),
        _ => return Op::Unsupported,
    };
    let wb = writeback || !pre;
    // post-indexed with W set is not a valid form for these encodings
    if !pre && writeback {
        return Op::Unsupported;
    }
    if size == Size::Double {
        if rt % 2 == 1 || rt == LR || (wb && (rn == PC || rn == rt || rn == rt + 1)) {
            return Op::Unsupported;
        }
        if let Offset::Reg { rm, .. } = offset {
            if rm == rt || rm == rt + 1 {
                return Op::Unsupported;
            }
        }
    } else if rt == PC || (wb && (rn == PC || rn == rt)) {
        return Op::Unsupported;
    }
    Op::Mem { load, size, signed, rt, rn, offset, add, pre, writeback: wb }
}

fn miscellaneous(raw: u32) -> Op {
    if bit(raw, 7) && !bit(raw, 4) {
        return media::halfword_multiply(raw);
    }
    if raw & 0x0F90_00F0 == 0x0100_0050 {
        return media::saturating(raw);
    }
    match raw & 0x0FF0_00F0 {
        // MRS Rd, CPSR (R = 0)
        0x0100_0000 if bits(raw, 19, 16) == 0xF && bits(raw, 11, 0) == 0 && reg(raw, 12) != PC => Op::Mrs { rd: reg(raw, 12) },
        // MSR CPSR_<mask>, Rm
        0x0120_0000 if bits(raw, 15, 4) == 0xF00 && reg(raw, 0) != PC => {
            Op::Msr { mask: bits(raw, 19, 16) as u8, imm: None, rm: reg(raw, 0) }
        }
        0x0120_0010 if bits(raw, 19, 8) == 0xFFF => match reg(raw, 0) {
            PC => Op::Unsupported,
            rm => Op::Bx { link: false, rm },
        },
        0x0120_0030 if bits(raw, 19, 8) == 0xFFF => match reg(raw, 0) {
            PC => Op::Unsupported,
            rm => Op::Bx { link: true, rm },
        },
        0x0160_0010 if bits(raw, 19, 16) == 0xF && bits(raw, 11, 8) == 0xF => match (reg(raw, 12), reg(raw, 0)) {
            (PC, _) | (_, PC) => Op::Unsupported,
            (rd, rm) => Op::Clz { rd, rm },
        },
        _ => Op::Unsupported,
    }
}

fn msr_immediate_or_hint(raw: u32) -> Op {
    let mask = bits(raw, 19, 16) as u8;
    if bit(raw, 22) || bits(raw, 15, 12) != 0xF {
        return Op::Unsupported; // SPSR, or not a valid MSR
    }
    if bit(raw, 21) && mask == 0 {
        // hints: NOP, YIELD (a scheduling hint, a no-op here); WFE/WFI/SEV are not
        return match bits(raw, 7, 0) {
            0 | 1 => Op::Nop,
            _ => Op::Unsupported,
        };
    }
    if !bit(raw, 21) {
        return Op::Unsupported; // MOVW/MOVT space (ARMv6T2, not on ARM11)
    }
    let value = bits(raw, 7, 0).rotate_right(bits(raw, 11, 8) * 2);
    Op::Msr { mask, imm: Some(value), rm: 0 }
}

fn load_store(raw: u32) -> Op {
    let (pre, add, byte, writeback, load) = (bit(raw, 24), bit(raw, 23), bit(raw, 22), bit(raw, 21), bit(raw, 20));
    let (rn, rt) = (reg(raw, 16), reg(raw, 12));
    let offset = if bit(raw, 25) {
        let rm = reg(raw, 0);
        if rm == PC {
            return Op::Unsupported;
        }
        Offset::Reg { rm, kind: bits(raw, 6, 5) as u8, amount: bits(raw, 11, 7) as u8 }
    } else {
        Offset::Imm(bits(raw, 11, 0))
    };
    // post-indexed with W set is LDRT/STRT: the same as LDR/STR in user mode
    let wb = writeback || !pre;
    if wb && (rn == PC || rn == rt) {
        return Op::Unsupported;
    }
    if byte && rt == PC {
        return Op::Unsupported;
    }
    let size = if byte { Size::Byte } else { Size::Word };
    Op::Mem { load, size, signed: false, rt, rn, offset, add, pre, writeback: wb }
}

fn block(raw: u32) -> Op {
    let (before, increment, user, writeback, load) = (bit(raw, 24), bit(raw, 23), bit(raw, 22), bit(raw, 21), bit(raw, 20));
    let (rn, list) = (reg(raw, 16), bits(raw, 15, 0) as u16);
    if user || rn == PC || list == 0 {
        return Op::Unsupported;
    }
    let in_list = list & (1 << rn) != 0;
    // LDM with writeback and the base in the list, and STM with writeback
    // and the base in the list but not lowest, are UNPREDICTABLE
    let lowest = list.trailing_zeros() as u8 == rn;
    if writeback && in_list && (load || !lowest) {
        return Op::Unsupported;
    }
    Op::Block { load, rn, list, before, increment, writeback }
}

#[cfg(test)]
mod tests;
