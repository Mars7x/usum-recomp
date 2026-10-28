//! The PICA200 shader unit: an interpreter for the shader instruction
//! set (3dbrew "GPU/Shader Instruction Set"), and the state the
//! command lists upload into it (3dbrew "GPU/Internal Registers", shader
//! registers).
//!
//! Arithmetic is f32. The hardware's floats are 24-bit (1.7.16), with no
//! negative zero and subnormals flushed; f32 keeps more precision, which
//! is an approximation recorded in RESEARCH.md (R-032). The special cases
//! 3dbrew lists are kept: 0 × anything (but NaN) = 0, GLSL-style MIN and
//! MAX, rcp(±0) = rsq(±0) = +inf.

pub type Vec4 = [f32; 4];

/// Words of program and operand descriptor memory (12-bit offsets).
pub const CODE_WORDS: usize = 4096;
pub const DESC_WORDS: usize = 128;

/// float1.7.16 (bias 63) to f32. Exponent 0 is zero (no subnormals);
/// the top exponent is infinity or NaN.
pub fn f24(bits: u32) -> f32 {
    let sign = (bits >> 23) & 1;
    let exponent = (bits >> 16) & 0x7F;
    let mantissa = bits & 0xFFFF;
    let out = match exponent {
        0 => sign << 31,
        0x7F => (sign << 31) | (0xFF << 23) | (mantissa << 7),
        e => (sign << 31) | ((e + 127 - 63) << 23) | (mantissa << 7),
    };
    f32::from_bits(out)
}

/// Three words holding four float1.7.16 values: one 96-bit number with
/// the first word most significant, holding w, z, y, x from the top.
/// That is the layout 3dbrew gives for GPUREG_FIXEDATTRIB_DATA0-2 (W in
/// DATA0 bits 8-31, X in DATA2 bits 0-23), and it is what Ultra Sun's
/// float24 uniforms decode to cleanly (R-032: c5 = 0, 1, 2, 0.5).
pub fn unpack_f24x4(w: [u32; 3]) -> Vec4 {
    let wv = w[0] >> 8;
    let z = ((w[0] & 0xFF) << 16) | (w[1] >> 16);
    let y = ((w[1] & 0xFFFF) << 8) | (w[2] >> 24);
    let x = w[2] & 0xFF_FFFF;
    [f24(x), f24(y), f24(z), f24(wv)]
}

/// One source operand as an operand descriptor selects it: the component
/// each output component takes, and the sign bit to flip (negation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operand {
    pub swizzle: [u8; 4],
    pub sign: u32,
}

impl Operand {
    /// From an 8-bit selector (x first, two bits each) and the negate bit.
    fn new(selector: u32, negate: bool) -> Operand {
        Operand {
            swizzle: std::array::from_fn(|k| ((selector >> (6 - 2 * k)) & 3) as u8),
            sign: if negate { 0x8000_0000 } else { 0 },
        }
    }

    /// `raw` swizzled and maybe negated; flipping the sign bit is exactly
    /// what unary minus does.
    #[inline(always)]
    fn apply(&self, raw: Vec4) -> Vec4 {
        super::arr4(|k| f32::from_bits(raw[usize::from(self.swizzle[k])].to_bits() ^ self.sign))
    }
}

/// An operand descriptor's three sources, decoded (3dbrew: src1 selector
/// bits 5-12, negate bit 4; src2 bits 14-21, bit 13; src3 bits 23-30,
/// bit 22).
fn operands(desc: u32) -> [Operand; 3] {
    [
        Operand::new((desc >> 5) & 0xFF, (desc >> 4) & 1 == 1),
        Operand::new((desc >> 14) & 0xFF, (desc >> 13) & 1 == 1),
        Operand::new((desc >> 23) & 0xFF, (desc >> 22) & 1 == 1),
    ]
}

/// The state of one shader unit (vertex or geometry), as uploaded.
#[derive(Debug, Clone)]
pub struct Unit {
    pub code: Vec<u32>,
    pub descs: Vec<u32>,
    /// `descs` decoded, kept in step with every descriptor write.
    pub operands: Vec<[Operand; 3]>,
    pub floats: Vec<Vec4>,
    pub ints: [[u8; 4]; 4],
    pub bools: u16,
    pub entry: u32,
    /// Input attribute k goes to input register `permutation[k]`.
    pub permutation: [u8; 16],
    pub output_mask: u16,
    // upload cursors
    code_at: usize,
    desc_at: usize,
    float_at: usize,
    float32: bool,
    pending: Vec<u32>,
    #[cfg(feature = "vulkan-compare")]
    gpu_cache: Option<std::sync::Arc<GpuProgramCache>>,
}

#[cfg(feature = "vulkan-compare")]
#[derive(Debug)]
struct GpuProgramCache {
    entry: u32,
    code: Vec<u32>,
    descriptors: Vec<u32>,
    program: Result<std::sync::Arc<usum_render::VertexProgram>, String>,
}

impl Default for Unit {
    fn default() -> Unit {
        Unit {
            code: vec![0; CODE_WORDS],
            descs: vec![0; DESC_WORDS],
            operands: vec![operands(0); DESC_WORDS],
            floats: vec![[0.0; 4]; 96],
            ints: [[0; 4]; 4],
            bools: 0,
            entry: 0,
            permutation: std::array::from_fn(|k| k as u8),
            output_mask: 0,
            code_at: 0,
            desc_at: 0,
            float_at: 0,
            float32: false,
            pending: Vec::new(),
            #[cfg(feature = "vulkan-compare")]
            gpu_cache: None,
        }
    }
}

impl Unit {
    #[cfg(feature = "vulkan-compare")]
    pub(super) fn gpu_program(&mut self) -> Result<std::sync::Arc<usum_render::VertexProgram>, String> {
        if !self
            .gpu_cache
            .as_ref()
            .is_some_and(|cache| cache.entry == self.entry && cache.code == self.code && cache.descriptors == self.descs)
        {
            self.gpu_cache = Some(std::sync::Arc::new(GpuProgramCache {
                entry: self.entry,
                code: self.code.clone(),
                descriptors: self.descs.clone(),
                program: usum_render::VertexProgram::compile(&self.code, &self.descs, self.entry),
            }));
        }
        self.gpu_cache.as_ref().expect("vertex program cached").program.clone()
    }

    /// A write to the shader register at `offset` from the unit's base
    /// (0x280 geometry, 0x2B0 vertex).
    pub fn write(&mut self, offset: u16, value: u32) {
        match offset {
            0x00 => self.bools = value as u16,
            0x01..=0x04 => self.ints[usize::from(offset - 1)] = value.to_le_bytes(),
            0x0A => self.entry = value & 0xFFFF,
            0x0B => {
                for k in 0..8 {
                    self.permutation[k] = ((value >> (4 * k)) & 0xF) as u8;
                }
            }
            0x0C => {
                for k in 0..8 {
                    self.permutation[8 + k] = ((value >> (4 * k)) & 0xF) as u8;
                }
            }
            0x0D => self.output_mask = value as u16,
            0x10 => {
                self.float_at = (value & 0xFF) as usize;
                self.float32 = value >> 31 != 0;
                self.pending.clear();
            }
            0x11..=0x18 => {
                self.pending.push(value);
                let need = if self.float32 { 4 } else { 3 };
                if self.pending.len() == need {
                    let v = if self.float32 {
                        let f = |k: usize| f32::from_bits(self.pending[k]);
                        [f(3), f(2), f(1), f(0)]
                    } else {
                        unpack_f24x4([self.pending[0], self.pending[1], self.pending[2]])
                    };
                    if let Some(slot) = self.floats.get_mut(self.float_at) {
                        *slot = v;
                    }
                    self.float_at += 1;
                    self.pending.clear();
                }
            }
            0x1B => self.code_at = (value & 0xFFF) as usize,
            0x1C..=0x23 => {
                if let Some(slot) = self.code.get_mut(self.code_at) {
                    *slot = value;
                }
                self.code_at += 1;
            }
            0x25 => self.desc_at = (value & 0xFFF) as usize,
            0x26..=0x2D => {
                if let Some(slot) = self.descs.get_mut(self.desc_at) {
                    *slot = value;
                    self.operands[self.desc_at] = operands(value);
                }
                self.desc_at += 1;
            }
            _ => {}
        }
    }
}

/// 0 × anything but NaN is 0 (3dbrew).
fn mul(a: f32, b: f32) -> f32 {
    if (a == 0.0 && !b.is_nan()) || (b == 0.0 && !a.is_nan()) {
        0.0
    } else {
        a * b
    }
}

fn max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

fn min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

/// `f` per component (generic, so each use is inlined).
fn each(s1: &Vec4, s2: &Vec4, f: impl Fn(f32, f32) -> f32) -> Vec4 {
    super::arr4(|k| f(s1[k], s2[k]))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(&x, &y)| mul(x, y)).sum()
}

fn compare(a: f32, b: f32, op: u32) -> bool {
    match op {
        0 => a == b,
        1 => a != b,
        2 => a < b,
        3 => a <= b,
        4 => a > b,
        5 => a >= b,
        _ => true,
    }
}

enum Frame {
    /// When `pc` reaches `at`, continue at `then`.
    Jump { at: u32, then: u32 },
    /// A LOOP body ending before `at`, starting at `start`.
    Loop { at: u32, start: u32, left: u32, step: i32 },
    /// A CALL returning to `then` when `pc` reaches `at`.
    Call { at: u32, then: u32 },
}

/// One run of the program from the entry point over input registers
/// `v` (already permuted); returns the output registers o0-o15.
/// Set to make the next `run` print each instruction (debugging).
pub static TRACE_ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The geometry shader's output state (3dbrew "GPU/Shader Instruction
/// Set", SETEMIT/EMIT): a buffer of four vertices, which a primitive
/// emit turns into a triangle. The buffer is not cleared between
/// primitives (3dbrew's working hypothesis).
#[derive(Debug, Default)]
pub struct Emitter {
    buffer: [[Vec4; 16]; 4],
    vertex: usize,
    winding: bool,
    primitive: bool,
    /// Triangles emitted: three vertices' output registers each.
    pub triangles: Vec<[[Vec4; 16]; 3]>,
}

pub fn run(unit: &Unit, v: &[Vec4; 16]) -> Result<[Vec4; 16], String> {
    run_with(unit, v, None)
}

/// `run` for a geometry shader: EMIT and SETEMIT go to `emitter`.
/// Shader runs and instructions executed, for the probe's report.
pub static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static STEPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn run_with(unit: &Unit, v: &[Vec4; 16], emitter: Option<&mut Emitter>) -> Result<[Vec4; 16], String> {
    let mut steps = 0u32;
    let out = run_counted(unit, v, emitter, &mut steps);
    record_runs(1, u64::from(steps));
    out
}

/// Runs a vertex shader and returns its instruction count, including on failure.
/// Batch callers publish their accumulated statistics after workers finish.
pub(super) fn run_measured(unit: &Unit, v: &[Vec4; 16]) -> (Result<[Vec4; 16], String>, u32) {
    let mut steps = 0;
    let out = run_counted(unit, v, None, &mut steps);
    (out, steps)
}

pub(super) fn record_runs(runs: u64, steps: u64) {
    RUNS.fetch_add(runs, std::sync::atomic::Ordering::Relaxed);
    STEPS.fetch_add(steps, std::sync::atomic::Ordering::Relaxed);
}

/// A float uniform with relative addressing (3dbrew): out-of-range offsets
/// read as 0, the index wraps at 0x80, beyond c95 reads (1, 1, 1, 1).
#[inline(always)]
fn float(unit: &Unit, index: usize, offset: i32) -> Vec4 {
    let offset = if (-128..=127).contains(&offset) { offset } else { 0 };
    let i = ((index as i32 + offset) & 0x7F) as usize;
    unit.floats.get(i).copied().unwrap_or([1.0; 4])
}

/// A source register (input, temporary or uniform), swizzled and maybe
/// negated by its operand descriptor.
#[inline(always)]
fn source(unit: &Unit, v: &[Vec4; 16], reg: u32, offset: i32, operand: &Operand, r: &[Vec4; 16]) -> Vec4 {
    let raw = match reg {
        0x00..=0x0F => v[reg as usize],
        0x10..=0x1F => r[(reg - 0x10) as usize],
        _ => float(unit, (reg - 0x20) as usize, offset),
    };
    operand.apply(raw)
}

/// Writes the masked components of `value` to output or temporary `dst`.
#[inline(always)]
fn write(dst: u32, mask: u32, value: Vec4, r: &mut [Vec4; 16], o: &mut [Vec4; 16]) {
    let target = if dst < 0x10 { &mut o[dst as usize] } else { &mut r[(dst - 0x10) as usize & 0xF] };
    for k in 0..4 {
        if mask & (8 >> k) != 0 {
            target[k] = value[k];
        }
    }
}

thread_local! {
    /// The flow-control stack, kept between runs so a run does not
    /// allocate (shaders run millions of times a minute).
    static STACK: std::cell::RefCell<Vec<Frame>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn run_counted(unit: &Unit, v: &[Vec4; 16], emitter: Option<&mut Emitter>, steps: &mut u32) -> Result<[Vec4; 16], String> {
    STACK.with(|stack| match stack.try_borrow_mut() {
        Ok(mut stack) => {
            stack.clear();
            run_on(unit, v, emitter, steps, &mut stack)
        }
        // a run inside a run cannot happen, but would still work
        Err(_) => run_on(unit, v, emitter, steps, &mut Vec::new()),
    })
}

fn run_on(
    unit: &Unit,
    v: &[Vec4; 16],
    mut emitter: Option<&mut Emitter>,
    steps: &mut u32,
    stack: &mut Vec<Frame>,
) -> Result<[Vec4; 16], String> {
    // Most vertices run with tracing disabled. A shared read avoids forcing
    // every shader worker to acquire this cache line for an atomic write.
    let trace =
        TRACE_ONCE.load(std::sync::atomic::Ordering::Relaxed) && TRACE_ONCE.swap(false, std::sync::atomic::Ordering::Relaxed);
    let mut r = [[0f32; 4]; 16];
    let mut o = [[0f32; 4]; 16];
    let mut a0 = [0i32; 2];
    let mut al = 0i32;
    let mut cmp = [false; 2];
    let mut pc = unit.entry;
    loop {
        *steps += 1;
        if *steps > 0x1_0000 {
            return Err(format!("vertex shader ran away at {pc:#x}"));
        }
        let word = *unit.code.get(pc as usize).ok_or_else(|| format!("shader pc {pc:#x} outside code memory"))?;
        let opcode = word >> 26;
        if trace {
            let regs: Vec<String> =
                r.iter().enumerate().filter(|(_, x)| x.iter().any(|&c| c != 0.0)).map(|(i, x)| format!("r{i}={x:?}")).collect();
            eprintln!("    {pc:03x}: {word:08x} op {opcode:02x} a0 {a0:?} cmp {cmp:?} {}", regs.join(" "));
        }
        let mut next = pc + 1;
        let offset_of = |idx: u32| -> i32 {
            match idx {
                1 => a0[0],
                2 => a0[1],
                3 => al,
                _ => 0,
            }
        };
        // a source register, swizzled and maybe negated by descriptor bits
        let condition = |word: u32, cmp: &[bool; 2]| -> bool {
            let (refy, refx) = ((word >> 24) & 1 == 1, (word >> 25) & 1 == 1);
            let (x, y) = (cmp[0] == refx, cmp[1] == refy);
            match (word >> 22) & 3 {
                0 => x || y,
                1 => x && y,
                2 => x,
                _ => y,
            }
        };
        let num = word & 0xFF;
        let target = (word >> 10) & 0xFFF;
        match opcode {
            // arithmetic, formats 1 / 1i / 1u
            0x00..=0x0F | 0x12 | 0x13 | 0x18..=0x1B => {
                let (desc, ops) = (unit.descs[(word & 0x7F) as usize], &unit.operands[(word & 0x7F) as usize]);
                let inverted = (0x18..=0x1B).contains(&opcode);
                let (src1, src2, idx) = if inverted {
                    ((word >> 14) & 0x1F, (word >> 7) & 0x7F, (word >> 19) & 3)
                } else {
                    ((word >> 12) & 0x7F, (word >> 7) & 0x1F, (word >> 19) & 3)
                };
                let (o1, o2) = if inverted { (0, offset_of(idx)) } else { (offset_of(idx), 0) };
                let s1 = source(unit, v, src1, o1, &ops[0], &r);
                let s2 = source(unit, v, src2, o2, &ops[1], &r);
                let dst = (word >> 21) & 0x1F;
                let mask = desc & 0xF;
                let broadcast = |x: f32| [x; 4];
                let value = match opcode {
                    0x00 => each(&s1, &s2, |a, b| a + b),
                    0x01 => broadcast(dot(&s1[..3], &s2[..3])),
                    0x02 => broadcast(dot(&s1, &s2)),
                    0x03 | 0x18 => broadcast(dot(&[s1[0], s1[1], s1[2], 1.0], &s2)),
                    0x04 | 0x19 => [1.0, mul(s1[1], s2[1]), s1[2], s2[3]],
                    0x05 => broadcast(s1[0].exp2()),
                    0x06 => broadcast(s1[0].log2()),
                    0x07 => {
                        cmp = [s1[0] >= 0.0, s1[3] >= 0.0];
                        [max(s1[0], 0.0), s1[1].clamp(-127.9961, 127.9961), 0.0, max(s1[3], 0.0)]
                    }
                    0x08 => each(&s1, &s2, mul),
                    0x09 | 0x1A => each(&s1, &s2, |a, b| if a >= b { 1.0 } else { 0.0 }),
                    0x0A | 0x1B => each(&s1, &s2, |a, b| if a < b { 1.0 } else { 0.0 }),
                    0x0B => s1.map(f32::floor),
                    0x0C => each(&s1, &s2, max),
                    0x0D => each(&s1, &s2, min),
                    0x0E => broadcast(if s1[0] == 0.0 { f32::INFINITY } else { 1.0 / s1[0] }),
                    0x0F => broadcast(if s1[0] == 0.0 { f32::INFINITY } else { 1.0 / s1[0].sqrt() }),
                    0x12 => {
                        if mask & 8 != 0 {
                            a0[0] = s1[0] as i32;
                        }
                        if mask & 4 != 0 {
                            a0[1] = s1[1] as i32;
                        }
                        pc = next;
                        continue_after(stack, &mut pc, &mut al);
                        continue;
                    }
                    _ => s1, // MOV
                };
                write(dst, mask, value, &mut r, &mut o);
            }
            // CMP (format 1c)
            0x2E | 0x2F => {
                let ops = &unit.operands[(word & 0x7F) as usize];
                let (src1, src2, idx) = ((word >> 12) & 0x7F, (word >> 7) & 0x1F, (word >> 19) & 3);
                let s1 = source(unit, v, src1, offset_of(idx), &ops[0], &r);
                let s2 = source(unit, v, src2, 0, &ops[1], &r);
                cmp = [compare(s1[0], s2[0], (word >> 24) & 7), compare(s1[1], s2[1], (word >> 21) & 7)];
            }
            // MADI (5i) and MAD (5)
            0x30..=0x3F => {
                let (desc, ops) = (unit.descs[(word & 0x1F) as usize], &unit.operands[(word & 0x1F) as usize]);
                let madi = opcode < 0x38;
                let (src1, src2, src3, idx) = if madi {
                    ((word >> 17) & 0x1F, (word >> 12) & 0x1F, (word >> 5) & 0x7F, (word >> 22) & 3)
                } else {
                    ((word >> 17) & 0x1F, (word >> 10) & 0x7F, (word >> 5) & 0x1F, (word >> 22) & 3)
                };
                let (o2, o3) = if madi { (0, offset_of(idx)) } else { (offset_of(idx), 0) };
                let s1 = source(unit, v, src1, 0, &ops[0], &r);
                let s2 = source(unit, v, src2, o2, &ops[1], &r);
                let s3 = source(unit, v, src3, o3, &ops[2], &r);
                let value = super::arr4(|k| mul(s1[k], s2[k]) + s3[k]);
                write((word >> 24) & 0x1F, desc & 0xF, value, &mut r, &mut o);
            }
            0x21 => {} // NOP
            0x22 => {
                if trace {
                    eprintln!("    end: o0 {:?} o1 {:?} o2 {:?} o3 {:?} o4 {:?}", o[0], o[1], o[2], o[3], o[4]);
                }
                return Ok(o);
            }
            0x20 | 0x23 => {
                // BREAK / BREAKC: leave the innermost loop
                if opcode == 0x20 || condition(word, &cmp) {
                    while let Some(f) = stack.pop() {
                        if let Frame::Loop { at, .. } = f {
                            next = at;
                            break;
                        }
                    }
                }
            }
            0x24..=0x26 => {
                let take = match opcode {
                    0x24 => true,
                    0x25 => condition(word, &cmp),
                    _ => unit.bools >> ((word >> 22) & 0xF) & 1 == 1,
                };
                if take {
                    stack.push(Frame::Call { at: target + num, then: pc + 1 });
                    next = target;
                }
            }
            0x27 | 0x28 => {
                let yes = if opcode == 0x27 { unit.bools >> ((word >> 22) & 0xF) & 1 == 1 } else { condition(word, &cmp) };
                if yes {
                    // the "then" part runs to `target`, then skips the else part
                    stack.push(Frame::Jump { at: target, then: target + num });
                } else {
                    next = target;
                }
            }
            0x29 => {
                let i = unit.ints[((word >> 22) & 3) as usize];
                al = i32::from(i[1]);
                stack.push(Frame::Loop { at: target + 1, start: pc + 1, left: u32::from(i[0]), step: i32::from(i[2] as i8) });
            }
            0x2C | 0x2D => {
                let yes = if opcode == 0x2C {
                    condition(word, &cmp)
                } else {
                    let b = unit.bools >> ((word >> 22) & 0xF) & 1 == 1;
                    b == (num & 1 == 0)
                };
                if yes {
                    next = target;
                }
            }
            0x2B => match emitter.as_deref_mut() {
                Some(e) => {
                    e.vertex = ((word >> 24) & 3) as usize;
                    e.winding = (word >> 22) & 1 == 1;
                    e.primitive = (word >> 23) & 1 == 1;
                }
                None => return Err("SETEMIT in a vertex shader".into()),
            },
            0x2A => match emitter.as_deref_mut() {
                Some(e) => {
                    e.buffer[e.vertex.min(3)] = o;
                    if e.primitive {
                        let (a, b, c) = (e.buffer[0], e.buffer[1], e.buffer[2]);
                        e.triangles.push(if e.winding { [a, c, b] } else { [a, b, c] });
                    }
                }
                None => return Err("EMIT in a vertex shader".into()),
            },
            other => return Err(format!("shader opcode {other:#x} at {pc:#x}")),
        }
        pc = next;
        continue_after(stack, &mut pc, &mut al);
    }
}

/// Applies the flow-control frames that end where execution now is.
fn continue_after(stack: &mut Vec<Frame>, pc: &mut u32, al: &mut i32) {
    while let Some(top) = stack.last_mut() {
        match top {
            Frame::Loop { at, start, left, step } if *pc == *at => {
                *al = al.wrapping_add(*step);
                if *left > 0 {
                    *left -= 1;
                    *pc = *start;
                    return;
                }
                stack.pop();
            }
            Frame::Jump { at, then } | Frame::Call { at, then } if *pc == *at => {
                *pc = *then;
                stack.pop();
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::identity_op)]
    use super::*;

    /// An operand descriptor: mask xyzw, identity swizzles, no negation.
    const XYZW: u32 = 0xF | (0x1B << 5) | (0x1B << 14) | (0x1B << 23);

    fn unit(code: &[u32]) -> Unit {
        let mut u = Unit::default();
        u.code[..code.len()].copy_from_slice(code);
        u.descs[0] = XYZW;
        u.operands[0] = operands(XYZW);
        u
    }

    #[cfg(feature = "vulkan-compare")]
    #[test]
    fn gpu_program_cache_tracks_public_code_descriptor_and_entry_changes() {
        let mut u = unit(&[0x13 << 26, 0x22 << 26]);
        let first = u.gpu_program().unwrap();
        u.floats[0] = [7.0; 4];
        u.ints[0] = [3, 0, 1, 0];
        u.bools = 1;
        assert!(std::sync::Arc::ptr_eq(&first, &u.gpu_program().unwrap()));
        u.code[0] = 0x21 << 26;
        let changed = u.gpu_program().unwrap();
        assert!(!std::sync::Arc::ptr_eq(&first, &changed));
        u.descs[0] ^= 1;
        let descriptor = u.gpu_program().unwrap();
        assert!(!std::sync::Arc::ptr_eq(&changed, &descriptor));
        u.entry = 1;
        assert!(!std::sync::Arc::ptr_eq(&descriptor, &u.gpu_program().unwrap()));
    }

    #[test]
    fn measured_shader_runs_count_success_and_failure_instructions() {
        let valid = unit(&[0x13 << 26, 0x22 << 26]); // mov o0, v0; end
        let mut inputs = [[0.0; 4]; 16];
        inputs[0] = [1.0, -2.0, 3.0, 4.0];
        let (result, steps) = run_measured(&valid, &inputs);
        assert_eq!(result.unwrap()[0], inputs[0]);
        assert_eq!(steps, 2);

        let invalid = unit(&[0x11 << 26]);
        let (result, steps) = run_measured(&invalid, &inputs);
        assert!(result.is_err());
        assert_eq!(steps, 1);
    }

    #[test]
    fn operands_match_the_descriptor_bits() {
        // the old per-component decode, for comparison
        let old = |raw: Vec4, selector: u32, negate: bool| -> Vec4 {
            std::array::from_fn(|k| {
                let x = raw[((selector >> (6 - 2 * k)) & 3) as usize];
                if negate {
                    -x
                } else {
                    x
                }
            })
        };
        let raw = [1.5, -0.0, f32::NAN, f32::INFINITY];
        for selector in 0..256 {
            for negate in [false, true] {
                let new = Operand::new(selector, negate).apply(raw);
                let want = old(raw, selector, negate);
                assert_eq!(new.map(f32::to_bits), want.map(f32::to_bits), "selector {selector:#x} negate {negate}");
            }
        }
        let d = 0x1B << 5 | 1 << 4 | 0xE4 << 14 | 0x55 << 23 | 1 << 22;
        let [a, b, c] = operands(d);
        assert_eq!((a.swizzle, a.sign), ([0, 1, 2, 3], 0x8000_0000));
        assert_eq!((b.swizzle, b.sign), ([3, 2, 1, 0], 0));
        assert_eq!((c.swizzle, c.sign), ([1, 1, 1, 1], 0x8000_0000));
    }

    #[test]
    fn float24_round_values() {
        // 1.0: exponent 63, mantissa 0
        assert_eq!(f24(63 << 16), 1.0);
        assert_eq!(f24(1 << 23 | 64 << 16 | 0x8000), -3.0);
        assert_eq!(f24(0), 0.0);
        let one = 63u32 << 16;
        let two = 64u32 << 16;
        // x = 1, y = 2, z = 1, w = 2 packed as 3dbrew shows
        let (x, y, z, w) = (one, two, one, two);
        let words = [w << 8 | z >> 16, (z & 0xFFFF) << 16 | y >> 8, (y & 0xFF) << 24 | x];
        assert_eq!(unpack_f24x4(words), [1.0, 2.0, 1.0, 2.0]);
        // Ultra Sun's c5 as uploaded: x 0, y 1, z 2, w 0.5
        assert_eq!(unpack_f24x4([0x3E00_0040, 0x0000_3F00, 0]), [0.0, 1.0, 2.0, 0.5]);
    }

    #[test]
    fn dp4_of_a_uniform_matrix_row_and_end() {
        // dp4 o0, c0, v0 (format 1: src1 = c0 = 0x20, src2 = v0), then end
        let dp4 = 0x02 << 26 | 0x20 << 12;
        let mut u = unit(&[dp4, 0x22 << 26]);
        u.floats[0] = [1.0, 2.0, 3.0, 4.0];
        let mut v = [[0f32; 4]; 16];
        v[0] = [1.0, 1.0, 1.0, 1.0];
        assert_eq!(run(&u, &v).unwrap()[0], [10.0; 4]);
    }

    #[test]
    fn mad_and_relative_uniforms() {
        // mova a0.x, v1 ; mov r0, c0[a0.x] ; mad o1, v0, r0, v0 ; end
        let mova = 0x12 << 26 | 0x01 << 12;
        let mov = 0x13 << 26 | 0x10 << 21 | 0x20 << 12 | 1 << 19;
        let mad = 0x38 << 26 | 0x01 << 24 | 0x00 << 17 | 0x10 << 10 | 0x00 << 5;
        let mut u = unit(&[mova, mov, mad, 0x22 << 26]);
        u.floats[2] = [2.0, 2.0, 2.0, 2.0];
        let mut v = [[0f32; 4]; 16];
        v[0] = [1.0, 2.0, 3.0, 4.0];
        v[1] = [2.0, 0.0, 0.0, 0.0];
        assert_eq!(run(&u, &v).unwrap()[1], [3.0, 6.0, 9.0, 12.0]);
    }

    #[test]
    fn loops_count_and_step_al() {
        // i0 = (3, 0, 1): 4 iterations; body: add r0, r0, c1 ; then mov o0, r0
        let lp = 0x29 << 26 | 1 << 10; // loop i0 to address 1 (inclusive)
        let add = 0x00 << 26 | 0x10 << 21 | 0x10 << 12 | 0 << 7; // r0 = r0 + v0
        let mov = 0x13 << 26 | 0x10 << 12;
        let mut u = unit(&[lp, add, mov, 0x22 << 26]);
        u.ints[0] = [3, 0, 1, 0];
        let mut v = [[0f32; 4]; 16];
        v[0] = [1.0; 4];
        assert_eq!(run(&u, &v).unwrap()[0], [4.0; 4]);
    }

    #[test]
    fn special_values() {
        assert_eq!(mul(0.0, f32::INFINITY), 0.0);
        assert!(mul(0.0, f32::NAN).is_nan());
        assert!(max(0.0, f32::NAN).is_nan() && max(f32::NAN, 0.0) == 0.0);
    }
}
