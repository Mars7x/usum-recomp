//! Instruction semantics shared by all generated code. Names and behavior
//! follow the ARM Architecture Reference Manual pseudocode (DDI 0406C,
//! which covers ARMv6K): `Shift_C`, `AddWithCarry`, `ConditionPassed`,
//! `BXWritePC`. Keeping them here keeps generated code small (R2) and
//! means each rule is tested once.

use crate::{Cpu, Fault, Memory};

/// Shift types as encoded in bits 6:5.
pub const LSL: u8 = 0;
pub const LSR: u8 = 1;
pub const ASR: u8 = 2;
pub const ROR: u8 = 3;

/// `ConditionPassed()` for a 4-bit condition code. 0b1111 is not a
/// condition; the decoder never emits it.
#[inline]
pub fn cond(cpu: &Cpu, code: u8) -> bool {
    let result = match code >> 1 {
        0 => cpu.z,
        1 => cpu.c,
        2 => cpu.n,
        3 => cpu.v,
        4 => cpu.c && !cpu.z,
        5 => cpu.n == cpu.v,
        6 => !cpu.z && cpu.n == cpu.v,
        _ => true,
    };
    if code & 1 == 1 && code != 0b1111 {
        !result
    } else {
        result
    }
}

/// An immediate shift as encoded (`DecodeImmShift` + `Shift_C`): amount 0
/// means LSL #0 (no shift), LSR #32, ASR #32, or RRX for ROR.
#[inline]
pub fn shift_imm(value: u32, kind: u8, amount: u8, carry: bool) -> (u32, bool) {
    match (kind, amount) {
        (LSL, 0) => (value, carry),
        (LSL, n) => (value << n, (value >> (32 - n)) & 1 == 1),
        (LSR, 0) => (0, value >> 31 == 1),
        (LSR, n) => (value >> n, (value >> (n - 1)) & 1 == 1),
        (ASR, 0) => (((value as i32) >> 31) as u32, value >> 31 == 1),
        (ASR, n) => (((value as i32) >> n) as u32, (value >> (n - 1)) & 1 == 1),
        (_, 0) => ((u32::from(carry) << 31) | (value >> 1), value & 1 == 1),
        (_, n) => (value.rotate_right(u32::from(n)), (value >> (n - 1)) & 1 == 1),
    }
}

/// A shift by the bottom byte of a register (`Shift_C` with the amount
/// not reduced): 0 leaves value and carry alone, 32 and above saturate.
#[inline]
pub fn shift_reg(value: u32, kind: u8, amount: u32, carry: bool) -> (u32, bool) {
    let amount = amount & 0xFF;
    if amount == 0 {
        return (value, carry);
    }
    match kind {
        LSL if amount < 32 => (value << amount, (value >> (32 - amount)) & 1 == 1),
        LSL if amount == 32 => (0, value & 1 == 1),
        LSL => (0, false),
        LSR if amount < 32 => (value >> amount, (value >> (amount - 1)) & 1 == 1),
        LSR if amount == 32 => (0, value >> 31 == 1),
        LSR => (0, false),
        ASR if amount < 32 => (((value as i32) >> amount) as u32, (value >> (amount - 1)) & 1 == 1),
        ASR => (((value as i32) >> 31) as u32, value >> 31 == 1),
        _ => {
            let n = amount & 31;
            if n == 0 {
                (value, value >> 31 == 1)
            } else {
                (value.rotate_right(n), (value >> (n - 1)) & 1 == 1)
            }
        }
    }
}

/// `AddWithCarry(x, y, carry_in)` → (result, carry_out, overflow).
#[inline]
pub fn add_with_carry(x: u32, y: u32, carry: bool) -> (u32, bool, bool) {
    let unsigned = u64::from(x) + u64::from(y) + u64::from(carry);
    let signed = i64::from(x as i32) + i64::from(y as i32) + i64::from(carry);
    let result = unsigned as u32;
    (result, unsigned >> 32 != 0, i64::from(result as i32) != signed)
}

/// Sets N and Z from a result.
#[inline]
pub fn nz(cpu: &mut Cpu, value: u32) {
    cpu.n = value >> 31 == 1;
    cpu.z = value == 0;
}

/// Sets N, Z, C and V from an `add_with_carry` result, returning it.
#[inline]
pub fn nzcv(cpu: &mut Cpu, (value, c, v): (u32, bool, bool)) -> u32 {
    nz(cpu, value);
    cpu.c = c;
    cpu.v = v;
    value
}

/// The CPSR as user-mode `MRS` reads it: NZCVQ, GE, the T bit and USR mode
/// (0x10). Interrupt masks and other bits read as 0 here; tests mask them.
#[inline]
pub fn cpsr(cpu: &Cpu) -> u32 {
    (u32::from(cpu.n) << 31)
        | (u32::from(cpu.z) << 30)
        | (u32::from(cpu.c) << 29)
        | (u32::from(cpu.v) << 28)
        | (u32::from(cpu.q) << 27)
        | (u32::from(cpu.ge & 0xF) << 16)
        | (u32::from(cpu.thumb) << 5)
        | 0x10
}

/// User-mode `MSR CPSR_<fields>`: only the flags byte (mask bit 3) and GE
/// (part of the "s" byte, mask bit 2) are writable from user mode.
#[inline]
pub fn msr(cpu: &mut Cpu, value: u32, mask: u8) {
    if mask & 0b1000 != 0 {
        cpu.n = value >> 31 & 1 == 1;
        cpu.z = value >> 30 & 1 == 1;
        cpu.c = value >> 29 & 1 == 1;
        cpu.v = value >> 28 & 1 == 1;
        cpu.q = value >> 27 & 1 == 1;
    }
    if mask & 0b0100 != 0 {
        cpu.ge = (value >> 16 & 0xF) as u8;
    }
}

/// `BXWritePC`: interworking branch. Returns the target address with the
/// state bit cleared, and sets the Thumb state from bit 0.
#[inline]
pub fn bx_target(cpu: &mut Cpu, target: u32) -> u32 {
    cpu.thumb = target & 1 == 1;
    if cpu.thumb {
        target & !1
    } else {
        target & !3
    }
}

/// Word access for LDM/STM/LDRD/STRD, which require alignment even with
/// unaligned support enabled (ARMv6 with SCTLR.U = 1).
#[inline]
pub fn read_aligned(mem: &Memory, address: u32) -> Result<u32, Fault> {
    if address & 3 != 0 {
        return Err(Fault::Unaligned { address });
    }
    mem.read_u32(address)
}

#[inline]
pub fn write_aligned(mem: &mut Memory, address: u32, value: u32) -> Result<(), Fault> {
    if address & 3 != 0 {
        return Err(Fault::Unaligned { address });
    }
    mem.write_u32(address, value)
}

/// `address` if it is a multiple of `size` (a power of two), else an
/// alignment fault: exclusive accesses require natural alignment.
#[inline]
pub fn aligned(address: u32, size: u32) -> Result<u32, Fault> {
    if address & (size - 1) != 0 {
        return Err(Fault::Unaligned { address });
    }
    Ok(address)
}

/// Count leading zeros (`CLZ`).
#[inline]
pub fn clz(value: u32) -> u32 {
    value.leading_zeros()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_shift_special_cases() {
        assert_eq!(shift_imm(0x8000_0001, LSL, 0, true), (0x8000_0001, true));
        assert_eq!(shift_imm(0x8000_0001, LSL, 1, false), (2, true));
        assert_eq!(shift_imm(0x8000_0000, LSR, 0, false), (0, true), "LSR #32");
        assert_eq!(shift_imm(0x8000_0000, ASR, 0, false), (0xFFFF_FFFF, true), "ASR #32");
        assert_eq!(shift_imm(0x0000_0003, ROR, 0, true), (0x8000_0001, true), "RRX");
        assert_eq!(shift_imm(0x0000_0003, ROR, 1, false), (0x8000_0001, true));
    }

    #[test]
    fn register_shift_saturation() {
        assert_eq!(shift_reg(0xFFFF_FFFF, LSL, 0x100, false), (0xFFFF_FFFF, false), "only the low byte counts");
        assert_eq!(shift_reg(1, LSL, 32, false), (0, true));
        assert_eq!(shift_reg(1, LSL, 33, true), (0, false));
        assert_eq!(shift_reg(0x8000_0000, LSR, 32, false), (0, true));
        assert_eq!(shift_reg(0x8000_0000, ASR, 200, false), (0xFFFF_FFFF, true));
        assert_eq!(shift_reg(0x8000_0001, ROR, 32, false), (0x8000_0001, true));
        assert_eq!(shift_reg(0x8000_0001, ROR, 33, false), (0xC000_0000, true));
    }

    #[test]
    fn add_with_carry_flags() {
        assert_eq!(add_with_carry(0xFFFF_FFFF, 1, false), (0, true, false));
        assert_eq!(add_with_carry(0x7FFF_FFFF, 1, false), (0x8000_0000, false, true));
        // 5 - 3 as 5 + !3 + 1: carry set means no borrow
        assert_eq!(add_with_carry(5, !3, true), (2, true, false));
        assert_eq!(add_with_carry(3, !5, true), (0xFFFF_FFFE, false, false));
        assert_eq!(add_with_carry(0x8000_0000, !1, true), (0x7FFF_FFFF, true, true));
    }

    #[test]
    fn conditions() {
        let mut cpu = Cpu::default();
        assert!(cond(&cpu, 0xE) && cond(&cpu, 0x1) && !cond(&cpu, 0x0));
        cpu.z = true;
        assert!(cond(&cpu, 0x0) && !cond(&cpu, 0x8) && cond(&cpu, 0x9), "EQ, HI, LS");
        cpu.n = true;
        assert!(cond(&cpu, 0xB) && !cond(&cpu, 0xA) && cond(&cpu, 0xD), "LT, GE, LE");
    }

    #[test]
    fn apsr_roundtrip() {
        let mut cpu = Cpu::default();
        msr(&mut cpu, 0xF80F_0000, 0b1100);
        assert_eq!(cpsr(&cpu), 0xF80F_0010);
        msr(&mut cpu, 0, 0b0100);
        assert_eq!(cpsr(&cpu), 0xF800_0010, "GE cleared, flags kept");
    }
}

// ---- ARMv6 media and DSP instructions -----------------------------------
// Pseudocode references are ARM ARM DDI 0406C section A8 entries.

/// Parallel add/subtract prefixes, in encoding order (bits 22:20 minus the
/// two undefined values): S, Q, SH, U, UQ, UH.
pub const PAR_S: u8 = 0;
pub const PAR_Q: u8 = 1;
pub const PAR_SH: u8 = 2;
pub const PAR_U: u8 = 3;
pub const PAR_UQ: u8 = 4;
pub const PAR_UH: u8 = 5;
/// Parallel operations: ADD16, ASX, SAX, SUB16, ADD8, SUB8.
pub const PAR_ADD16: u8 = 0;
pub const PAR_ASX: u8 = 1;
pub const PAR_SAX: u8 = 2;
pub const PAR_SUB16: u8 = 3;
pub const PAR_ADD8: u8 = 4;
pub const PAR_SUB8: u8 = 5;

/// The 36 parallel add/subtract instructions ({S,Q,SH,U,UQ,UH}{ADD16,
/// ASX,SAX,SUB16,ADD8,SUB8}). Only the plain S and U forms set GE.
pub fn parallel(cpu: &mut Cpu, prefix: u8, op: u8, n: u32, m: u32) -> u32 {
    let width = if op < PAR_ADD8 { 16 } else { 8 };
    let lanes = 32 / width;
    let mask = (1u32 << width) - 1;
    let signed = matches!(prefix, PAR_S | PAR_Q | PAR_SH);
    let lane = |x: u32, i: u32| {
        let v = (x >> (i * width)) & mask;
        if signed {
            i64::from((v << (32 - width)) as i32 >> (32 - width))
        } else {
            i64::from(v)
        }
    };
    let (mut result, mut ge) = (0u32, 0u8);
    for i in 0..lanes {
        // ASX: lane 0 = n.lo - m.hi, lane 1 = n.hi + m.lo; SAX the reverse
        let (a, b, add) = match op {
            PAR_ASX => (lane(n, i), lane(m, 1 - i), i == 1),
            PAR_SAX => (lane(n, i), lane(m, 1 - i), i == 0),
            PAR_ADD16 | PAR_ADD8 => (lane(n, i), lane(m, i), true),
            _ => (lane(n, i), lane(m, i), false),
        };
        let r = if add { a + b } else { a - b };
        let half = 1i64 << (width - 1);
        let value = match prefix {
            PAR_Q => r.clamp(-half, half - 1),
            PAR_UQ => r.clamp(0, i64::from(mask)),
            PAR_SH | PAR_UH => r >> 1,
            _ => r,
        };
        result |= (value as u32 & mask) << (i * width);
        let set = match prefix {
            PAR_S => r >= 0,
            PAR_U if add => r > i64::from(mask),
            PAR_U => r >= 0,
            _ => false,
        };
        if set {
            ge |= if width == 16 { 0b11 << (2 * i) } else { 1 << i };
        }
    }
    if matches!(prefix, PAR_S | PAR_U) {
        cpu.ge = ge;
    }
    result
}

/// SEL: byte i from n where GE[i] is set, else from m.
#[inline]
pub fn sel(cpu: &Cpu, n: u32, m: u32) -> u32 {
    let mask = (0..4).filter(|i| cpu.ge >> i & 1 == 1).fold(0u32, |acc, i| acc | (0xFF << (8 * i)));
    (n & mask) | (m & !mask)
}

/// Saturates `value` to a signed `bits`-bit range (1..=32), setting Q.
pub fn signed_sat(cpu: &mut Cpu, value: i64, bits: u32) -> u32 {
    let max = (1i64 << (bits - 1)) - 1;
    let r = value.clamp(-max - 1, max);
    if r != value {
        cpu.q = true;
    }
    r as u32
}

/// Saturates `value` to an unsigned `bits`-bit range (0..=31), setting Q.
pub fn unsigned_sat(cpu: &mut Cpu, value: i64, bits: u32) -> u32 {
    let max = (1i64 << bits) - 1;
    let r = value.clamp(0, max);
    if r != value {
        cpu.q = true;
    }
    r as u32
}

/// SSAT16 (`signed`) / USAT16: each halfword saturated to `bits` bits.
pub fn sat16(cpu: &mut Cpu, x: u32, bits: u32, signed: bool) -> u32 {
    let lo = i64::from(x as i16);
    let hi = i64::from((x >> 16) as i16);
    let (lo, hi) = if signed {
        (signed_sat(cpu, lo, bits), signed_sat(cpu, hi, bits))
    } else {
        (unsigned_sat(cpu, lo, bits), unsigned_sat(cpu, hi, bits))
    };
    (lo & 0xFFFF) | (hi << 16)
}

/// The signed halfword of `x` selected by `top`.
#[inline]
pub fn half(x: u32, top: bool) -> i64 {
    i64::from(if top { (x >> 16) as i16 } else { x as i16 })
}

/// A signed 32-bit accumulate that sets Q on overflow (SMLAxy, SMLAWy,
/// SMLAD, SMLSD, SMUAD).
pub fn add_q(cpu: &mut Cpu, a: i64, b: i64) -> u32 {
    let r = a + b;
    if r != i64::from(r as i32) {
        cpu.q = true;
    }
    r as u32
}

/// QADD / QSUB / QDADD / QDSUB (op 0..=3): Rd = sat(Rm ± [sat(2 * ] Rn [)]).
pub fn qarith(cpu: &mut Cpu, op: u8, m: u32, n: u32) -> u32 {
    let n = if op >= 2 { i64::from(signed_sat(cpu, 2 * i64::from(n as i32), 32) as i32) } else { i64::from(n as i32) };
    let m = i64::from(m as i32);
    let r = if op.is_multiple_of(2) { m + n } else { m - n };
    signed_sat(cpu, r, 32)
}

/// USAD8: the sum of absolute byte differences.
pub fn usad8(n: u32, m: u32) -> u32 {
    (0..4).map(|i| ((n >> (8 * i)) as u8).abs_diff((m >> (8 * i)) as u8) as u32).sum()
}

#[cfg(test)]
mod media_tests {
    use super::*;

    #[test]
    fn parallel_ge_and_saturation() {
        let mut cpu = Cpu::default();
        // uadd8: 0xFF + 0x01 carries in byte 0 only
        assert_eq!(parallel(&mut cpu, PAR_U, PAR_ADD8, 0x0000_00FF, 0x0000_0001), 0);
        assert_eq!(cpu.ge, 0b0001);
        // uqsub8 saturates at 0 and leaves GE alone
        assert_eq!(parallel(&mut cpu, PAR_UQ, PAR_SUB8, 0x0510_0000, 0x1001_0000), 0x000F_0000);
        assert_eq!(cpu.ge, 0b0001);
        // ssub16 sets GE per halfword where the difference is >= 0
        assert_eq!(parallel(&mut cpu, PAR_S, PAR_SUB16, 0x0001_0005, 0x0002_0003), 0xFFFF_0002);
        assert_eq!(cpu.ge, 0b0011);
        // qadd16 clamps to 0x7FFF; uhadd8 halves without wrapping
        assert_eq!(parallel(&mut cpu, PAR_Q, PAR_ADD16, 0x7000_0001, 0x7000_0001), 0x7FFF_0002);
        assert_eq!(parallel(&mut cpu, PAR_UH, PAR_ADD8, 0x0000_00FF, 0x0000_00FF), 0xFF);
        cpu.ge = 0b0101;
        assert_eq!(sel(&cpu, 0x1122_3344, 0xAABB_CCDD), 0xAA22_CC44);
    }

    #[test]
    fn saturation_sets_q() {
        let mut cpu = Cpu::default();
        assert_eq!(signed_sat(&mut cpu, 300, 8), 127);
        assert!(cpu.q);
        let mut cpu = Cpu::default();
        assert_eq!(unsigned_sat(&mut cpu, -5, 8), 0);
        assert!(cpu.q);
        let mut cpu = Cpu::default();
        assert_eq!(qarith(&mut cpu, 0, 0x7FFF_FFFF, 1), 0x7FFF_FFFF);
        assert!(cpu.q);
    }
}
