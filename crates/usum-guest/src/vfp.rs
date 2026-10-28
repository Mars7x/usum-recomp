//! VFPv2 semantics (the ARM11 MPCore's VFP11), on raw register bits.
//!
//! Rules follow the ARM ARM pseudocode (DDI 0406C: FPProcessNaNs, FPAdd,
//! FPMul, FPRound, FPCompare, FPToFixed, FixedToFP, FPConvert) for what
//! this module supports:
//!
//! - NaN operands are handled before any Rust arithmetic, since Rust
//!   leaves NaN payloads unspecified: a signaling NaN wins over a quiet
//!   one, the first operand wins ties, the result is quieted, and FPSCR.DN
//!   replaces it with the default NaN. A NaN produced by an invalid
//!   operation (inf - inf, 0 * inf, sqrt(-1)) is the default NaN.
//! - FPSCR.FZ flushes subnormal inputs to signed zero, and results whose
//!   exact value is below the smallest normal number. For single
//!   precision the check uses an f64 intermediate, which holds the exact
//!   result of a multiply and is within one rounding of the exact result
//!   otherwise; for double precision it is made after rounding (a known,
//!   narrow difference at the boundary).
//! - Single-precision add, subtract, multiply, divide and square root are
//!   computed in f64 and rounded to f32. Rounding twice is harmless here
//!   because 53 >= 2 * 24 + 2.
//! - Arithmetic supports only round-to-nearest. Another FPSCR.RMode, or
//!   short vectors whose length or stride the code below rejects, make the
//!   slow path return a fault rather than a wrong value. Conversions
//!   support all four rounding modes.
//! - Cumulative exception flags (FPSCR bits 7, 4:0) are not maintained,
//!   and trap enables are stored but never trap. RESEARCH.md R-017.

use crate::{Cpu, Fault};

pub const FZ: u32 = 1 << 24;
pub const DN: u32 = 1 << 25;
const RMODE_SHIFT: u32 = 22;
const LEN_SHIFT: u32 = 16;
const STRIDE_SHIFT: u32 = 20;
/// FPSCR bits that select short vectors (LEN and STRIDE).
pub const VECTOR_BITS: u32 = (0x7 << LEN_SHIFT) | (0x3 << STRIDE_SHIFT);
/// FPSCR bits that send arithmetic to the slow path: vectors and RMode.
pub const SLOW_BITS: u32 = VECTOR_BITS | (0x3 << RMODE_SHIFT);
/// FPSCR bits a VMSR stores: NZCV, DN, FZ, RMode, STRIDE, LEN, the trap
/// enables and the cumulative flags.
pub const WRITABLE: u32 = 0xF3F7_9F9F;

const SIGN32: u32 = 0x8000_0000;
const QUIET32: u32 = 0x0040_0000;
pub const DEFAULT_NAN32: u32 = 0x7FC0_0000;
const SIGN64: u64 = 0x8000_0000_0000_0000;
const QUIET64: u64 = 0x0008_0000_0000_0000;
pub const DEFAULT_NAN64: u64 = 0x7FF8_0000_0000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Nearest,
    Up,
    Down,
    TowardZero,
}

fn rounding(cpu: &Cpu) -> Rounding {
    match (cpu.fpscr >> RMODE_SHIFT) & 3 {
        0 => Rounding::Nearest,
        1 => Rounding::Up,
        2 => Rounding::Down,
        _ => Rounding::TowardZero,
    }
}

// ---- register access ---------------------------------------------------

/// Dn: s(2n) is the low word, s(2n+1) the high word.
#[inline]
pub fn d(cpu: &Cpu, n: usize) -> u64 {
    u64::from(cpu.s[2 * n]) | (u64::from(cpu.s[2 * n + 1]) << 32)
}

#[inline]
pub fn set_d(cpu: &mut Cpu, n: usize, value: u64) {
    cpu.s[2 * n] = value as u32;
    cpu.s[2 * n + 1] = (value >> 32) as u32;
}

// ---- classification ----------------------------------------------------

fn is_nan32(x: u32) -> bool {
    x & !SIGN32 > 0x7F80_0000
}

fn is_snan32(x: u32) -> bool {
    is_nan32(x) && x & QUIET32 == 0
}

fn is_nan64(x: u64) -> bool {
    x & !SIGN64 > 0x7FF0_0000_0000_0000
}

fn is_snan64(x: u64) -> bool {
    is_nan64(x) && x & QUIET64 == 0
}

fn flush_in32(cpu: &Cpu, x: u32) -> u32 {
    if cpu.fpscr & FZ != 0 && x & 0x7F80_0000 == 0 {
        x & SIGN32
    } else {
        x
    }
}

fn flush_in64(cpu: &Cpu, x: u64) -> u64 {
    if cpu.fpscr & FZ != 0 && x & 0x7FF0_0000_0000_0000 == 0 {
        x & SIGN64
    } else {
        x
    }
}

/// FPProcessNaNs for two operands: the NaN result, if either is a NaN.
fn nans32(cpu: &Cpu, a: u32, b: u32) -> Option<u32> {
    let pick = [a, b].into_iter().find(|&x| is_snan32(x)).or_else(|| [a, b].into_iter().find(|&x| is_nan32(x)))?;
    Some(if cpu.fpscr & DN != 0 { DEFAULT_NAN32 } else { pick | QUIET32 })
}

fn nans64(cpu: &Cpu, a: u64, b: u64) -> Option<u64> {
    let pick = [a, b].into_iter().find(|&x| is_snan64(x)).or_else(|| [a, b].into_iter().find(|&x| is_nan64(x)))?;
    Some(if cpu.fpscr & DN != 0 { DEFAULT_NAN64 } else { pick | QUIET64 })
}

/// Rounds an f64 holding a single-precision result to f32 bits in `mode`,
/// applying FZ to tiny results. `value` must be exact for directed modes.
fn round32(cpu: &Cpu, value: f64, mode: Rounding) -> u32 {
    if value.is_nan() {
        return DEFAULT_NAN32;
    }
    if cpu.fpscr & FZ != 0 && value != 0.0 && value.abs() < f64::from(f32::MIN_POSITIVE) {
        return if value.is_sign_negative() { SIGN32 } else { 0 };
    }
    let nearest = value as f32; // ties to even
    let back = f64::from(nearest);
    let r = match mode {
        Rounding::Nearest => nearest,
        _ if back == value || nearest.is_nan() => nearest,
        Rounding::Up if back < value => next_up32(nearest),
        Rounding::Down if back > value => next_down32(nearest),
        Rounding::TowardZero if back.abs() > value.abs() => toward_zero32(nearest),
        _ => nearest,
    };
    // a directed rounding can step back into the subnormal range
    if cpu.fpscr & FZ != 0 && r != 0.0 && r.is_subnormal() {
        return r.to_bits() & SIGN32;
    }
    r.to_bits()
}

fn next_up32(x: f32) -> f32 {
    if x.is_infinite() && x > 0.0 {
        return x;
    }
    if x == 0.0 {
        return f32::from_bits(1);
    }
    let bits = x.to_bits();
    f32::from_bits(if x > 0.0 { bits + 1 } else { bits - 1 })
}

fn next_down32(x: f32) -> f32 {
    -next_up32(-x)
}

fn toward_zero32(x: f32) -> f32 {
    if x > 0.0 {
        next_down32(x)
    } else {
        next_up32(x)
    }
}

fn finish64(cpu: &Cpu, value: f64) -> u64 {
    if value.is_nan() {
        return DEFAULT_NAN64;
    }
    if cpu.fpscr & FZ != 0 && value != 0.0 && value.is_subnormal() {
        return value.to_bits() & SIGN64;
    }
    value.to_bits()
}

// ---- arithmetic ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
}

fn apply(op: Arith, a: f64, b: f64) -> f64 {
    match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
        Arith::Div => a / b,
    }
}

/// A two-operand single-precision operation in round-to-nearest.
#[inline]
pub fn arith32(cpu: &Cpu, op: Arith, a: u32, b: u32) -> u32 {
    let (a, b) = (flush_in32(cpu, a), flush_in32(cpu, b));
    if let Some(nan) = nans32(cpu, a, b) {
        return nan;
    }
    let value = apply(op, f64::from(f32::from_bits(a)), f64::from(f32::from_bits(b)));
    round32(cpu, value, Rounding::Nearest)
}

#[inline]
pub fn arith64(cpu: &Cpu, op: Arith, a: u64, b: u64) -> u64 {
    let (a, b) = (flush_in64(cpu, a), flush_in64(cpu, b));
    if let Some(nan) = nans64(cpu, a, b) {
        return nan;
    }
    finish64(cpu, apply(op, f64::from_bits(a), f64::from_bits(b)))
}

/// VMLA family: d ± n*m with the product rounded first.
/// `negate_product`: VMLS/VNMLA; `negate_d`: VNMLA/VNMLS.
#[inline]
pub fn mla32(cpu: &Cpu, d: u32, n: u32, m: u32, negate_product: bool, negate_d: bool) -> u32 {
    let product = arith32(cpu, Arith::Mul, n, m) ^ if negate_product { SIGN32 } else { 0 };
    let d = d ^ if negate_d { SIGN32 } else { 0 };
    arith32(cpu, Arith::Add, d, product)
}

#[inline]
pub fn mla64(cpu: &Cpu, d: u64, n: u64, m: u64, negate_product: bool, negate_d: bool) -> u64 {
    let product = arith64(cpu, Arith::Mul, n, m) ^ if negate_product { SIGN64 } else { 0 };
    let d = d ^ if negate_d { SIGN64 } else { 0 };
    arith64(cpu, Arith::Add, d, product)
}

#[inline]
pub fn sqrt32(cpu: &Cpu, a: u32) -> u32 {
    let a = flush_in32(cpu, a);
    if let Some(nan) = nans32(cpu, a, a) {
        return nan;
    }
    round32(cpu, f64::from(f32::from_bits(a)).sqrt(), Rounding::Nearest)
}

#[inline]
pub fn sqrt64(cpu: &Cpu, a: u64) -> u64 {
    let a = flush_in64(cpu, a);
    if let Some(nan) = nans64(cpu, a, a) {
        return nan;
    }
    finish64(cpu, f64::from_bits(a).sqrt())
}

// ---- compare ------------------------------------------------------------

fn nzcv(order: Option<std::cmp::Ordering>) -> u32 {
    use std::cmp::Ordering::*;
    match order {
        None => 0b0011,
        Some(Equal) => 0b0110,
        Some(Less) => 0b1000,
        Some(Greater) => 0b0010,
    }
}

/// VCMP: writes FPSCR.NZCV.
#[inline]
pub fn cmp32(cpu: &mut Cpu, a: u32, b: u32) {
    let (a, b) = (f32::from_bits(flush_in32(cpu, a)), f32::from_bits(flush_in32(cpu, b)));
    cpu.fpscr = (cpu.fpscr & 0x0FFF_FFFF) | (nzcv(a.partial_cmp(&b)) << 28);
}

#[inline]
pub fn cmp64(cpu: &mut Cpu, a: u64, b: u64) {
    let (a, b) = (f64::from_bits(flush_in64(cpu, a)), f64::from_bits(flush_in64(cpu, b)));
    cpu.fpscr = (cpu.fpscr & 0x0FFF_FFFF) | (nzcv(a.partial_cmp(&b)) << 28);
}

/// VMRS APSR_nzcv, FPSCR.
#[inline]
pub fn flags_to_apsr(cpu: &mut Cpu) {
    cpu.n = cpu.fpscr >> 31 & 1 == 1;
    cpu.z = cpu.fpscr >> 30 & 1 == 1;
    cpu.c = cpu.fpscr >> 29 & 1 == 1;
    cpu.v = cpu.fpscr >> 28 & 1 == 1;
}

// ---- conversions ---------------------------------------------------------

/// VCVT.F64.F32.
#[inline]
pub fn f32_to_f64(cpu: &Cpu, a: u32) -> u64 {
    let a = flush_in32(cpu, a);
    if is_nan32(a) {
        if cpu.fpscr & DN != 0 {
            return DEFAULT_NAN64;
        }
        let sign = u64::from(a & SIGN32) << 32;
        return sign | 0x7FF0_0000_0000_0000 | QUIET64 | (u64::from(a & 0x003F_FFFF) << 29);
    }
    f64::from(f32::from_bits(a)).to_bits()
}

/// VCVT.F32.F64, in FPSCR's rounding mode.
#[inline]
pub fn f64_to_f32(cpu: &Cpu, a: u64) -> u32 {
    let a = flush_in64(cpu, a);
    if is_nan64(a) {
        if cpu.fpscr & DN != 0 {
            return DEFAULT_NAN32;
        }
        let sign = ((a & SIGN64) >> 32) as u32;
        return sign | 0x7F80_0000 | QUIET32 | ((a >> 29) as u32 & 0x003F_FFFF);
    }
    round32(cpu, f64::from_bits(a), rounding(cpu))
}

/// VCVT.F32.{S32,U32}, in FPSCR's rounding mode (an i32 or u32 is exact in f64).
#[inline]
pub fn int_to_f32(cpu: &Cpu, x: u32, signed: bool) -> u32 {
    let value = if signed { f64::from(x as i32) } else { f64::from(x) };
    round32(cpu, value, rounding(cpu))
}

/// VCVT.F64.{S32,U32}: always exact.
#[inline]
pub fn int_to_f64(x: u32, signed: bool) -> u64 {
    if signed { f64::from(x as i32) } else { f64::from(x) }.to_bits()
}

fn to_int(cpu: &Cpu, value: f64, signed: bool, toward_zero: bool) -> u32 {
    if value.is_nan() {
        return 0;
    }
    let mode = if toward_zero { Rounding::TowardZero } else { rounding(cpu) };
    let r = match mode {
        Rounding::Nearest => value.round_ties_even(),
        Rounding::Up => value.ceil(),
        Rounding::Down => value.floor(),
        Rounding::TowardZero => value.trunc(),
    };
    // `as` saturates, as FPToFixed does
    if signed {
        r as i32 as u32
    } else {
        r as u32
    }
}

/// VCVT{R}.{S32,U32}.F32.
#[inline]
pub fn f32_to_int(cpu: &Cpu, a: u32, signed: bool, toward_zero: bool) -> u32 {
    to_int(cpu, f64::from(f32::from_bits(flush_in32(cpu, a))), signed, toward_zero)
}

/// VCVT{R}.{S32,U32}.F64.
#[inline]
pub fn f64_to_int(cpu: &Cpu, a: u64, signed: bool, toward_zero: bool) -> u32 {
    to_int(cpu, f64::from_bits(flush_in64(cpu, a)), signed, toward_zero)
}

// ---- short vectors (the slow path) --------------------------------------

/// The operations short vectors apply to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vector {
    Arith(Arith),
    Mla { negate_product: bool, negate_d: bool },
    Nmul,
    Mov,
    Abs,
    Neg,
    Sqrt,
}

impl Vector {
    fn rounds(self) -> bool {
        !matches!(self, Vector::Mov | Vector::Abs | Vector::Neg)
    }
}

fn scalar32(cpu: &mut Cpu, op: Vector, d: usize, n: usize, m: usize) {
    let (dv, nv, mv) = (cpu.s[d], cpu.s[n], cpu.s[m]);
    cpu.s[d] = match op {
        Vector::Arith(a) => arith32(cpu, a, nv, mv),
        Vector::Mla { negate_product, negate_d } => mla32(cpu, dv, nv, mv, negate_product, negate_d),
        Vector::Nmul => arith32(cpu, Arith::Mul, nv, mv) ^ SIGN32,
        Vector::Mov => mv,
        Vector::Abs => mv & !SIGN32,
        Vector::Neg => mv ^ SIGN32,
        Vector::Sqrt => sqrt32(cpu, mv),
    };
}

fn scalar64(cpu: &mut Cpu, op: Vector, d_: usize, n: usize, m: usize) {
    let (dv, nv, mv) = (d(cpu, d_), d(cpu, n), d(cpu, m));
    let r = match op {
        Vector::Arith(a) => arith64(cpu, a, nv, mv),
        Vector::Mla { negate_product, negate_d } => mla64(cpu, dv, nv, mv, negate_product, negate_d),
        Vector::Nmul => arith64(cpu, Arith::Mul, nv, mv) ^ SIGN64,
        Vector::Mov => mv,
        Vector::Abs => mv & !SIGN64,
        Vector::Neg => mv ^ SIGN64,
        Vector::Sqrt => sqrt64(cpu, mv),
    };
    set_d(cpu, d_, r);
}

/// Runs a vectorizable operation when FPSCR selects short vectors or a
/// rounding mode other than nearest (`SLOW_BITS`). Registers are indices
/// in the precision's own space.
///
/// VFPv2 short vectors: registers form banks of 8 singles or 4 doubles.
/// A destination in bank 0 makes the operation scalar. Otherwise it is
/// repeated LEN + 1 times, with d and n (and m, unless m is in bank 0)
/// advancing by STRIDE and wrapping within their banks.
pub fn slow(cpu: &mut Cpu, op: Vector, double: bool, d: u8, n: u8, m: u8, address: u32) -> Result<(), Fault> {
    let fault = Fault::UnsupportedFp { address, fpscr: cpu.fpscr };
    if op.rounds() && rounding(cpu) != Rounding::Nearest {
        return Err(fault);
    }
    let len = ((cpu.fpscr >> LEN_SHIFT) & 7) + 1;
    let stride = match (cpu.fpscr >> STRIDE_SHIFT) & 3 {
        0 => 1,
        3 => 2,
        _ => return Err(fault), // UNPREDICTABLE strides
    };
    let bank = if double { 4 } else { 8 };
    // a vector that would wrap onto its own start is UNPREDICTABLE
    if len * stride > bank {
        return Err(fault);
    }
    let (d, n, m) = (u32::from(d), u32::from(n), u32::from(m));
    let step = |r: u32, i: u32| (r / bank) * bank + (r + i * stride) % bank;
    let iterations = if d < bank { 1 } else { len };
    for i in 0..iterations {
        let (di, ni) = (step(d, i), step(n, i));
        let mi = if m < bank || d < bank { m } else { step(m, i) };
        if double {
            scalar64(cpu, op, di as usize, ni as usize, mi as usize);
        } else {
            scalar32(cpu, op, di as usize, ni as usize, mi as usize);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(fpscr: u32) -> Cpu {
        Cpu { fpscr, ..Cpu::default() }
    }

    fn bits(x: f32) -> u32 {
        x.to_bits()
    }

    #[test]
    fn nan_rules() {
        let c = cpu(0);
        let (snan, qnan) = (0x7F80_0001, 0xFFC0_0002);
        // a signaling NaN wins and is quieted, even as the second operand
        assert_eq!(arith32(&c, Arith::Add, qnan, snan), 0x7FC0_0001);
        assert_eq!(arith32(&c, Arith::Add, qnan, bits(1.0)), qnan);
        assert_eq!(arith32(&c, Arith::Sub, f32::INFINITY.to_bits(), f32::INFINITY.to_bits()), DEFAULT_NAN32);
        assert_eq!(arith32(&cpu(DN), Arith::Add, qnan, bits(1.0)), DEFAULT_NAN32);
    }

    #[test]
    fn flush_to_zero() {
        let tiny = 0x0000_0001; // smallest subnormal
        assert_eq!(arith32(&cpu(0), Arith::Add, tiny, tiny), 2);
        assert_eq!(arith32(&cpu(FZ), Arith::Add, tiny, bits(1.0)), bits(1.0), "input flushed");
        // min normal * 0.5 is subnormal: flushed, keeping the sign
        assert_eq!(arith32(&cpu(FZ), Arith::Mul, bits(-f32::MIN_POSITIVE), bits(0.5)), SIGN32);
    }

    #[test]
    fn directed_conversions() {
        // 2^24 + 1 is not representable: nearest-even gives 2^24, up 2^24 + 2
        let x = (1u32 << 24) + 1;
        assert_eq!(int_to_f32(&cpu(0), x, false), bits(16_777_216.0));
        assert_eq!(int_to_f32(&cpu(1 << RMODE_SHIFT), x, false), bits(16_777_218.0));
        assert_eq!(int_to_f32(&cpu(3 << RMODE_SHIFT), x, false), bits(16_777_216.0));
        assert_eq!(f32_to_int(&cpu(0), bits(-2.5), true, false), (-2i32) as u32, "ties to even");
        assert_eq!(f32_to_int(&cpu(2 << RMODE_SHIFT), bits(-2.5), true, false), (-3i32) as u32);
        assert_eq!(f32_to_int(&cpu(0), bits(-1.0), false, true), 0, "saturates at 0");
        assert_eq!(f32_to_int(&cpu(0), bits(f32::NAN), true, true), 0);
        assert_eq!(f32_to_int(&cpu(0), bits(3e9), true, true), i32::MAX as u32);
    }

    #[test]
    fn short_vector_banks() {
        // LEN = 2 (three elements), stride 1: s8..s10 = s16..s18 + s0 (scalar)
        let mut c = cpu(2 << LEN_SHIFT);
        for i in 0..3 {
            c.s[16 + i] = bits(i as f32);
        }
        c.s[0] = bits(10.0);
        slow(&mut c, Vector::Arith(Arith::Add), false, 8, 16, 0, 0).unwrap();
        assert_eq!(&c.s[8..11], &[bits(10.0), bits(11.0), bits(12.0)]);
        // stride 2 (STRIDE = 0b11), LEN = 2: d = s17, s19, s21; n = s20, s22,
        // s16 (wrapping in bank s16-s23); m = s18, s20, s22. Derived from the
        // ARM ARM's short-vector rules; QEMU disagrees here (R-018).
        let mut c = cpu((2 << LEN_SHIFT) | (3 << STRIDE_SHIFT));
        for (i, v) in [16.0f32, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 23.0].iter().enumerate() {
            c.s[16 + i] = bits(*v);
        }
        slow(&mut c, Vector::Arith(Arith::Add), false, 17, 20, 18, 0).unwrap();
        // element 0: s17 = s20 + s18 = 38; element 1: s19 = s22 + s20 = 42;
        // element 2: s21 = s16 + s22 = 16 + 22 = 38
        assert_eq!((c.s[17], c.s[19], c.s[21]), (bits(38.0), bits(42.0), bits(38.0)));
        assert_eq!(c.s[18], bits(18.0), "elements in between are untouched");
        // two-operand doubles, stride 2, LEN = 1: d12 = -d10, d14 = -d8
        // (d10 + 2 wraps to d8 in bank d8-d11); QEMU differs here (R-018)
        let mut c = cpu((1 << LEN_SHIFT) | (3 << STRIDE_SHIFT));
        set_d(&mut c, 8, 1.5f64.to_bits());
        set_d(&mut c, 10, 2.5f64.to_bits());
        slow(&mut c, Vector::Neg, true, 12, 0, 10, 0).unwrap();
        assert_eq!((d(&c, 12), d(&c, 14)), ((-2.5f64).to_bits(), (-1.5f64).to_bits()));
        assert_eq!(d(&c, 8), 1.5f64.to_bits(), "the source is not written");
        // a destination in bank 0 is scalar
        let mut c = cpu(2 << LEN_SHIFT);
        slow(&mut c, Vector::Arith(Arith::Add), false, 1, 16, 0, 0).unwrap();
        assert_eq!(c.s[2], 0);
        // directed rounding in arithmetic is refused, not approximated
        let mut c = cpu(1 << RMODE_SHIFT);
        assert!(slow(&mut c, Vector::Arith(Arith::Add), false, 1, 2, 3, 0x100).is_err());
        assert!(slow(&mut c, Vector::Mov, false, 1, 2, 3, 0x100).is_ok(), "moves do not round");
    }
}
