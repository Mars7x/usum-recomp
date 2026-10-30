//! One draw call: vertex loading, the vertex shader, primitive assembly,
//! clipping, the viewport, rasterization, the texture combiners and the
//! per-fragment operations, into the tiled color and depth buffers.
//! Register meanings from 3dbrew "GPU/Internal Registers" and "GPU/
//! Pitfalls" (attribute alignment, output map slots).
//!
//! Choices where 3dbrew is silent, to be checked against the game's own
//! frames (R-032): clipping keeps w > 0 and -w <= z <= 0 (depth = -z/w in
//! [0, 1]); rasterized rows count from the framebuffer's bottom (OpenGL
//! convention); attribute components not present read as (0, 0, 0, 1);
//! procedural texture (unit 3) reads as opaque white.

use usum_guest::Memory;

#[cfg(feature = "vulkan-compare")]
mod input;
#[cfg(feature = "vulkan-compare")]
pub(super) mod input_cache;
mod stencil;

use super::shader::{self, Vec4};
use super::{bytes_per_pixel, decode_pixel, encode_pixel, tiled_offset, Pica};

/// A shaded vertex, by output semantic (3dbrew SH_OUTMAP_Oi values).
#[derive(Debug, Clone, Copy, Default)]
pub struct Vertex {
    pub position: Vec4,
    pub color: Vec4,
    pub texcoord: [[f32; 3]; 3],
    /// For fragment lighting: the quaternion rotating the surface's frame
    /// into view space, and the view vector (3dbrew output map semantics
    /// 0x04-0x07 and 0x12-0x14).
    pub quaternion: Vec4,
    pub view: [f32; 3],
}

fn reg(p: &Pica, r: u16) -> u32 {
    p.regs[usize::from(r)]
}

#[cfg(feature = "vulkan-compare")]
fn input_parallel_min() -> usize {
    static MIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MIN.get_or_init(|| {
        std::env::var("USUM_VULKAN_INPUT_PARALLEL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(4096).max(256)
    })
}

/// Builds a vertex from the shader's outputs through the output map: the
/// k-th *enabled* output register uses map slot k (3dbrew "GPU/Pitfalls").
fn map_outputs(p: &Pica, mask: u16, o: &[Vec4; 16]) -> Vertex {
    let mut v = Vertex { color: [0.0, 0.0, 0.0, 1.0], ..Vertex::default() };
    let total = (reg(p, 0x4F) & 7) as usize;
    let mut slot = 0;
    for (n, out) in o.iter().enumerate() {
        if mask & (1 << n) == 0 {
            continue;
        }
        if slot >= total.min(7) {
            break;
        }
        let map = reg(p, 0x50 + slot as u16);
        for (c, &value) in out.iter().enumerate() {
            match (map >> (8 * c)) & 0x1F {
                s @ 0x00..=0x03 => v.position[s as usize] = value,
                s @ 0x04..=0x07 => v.quaternion[(s - 4) as usize] = value,
                s @ 0x08..=0x0B => v.color[(s - 8) as usize] = value,
                s @ 0x12..=0x14 => v.view[(s - 0x12) as usize] = value,
                0x0C => v.texcoord[0][0] = value,
                0x0D => v.texcoord[0][1] = value,
                0x0E => v.texcoord[1][0] = value,
                0x0F => v.texcoord[1][1] = value,
                0x10 => v.texcoord[0][2] = value,
                0x16 => v.texcoord[2][0] = value,
                0x17 => v.texcoord[2][1] = value,
                _ => {}
            }
        }
        slot += 1;
    }
    v
}

/// Reads one attribute element of `kind` (0 byte, 1 unsigned byte, 2
/// short, 3 float).
fn element(mem: &Memory, address: u32, kind: u32) -> Result<f32, String> {
    let fault = |f: usum_guest::Fault| format!("vertex data at {address:#x}: {f}");
    Ok(match kind {
        0 => f32::from(mem.read_u8(address).map_err(fault)? as i8),
        1 => f32::from(mem.read_u8(address).map_err(fault)?),
        2 => f32::from(mem.read_u16(address).map_err(fault)? as i16),
        _ => f32::from_bits(mem.read_u32(address).map_err(fault)?),
    })
}

/// One attribute loader of a draw, decoded once: where vertex 0's data
/// is, the stride, the bytes one vertex's data spans, and its fields
/// (attribute, element kind, element count, offset).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Loader {
    start: u32,
    stride: u32,
    span: u32,
    fields: Vec<(usize, u32, u32, u32)>,
}

/// The vertex attribute layout of a draw (GPUREG_ATTRIBBUFFERS_*).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Layout {
    loaders: Vec<Loader>,
    total: usize,
}

fn layout(p: &Pica) -> Result<Layout, String> {
    let base = (reg(p, 0x200) & 0x1FFF_FFFE) << 3;
    let (low, high) = (reg(p, 0x201), reg(p, 0x202));
    let format = |a: usize| -> (u32, u32) {
        let bits = if a < 8 { low >> (4 * a) } else { high >> (4 * (a - 8)) };
        (bits & 3, ((bits >> 2) & 3) + 1)
    };
    let mut loaders = Vec::new();
    for l in 0..12u16 {
        let offset = reg(p, 0x203 + 3 * l) & 0x0FFF_FFFF;
        let (c1, c2) = (reg(p, 0x204 + 3 * l), reg(p, 0x205 + 3 * l));
        let count = c2 >> 28;
        let stride = (c2 >> 16) & 0xFF;
        if count == 0 {
            continue;
        }
        let start = p.pa_to_va(base + offset)?;
        let mut at = 0u32;
        let mut fields = Vec::new();
        for k in 0..count {
            let id = if k < 8 { (c1 >> (4 * k)) & 0xF } else { (c2 >> (4 * (k - 8))) & 0xF };
            if id >= 12 {
                at = at.next_multiple_of(4) + 4 * (id - 11);
                continue;
            }
            let (kind, elements) = format(id as usize);
            let size = [1, 1, 2, 4][kind as usize];
            at = at.next_multiple_of(size);
            fields.push((id as usize, kind, elements, at));
            at += elements * size;
        }
        loaders.push(Loader { start, stride, span: at, fields });
    }
    Ok(Layout { loaders, total: ((high >> 28) + 1) as usize })
}

/// One element of `kind` (0 s8, 1 u8, 2 s16, 3 f32) at `o` in `bytes`.
fn element_in(bytes: &[u8], o: usize, kind: u32) -> f32 {
    match kind {
        0 => f32::from(bytes[o] as i8),
        1 => f32::from(bytes[o]),
        2 => f32::from(i16::from_le_bytes([bytes[o], bytes[o + 1]])),
        _ => f32::from_bits(u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])),
    }
}

/// The shader inputs of vertex `index`: every loader's components, then
/// fixed attributes for the rest, permuted into input registers.
fn load_vertex(p: &Pica, layout: &Layout, mem: &Memory, index: u32) -> Result<[Vec4; 16], String> {
    let mut attributes: [Option<Vec4>; 12] = [None; 12];
    for loader in &layout.loaders {
        let start = loader.start.wrapping_add(index.wrapping_mul(loader.stride));
        // one slice for the vertex's data when it lies in one region;
        // element by element otherwise (and for the fault it reports)
        let bytes = mem.slice(start, loader.span);
        for &(id, kind, elements, at) in &loader.fields {
            let size = [1, 1, 2, 4][kind as usize];
            let mut value = [0.0, 0.0, 0.0, 1.0];
            for (e, slot) in value.iter_mut().enumerate().take(elements as usize) {
                let offset = at + e as u32 * size;
                *slot = match bytes {
                    Some(b) => element_in(b, offset as usize, kind),
                    None => element(mem, start.wrapping_add(offset), kind)?,
                };
            }
            attributes[id] = Some(value);
        }
    }
    let mut inputs = [[0f32; 4]; 16];
    for a in 0..layout.total.min(12) {
        let value = attributes[a].unwrap_or(p.fixed[a]);
        inputs[usize::from(p.vs.permutation[a] & 0xF)] = value;
    }
    Ok(inputs)
}

/// Load and shade in one indexed worker pass. Inputs stay on the worker's
/// stack; ordinary draws retain only mapped vertices, while geometry draws
/// can request the raw registers too. Collection keeps first-error order.
fn process_vertices<T: Send>(
    p: &Pica,
    mem: &Memory,
    layout: &Result<Layout, String>,
    indices: &[u32],
    map: impl Fn([Vec4; 16]) -> T + Sync,
) -> Result<Vec<T>, String> {
    let process = |index: &u32| {
        let input = layout.as_ref().map_err(Clone::clone).and_then(|layout| load_vertex(p, layout, mem, *index));
        match input {
            Ok(input) => {
                let (out, steps) = shader::run_measured(&p.vs, &input);
                (out.map(&map), Some(steps))
            }
            Err(error) => (Err(error), None),
        }
    };
    let processed: Vec<_> = if indices.len() >= 16 {
        use rayon::prelude::*;
        indices.par_iter().map(process).collect()
    } else {
        indices.iter().map(process).collect()
    };
    let (runs, steps) = processed
        .iter()
        .filter_map(|(_, steps)| *steps)
        .fold((0, 0), |(runs, total), steps| (runs + 1, total + u64::from(steps)));
    shader::record_runs(runs, steps);
    processed.into_iter().map(|(result, _)| result).collect()
}

/// The geometry shader over the vertex shader's outputs. Its input buffer
/// holds each vertex's *enabled* output registers in order; one run takes
/// as many vertices as its input count covers. That grouping is a
/// HYPOTHESIS (R-033): 3dbrew only says the input buffer is "the vertex
/// shader output buffer". Returns the emitted triangles' vertices, three
/// per triangle.
fn geometry_stage(p: &Pica, raw: &[[Vec4; 16]]) -> Result<Vec<Vertex>, String> {
    let per_vertex: Vec<usize> = (0..16).filter(|n| p.vs.output_mask & (1 << n) != 0).collect();
    if per_vertex.is_empty() {
        return Ok(Vec::new());
    }
    let inputs = ((reg(p, 0x289) & 0xF) + 1) as usize;
    let group = (inputs / per_vertex.len()).max(1);
    let mut emitter = shader::Emitter::default();
    for vertices in raw.chunks(group) {
        let attributes: Vec<Vec4> = vertices.iter().flat_map(|o| per_vertex.iter().map(|&n| o[n])).collect();
        let mut v = [[0f32; 4]; 16];
        for (a, value) in attributes.iter().take(16).enumerate() {
            v[usize::from(p.gs.permutation[a] & 0xF)] = *value;
        }
        shader::run_with(&p.gs, &v, Some(&mut emitter))?;
    }
    Ok(emitter.triangles.iter().flat_map(|t| t.iter().map(|o| map_outputs(p, p.gs.output_mask, o))).collect())
}

/// Clips a polygon against `inside(v) >= 0` (Sutherland-Hodgman).
fn clip(poly: Vec<Vertex>, plane: impl Fn(&Vertex) -> f32) -> Vec<Vertex> {
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
        let (da, db) = (plane(a), plane(b));
        if da >= 0.0 {
            out.push(*a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(lerp(a, b, t));
        }
    }
    out
}

fn lerp(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let l = |x: f32, y: f32| x + (y - x) * t;
    let mut v = *a;
    for k in 0..4 {
        v.position[k] = l(a.position[k], b.position[k]);
        v.color[k] = l(a.color[k], b.color[k]);
        v.quaternion[k] = l(a.quaternion[k], b.quaternion[k]);
    }
    for k in 0..3 {
        v.view[k] = l(a.view[k], b.view[k]);
    }
    for u in 0..3 {
        for k in 0..3 {
            v.texcoord[u][k] = l(a.texcoord[u][k], b.texcoord[u][k]);
        }
    }
    v
}

/// The framebuffer as the rasterizer sees it.
struct Target {
    color: u32,
    depth: u32,
    width: u32,
    height: u32,
    color_format: u32,
    depth_format: u32,
}

fn compare(op: u32, a: u32, b: u32) -> bool {
    match op {
        0 => false,
        1 => true,
        2 => a == b,
        3 => a != b,
        4 => a < b,
        5 => a <= b,
        6 => a > b,
        _ => a >= b,
    }
}

/// The color buffer's format as a transfer-engine pixel format (3dbrew:
/// color buffer 0 RGBA8, 2 RGB5A1, 3 RGB565, 4 RGBA4).
fn color_buffer_format(f: u32) -> Result<u32, String> {
    match f {
        0 => Ok(0),
        2 => Ok(3),
        3 => Ok(2),
        4 => Ok(4),
        other => Err(format!("color buffer format {other}")),
    }
}

/// What a draw's combiner configuration needs, worked out once per draw:
/// which stages only pass the previous value through (Replace of
/// Previous, plain operands, scale 1), and which textures any stage reads.
struct TevPlan {
    pass: [bool; 6],
    uses: [bool; 3],
    /// Whether a read argument takes fragment lighting (source 1 or 2).
    lit: bool,
    /// Each stage's registers, decoded once per draw: sources, operands,
    /// combiner, the constant color, scale.
    stages: [Stage; 6],
    /// The combiner buffer's initial color and the buffer update bits.
    buffer: Vec4,
    update: u32,
    /// Blending: the function register and the constant color.
    blend: (u32, Vec4),
}

/// One combiner stage's registers, decoded once per draw (3dbrew
/// GPUREG_TEXENVi_*): per argument the source and operand, the RGB and
/// alpha modes, how many arguments each mode reads, the constant color and
/// the scale factors.
#[derive(Clone, Copy)]
struct Stage {
    rgb_source: [u32; 3],
    alpha_source: [u32; 3],
    rgb_operand: [u32; 3],
    alpha_operand: [u32; 3],
    rgb_mode: u32,
    alpha_mode: u32,
    rgb_args: usize,
    alpha_args: usize,
    constant: Vec4,
    rgb_scale: f32,
    alpha_scale: f32,
}

/// Arguments a combiner mode reads (unknown modes act as Replace).
fn mode_args(mode: u32) -> usize {
    match mode {
        1 | 2 | 3 | 5 | 6 | 7 => 2,
        4 | 8 | 9 => 3,
        _ => 1,
    }
}

impl Stage {
    fn new(sources: u32, operands: u32, combiner: u32, constant: u32, scale: u32) -> Stage {
        let factor = |bits: u32| [1.0, 2.0, 4.0, 1.0][bits as usize & 3];
        let (rgb_mode, alpha_mode) = (combiner & 0xF, (combiner >> 16) & 0xF);
        Stage {
            rgb_source: std::array::from_fn(|k| (sources >> (4 * k)) & 0xF),
            alpha_source: std::array::from_fn(|k| (sources >> (16 + 4 * k)) & 0xF),
            rgb_operand: std::array::from_fn(|k| (operands >> (4 * k)) & 0xF),
            alpha_operand: std::array::from_fn(|k| (operands >> (12 + 4 * k)) & 7),
            rgb_mode,
            alpha_mode,
            rgb_args: mode_args(rgb_mode),
            // DOT3_RGBA writes alpha from the RGB dot product
            alpha_args: if rgb_mode == 7 { 0 } else { mode_args(alpha_mode) },
            constant: unpack_color(constant),
            rgb_scale: factor(scale),
            alpha_scale: factor(scale >> 16),
        }
    }
}

/// A packed RGBA8 color register as four values 0..1.
fn unpack_color(c: u32) -> Vec4 {
    [0, 8, 16, 24].map(|s| ((c >> s) & 0xFF) as f32 / 255.0)
}

const STAGES: [u16; 6] = [0xC0, 0xC8, 0xD0, 0xD8, 0xF0, 0xF8];

fn tev_plan(p: &Pica) -> TevPlan {
    let mut plan = TevPlan {
        pass: [false; 6],
        uses: [false; 3],
        lit: false,
        stages: STAGES.map(|s| Stage::new(reg(p, s), reg(p, s + 1), reg(p, s + 2), reg(p, s + 3), reg(p, s + 4))),
        buffer: unpack_color(reg(p, 0xFD)),
        update: reg(p, 0xE0),
        blend: (reg(p, 0x101), unpack_color(reg(p, 0x103))),
    };
    for (i, &s) in STAGES.iter().enumerate() {
        let (sources, operands, combiner, scale) = (reg(p, s), reg(p, s + 1), reg(p, s + 2), reg(p, s + 4));
        // stage 0 reading Previous is special (3dbrew), so never skipped
        plan.pass[i] = i > 0
            && sources & 0xF == 15
            && (sources >> 16) & 0xF == 15
            && combiner & 0xF == 0
            && (combiner >> 16) & 0xF == 0
            && operands & 0xF == 0
            && (operands >> 12) & 7 == 0
            && scale & 3 == 0
            && (scale >> 16) & 3 == 0;
        if !plan.pass[i] {
            // only the arguments the modes read: a texture named by an
            // unused argument cannot change the result
            let stage = plan.stages[i];
            let read = stage.rgb_source[..stage.rgb_args].iter().chain(&stage.alpha_source[..stage.alpha_args]);
            for &source in read {
                if (3..=5).contains(&source) {
                    plan.uses[(source - 3) as usize] = true;
                }
                if matches!(source, 1 | 2) {
                    plan.lit = true;
                }
            }
        }
    }
    plan
}

/// One texture combiner stage's inputs and output (values 0..1).
/// A combiner RGB operand (3dbrew GPUREG_TEXENVi_OPERAND).
#[inline(always)]
fn rgb_operand(v: Vec4, op: u32) -> [f32; 3] {
    let one_minus = |x: f32| 1.0 - x;
    match op {
        0 => [v[0], v[1], v[2]],
        1 => [one_minus(v[0]), one_minus(v[1]), one_minus(v[2])],
        2 => [v[3]; 3],
        3 => [one_minus(v[3]); 3],
        4 => [v[0]; 3],
        5 => [one_minus(v[0]); 3],
        8 => [v[1]; 3],
        9 => [one_minus(v[1]); 3],
        12 => [v[2]; 3],
        13 => [one_minus(v[2]); 3],
        _ => [v[0], v[1], v[2]],
    }
}

/// A combiner alpha operand.
#[inline(always)]
fn alpha_operand(v: Vec4, op: u32) -> f32 {
    let c = [v[3], v[0], v[1], v[2]][(op >> 1) as usize & 3];
    if op & 1 == 1 {
        1.0 - c
    } else {
        c
    }
}

/// A combiner mode on one channel (Dot3 is handled by the caller).
#[inline(always)]
fn apply(mode: u32, a: f32, b: f32, c: f32) -> f32 {
    match mode {
        0 => a,
        1 => a * b,
        2 => a + b,
        3 => a + b - 0.5,
        4 => a * c + b * (1.0 - c),
        5 => a - b,
        8 => a * b + c,
        9 => (a + b).min(1.0) * c,
        _ => a,
    }
}

/// The Dot3 modes' value.
#[inline(always)]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    4.0 * (0..3).map(|k| (a[k] - 0.5) * (b[k] - 0.5)).sum::<f32>()
}

/// The combiner for one fragment: the reference `combine_span` is tested
/// against.
#[cfg(test)]
fn combine(plan: &TevPlan, primary: Vec4, textures: &[Vec4; 3], lit: [Vec4; 2]) -> Vec4 {
    let update = plan.update;
    let mut previous = [0f32; 4];
    let mut buffer = plan.buffer;
    let white = [1.0f32; 4];
    for (i, stage) in plan.stages.iter().enumerate() {
        if plan.pass[i] {
            // the output is the previous value; only the buffer may update
            if (1..5).contains(&i) {
                if update >> (8 + i - 1) & 1 == 1 {
                    buffer[..3].copy_from_slice(&previous[..3]);
                }
                if update >> (12 + i - 1) & 1 == 1 {
                    buffer[3] = previous[3];
                }
            }
            continue;
        }
        let pick = |source: u32| -> Vec4 {
            match source {
                0 => primary,
                1 => lit[0],
                2 => lit[1],
                3..=5 => textures[(source - 3) as usize],
                6 => white,
                13 => buffer,
                14 => stage.constant,
                _ => previous,
            }
        };
        // only the arguments the modes read (the others cannot change the
        // result)
        let mut rgb = [[0f32; 3]; 3];
        for (k, arg) in rgb.iter_mut().enumerate().take(stage.rgb_args) {
            *arg = rgb_operand(pick(stage.rgb_source[k]), stage.rgb_operand[k]);
        }
        let mut alpha = [0f32; 3];
        for (k, arg) in alpha.iter_mut().enumerate().take(stage.alpha_args) {
            *arg = alpha_operand(pick(stage.alpha_source[k]), stage.alpha_operand[k]);
        }
        let (rgb_mode, alpha_mode) = (stage.rgb_mode, stage.alpha_mode);
        let mut out = [0f32; 4];
        if matches!(rgb_mode, 6 | 7) {
            let dot = dot3(rgb[0], rgb[1]);
            out = [dot, dot, dot, if rgb_mode == 7 { dot } else { 0.0 }];
        } else {
            for k in 0..3 {
                out[k] = apply(rgb_mode, rgb[0][k], rgb[1][k], rgb[2][k]);
            }
        }
        if rgb_mode != 7 {
            out[3] = apply(alpha_mode, alpha[0], alpha[1], alpha[2]);
        }
        for c in out.iter_mut().take(3) {
            *c = (*c * stage.rgb_scale).clamp(0.0, 1.0);
        }
        out[3] = (out[3] * stage.alpha_scale).clamp(0.0, 1.0);
        // the buffer the next stage reads (stages 1-4 choose)
        if (1..5).contains(&i) {
            if update >> (8 + i - 1) & 1 == 1 {
                buffer[..3].copy_from_slice(&previous[..3]);
            }
            if update >> (12 + i - 1) & 1 == 1 {
                buffer[3] = previous[3];
            }
        }
        previous = out;
    }
    previous
}

/// Scratch space for the fragments of one span (one triangle's run of a
/// row), reused from span to span within a band: what survives the depth
/// test, its inputs, and the combiner's state per fragment.
#[derive(Default)]
struct Span {
    pixel: Vec<usize>,
    z: Vec<u32>,
    stencil_outcome: Vec<u8>,
    /// Perspective-correct weights.
    weights: Vec<[f32; 3]>,
    color: Vec<Vec4>,
    sampled: [Vec<Vec4>; 3],
    /// Fragment lighting's primary and secondary colors (combiner sources
    /// 1 and 2), zero when lighting is off.
    lit: [Vec<Vec4>; 2],
    previous: Vec<Vec4>,
    buffer: Vec<Vec4>,
    out: Vec<Vec4>,
    rgb: [Vec<[f32; 3]>; 3],
    alpha: [Vec<f32>; 3],
    constant: Vec<Vec4>,
    zeros: Vec<Vec4>,
    whites: Vec<Vec4>,
}

/// `out[f] = op(src[f])` (generic, so each use is its own tight loop).
#[inline(always)]
fn fill<T>(src: &[Vec4], out: &mut [T], op: impl Fn(Vec4) -> T) {
    for (o, v) in out.iter_mut().zip(src) {
        *o = op(*v);
    }
}

/// An RGB operand over a span: one loop per operand, the operand fixed.
fn gather_rgb(src: &[Vec4], out: &mut [[f32; 3]], op: u32) {
    match op {
        0 => fill(src, out, |v| rgb_operand(v, 0)),
        1 => fill(src, out, |v| rgb_operand(v, 1)),
        2 => fill(src, out, |v| rgb_operand(v, 2)),
        3 => fill(src, out, |v| rgb_operand(v, 3)),
        4 => fill(src, out, |v| rgb_operand(v, 4)),
        5 => fill(src, out, |v| rgb_operand(v, 5)),
        8 => fill(src, out, |v| rgb_operand(v, 8)),
        9 => fill(src, out, |v| rgb_operand(v, 9)),
        12 => fill(src, out, |v| rgb_operand(v, 12)),
        13 => fill(src, out, |v| rgb_operand(v, 13)),
        other => fill(src, out, |v| rgb_operand(v, other)),
    }
}

/// An alpha operand over a span.
fn gather_alpha(src: &[Vec4], out: &mut [f32], op: u32) {
    match op & 7 {
        0 => fill(src, out, |v| alpha_operand(v, 0)),
        1 => fill(src, out, |v| alpha_operand(v, 1)),
        2 => fill(src, out, |v| alpha_operand(v, 2)),
        3 => fill(src, out, |v| alpha_operand(v, 3)),
        4 => fill(src, out, |v| alpha_operand(v, 4)),
        5 => fill(src, out, |v| alpha_operand(v, 5)),
        6 => fill(src, out, |v| alpha_operand(v, 6)),
        _ => fill(src, out, |v| alpha_operand(v, 7)),
    }
}

/// A mode on the RGB channels over a span, the mode fixed per loop.
fn apply_rgb(mode: u32, args: &[Vec<[f32; 3]>; 3], out: &mut [Vec4]) {
    #[inline(always)]
    fn each(args: &[Vec<[f32; 3]>; 3], out: &mut [Vec4], f: impl Fn(f32, f32, f32) -> f32) {
        for (i, o) in out.iter_mut().enumerate() {
            for k in 0..3 {
                o[k] = f(args[0][i][k], args[1][i][k], args[2][i][k]);
            }
        }
    }
    match mode {
        0 => each(args, out, |a, b, c| apply(0, a, b, c)),
        1 => each(args, out, |a, b, c| apply(1, a, b, c)),
        2 => each(args, out, |a, b, c| apply(2, a, b, c)),
        3 => each(args, out, |a, b, c| apply(3, a, b, c)),
        4 => each(args, out, |a, b, c| apply(4, a, b, c)),
        5 => each(args, out, |a, b, c| apply(5, a, b, c)),
        8 => each(args, out, |a, b, c| apply(8, a, b, c)),
        9 => each(args, out, |a, b, c| apply(9, a, b, c)),
        other => each(args, out, |a, b, c| apply(other, a, b, c)),
    }
}

/// A mode on alpha over a span.
fn apply_alpha(mode: u32, args: &[Vec<f32>; 3], out: &mut [Vec4]) {
    #[inline(always)]
    fn each(args: &[Vec<f32>; 3], out: &mut [Vec4], f: impl Fn(f32, f32, f32) -> f32) {
        for (i, o) in out.iter_mut().enumerate() {
            o[3] = f(args[0][i], args[1][i], args[2][i]);
        }
    }
    match mode {
        0 => each(args, out, |a, b, c| apply(0, a, b, c)),
        1 => each(args, out, |a, b, c| apply(1, a, b, c)),
        2 => each(args, out, |a, b, c| apply(2, a, b, c)),
        3 => each(args, out, |a, b, c| apply(3, a, b, c)),
        4 => each(args, out, |a, b, c| apply(4, a, b, c)),
        5 => each(args, out, |a, b, c| apply(5, a, b, c)),
        8 => each(args, out, |a, b, c| apply(8, a, b, c)),
        9 => each(args, out, |a, b, c| apply(9, a, b, c)),
        other => each(args, out, |a, b, c| apply(other, a, b, c)),
    }
}

/// The combiner over the span's `n` fragments, stage by stage: each
/// stage's sources, operands and modes are chosen once for the span, not
/// per fragment. Per fragment the arithmetic is `combine`'s, in the same
/// order, so the result is identical (tested). Leaves the output in
/// `span.previous`.
fn combine_span(plan: &TevPlan, span: &mut Span, n: usize) {
    let Span { color, sampled, lit, previous, buffer, out, rgb, alpha, constant, zeros, whites, .. } = span;
    previous.clear();
    previous.resize(n, [0.0; 4]);
    buffer.clear();
    buffer.resize(n, plan.buffer);
    out.resize(n, [0.0; 4]);
    for v in rgb.iter_mut() {
        v.resize(n, [0.0; 3]);
    }
    for v in alpha.iter_mut() {
        v.resize(n, 0.0);
    }
    if zeros.len() < n {
        zeros.resize(n, [0.0; 4]);
        whites.resize(n, [1.0; 4]);
    }
    let update = plan.update;
    let update_buffer = |i: usize, buffer: &mut [Vec4], previous: &[Vec4]| {
        if (1..5).contains(&i) {
            let (rgb, a) = (update >> (8 + i - 1) & 1 == 1, update >> (12 + i - 1) & 1 == 1);
            for (b, p) in buffer.iter_mut().zip(previous) {
                if rgb {
                    b[..3].copy_from_slice(&p[..3]);
                }
                if a {
                    b[3] = p[3];
                }
            }
        }
    };
    for (i, stage) in plan.stages.iter().enumerate() {
        if plan.pass[i] {
            update_buffer(i, buffer, previous);
            continue;
        }
        constant.clear();
        constant.resize(n, stage.constant);
        let source = |s: u32| -> &[Vec4] {
            match s {
                0 => &color[..n],
                1 => &lit[0][..n],
                2 => &lit[1][..n],
                3..=5 => &sampled[(s - 3) as usize][..n],
                6 => &whites[..n],
                13 => &buffer[..n],
                14 => &constant[..n],
                _ => &previous[..n],
            }
        };
        for (k, arg) in rgb.iter_mut().enumerate().take(stage.rgb_args) {
            gather_rgb(source(stage.rgb_source[k]), arg, stage.rgb_operand[k]);
        }
        for (k, arg) in alpha.iter_mut().enumerate().take(stage.alpha_args) {
            gather_alpha(source(stage.alpha_source[k]), arg, stage.alpha_operand[k]);
        }
        let (rgb_mode, alpha_mode) = (stage.rgb_mode, stage.alpha_mode);
        if matches!(rgb_mode, 6 | 7) {
            for (f, o) in out.iter_mut().enumerate() {
                let dot = dot3(rgb[0][f], rgb[1][f]);
                *o = [dot, dot, dot, if rgb_mode == 7 { dot } else { 0.0 }];
            }
        } else {
            apply_rgb(rgb_mode, rgb, out);
        }
        if rgb_mode != 7 {
            apply_alpha(alpha_mode, alpha, out);
        }
        for o in out.iter_mut() {
            for c in o.iter_mut().take(3) {
                *c = (*c * stage.rgb_scale).clamp(0.0, 1.0);
            }
            o[3] = (o[3] * stage.alpha_scale).clamp(0.0, 1.0);
        }
        update_buffer(i, buffer, previous);
        std::mem::swap(previous, out);
    }
}

fn blend(plan: &TevPlan, src: Vec4, dst: Vec4) -> Vec4 {
    let (f, constant) = plan.blend;
    let factor = |func: u32, k: usize| -> f32 {
        match func {
            0 => 0.0,
            1 => 1.0,
            2 => src[k],
            3 => 1.0 - src[k],
            4 => dst[k],
            5 => 1.0 - dst[k],
            6 => src[3],
            7 => 1.0 - src[3],
            8 => dst[3],
            9 => 1.0 - dst[3],
            10 => constant[k],
            11 => 1.0 - constant[k],
            12 => constant[3],
            13 => 1.0 - constant[3],
            _ => {
                if k == 3 {
                    1.0
                } else {
                    src[3].min(1.0 - dst[3])
                }
            }
        }
    };
    let equation = |eq: u32, s: f32, d: f32, sf: f32, df: f32| -> f32 {
        match eq {
            1 => s * sf - d * df,
            2 => d * df - s * sf,
            3 => s.min(d),
            4 => s.max(d),
            _ => s * sf + d * df,
        }
    };
    let mut out = [0f32; 4];
    for k in 0..3 {
        out[k] = equation(f & 7, src[k], dst[k], factor((f >> 16) & 0xF, k), factor((f >> 20) & 0xF, k)).clamp(0.0, 1.0);
    }
    out[3] = equation((f >> 8) & 7, src[3], dst[3], factor((f >> 24) & 0xF, 3), factor((f >> 28) & 0xF, 3)).clamp(0.0, 1.0);
    out
}

/// A triangle in window coordinates (x, y, depth, 1/w per vertex),
/// ready to rasterize.
struct Prepared {
    s: [[f32; 4]; 3],
    area: f32,
    v: [Vertex; 3],
    /// Coverage in fixed point (R-049): per edge (v1→v2, v2→v0, v0→v1)
    /// its start and direction in 1/16 pixels, signed so the inside is
    /// positive, and whether a pixel exactly on it belongs to this
    /// triangle (the top-left rule); and twice the area, positive.
    edges: [Edge; 3],
    area_fixed: i64,
}

/// One edge in 1/16 pixels: E(p) = dx (py - ay) - dy (px - ax), times
/// the orientation's sign so the inside is positive.
#[derive(Clone, Copy)]
struct Edge {
    ax: i64,
    ay: i64,
    dx: i64,
    dy: i64,
    sign: i64,
    owns_zero: bool,
}

impl Edge {
    /// The edge function at a pixel centre (x + 0.5, y + 0.5).
    #[inline(always)]
    fn at(&self, x: u32, y: u32) -> i64 {
        let (px, py) = (i64::from(x) * 16 + 8, i64::from(y) * 16 + 8);
        (self.dx * (py - self.ay) - self.dy * (px - self.ax)) * self.sign
    }

    /// Whether a pixel with this edge value is on the inside.
    #[inline(always)]
    fn covers(&self, e: i64) -> bool {
        e > 0 || (e == 0 && self.owns_zero)
    }
}

/// A triangle's edges in fixed point, and twice its area (positive); None
/// when it has no area once snapped.
fn coverage(xy: [[f32; 2]; 3]) -> Option<([Edge; 3], i64)> {
    let f = xy.map(|[x, y]| (fixed(x), fixed(y)));
    let area = (f[1].0 - f[0].0) * (f[2].1 - f[0].1) - (f[2].0 - f[0].0) * (f[1].1 - f[0].1);
    if area == 0 {
        return None;
    }
    let sign = area.signum();
    let edge = |a: (i64, i64), b: (i64, i64)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        // E = A px + B py + C with A = -dy, B = dx (times the sign): a
        // shared edge has opposite (A, B) in its two triangles, so exactly
        // one of them owns pixels exactly on it
        let (a_coef, b_coef) = (-dy * sign, dx * sign);
        Edge { ax: a.0, ay: a.1, dx, dy, sign, owns_zero: a_coef > 0 || (a_coef == 0 && b_coef > 0) }
    };
    Some(([edge(f[1], f[2]), edge(f[2], f[0]), edge(f[0], f[1])], area.abs()))
}

#[inline]
fn map_depth(window_depth: f32, inv_w: f32, scale: f32, offset: f32, w_buffer: bool) -> f32 {
    let mapped = -window_depth * scale + offset;
    (if w_buffer { mapped / inv_w } else { mapped }).clamp(0.0, 1.0)
}

/// A window coordinate in 1/16 pixels (4 fractional bits), kept well
/// inside i64 for points far outside the screen.
fn fixed(c: f32) -> i64 {
    (c * 16.0).round().clamp(-(1 << 30) as f32, (1 << 30) as f32) as i64
}

/// Window coordinates and culling for one clipped triangle.
fn prepare(p: &Pica, v: [&Vertex; 3]) -> Option<Prepared> {
    let half_w = shader::f24(reg(p, 0x41));
    let half_h = shader::f24(reg(p, 0x43));
    let xy = reg(p, 0x68);
    let (ox, oy) = ((((xy & 0x3FF) << 22) as i32 >> 22) as f32, ((((xy >> 16) & 0x3FF) << 22) as i32 >> 22) as f32);
    let s = v.map(|v| {
        let iw = 1.0 / v.position[3];
        [(v.position[0] * iw + 1.0) * half_w + ox, (v.position[1] * iw + 1.0) * half_h + oy, -v.position[2] * iw, iw]
    });
    let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1]) - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
    if area == 0.0 || !area.is_finite() {
        return None;
    }
    // culling: 1 front (counter-clockwise), 2 back
    match reg(p, 0x40) & 3 {
        1 if area > 0.0 => return None,
        2 if area < 0.0 => return None,
        _ => {}
    }
    // coverage in fixed point: pixels on an edge shared by two triangles
    // belong to exactly one of them (watertight), which floating-point
    // edge tests evaluated per triangle do not guarantee (R-049)
    let (edges, area_fixed) = coverage(s.map(|s| [s[0], s[1]]))?;
    Some(Prepared { s, area, v: v.map(|v| *v), edges, area_fixed })
}

/// A piece of the target one thread rasterizes: whole tile rows, or one
/// tile row's run of tile columns (`first_col`, `cols` in pixels).
struct Band<'a> {
    first_row: u32,
    rows: u32,
    first_col: u32,
    cols: u32,
    color: &'a mut [u8],
    depth: &'a mut [u8],
}

/// CPU time spent inside raster bands, summed over threads (ns), for the
/// probe's report: against the bands phase's wall time it says how well
/// the bands are spread over the cores.
pub static BAND_CPU_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Draws with fragment lighting enabled (GPUREG_LIGHTING_ENABLE0 bit 0),
/// and draws whose combiners read the lighting results (sources 1, 2) or
/// texture 3 (source 6), none of which are emulated yet: for the
/// probe's report.
pub static LIT_DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static LIGHTING_READ_DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static TEXTURE3_DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Fragments shaded (past the depth test), and of those, rejected by the
/// alpha test: for the probe's report.
pub static SHADED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static ALPHA_REJECTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn timed_band(
    p: &Pica,
    (textures, lighting): (&[Option<super::texture::Bound>; 3], Option<&super::lighting::Lighting>),
    t: &Target,
    tris: &[Prepared],
    band: Band,
) -> u64 {
    let started = std::time::Instant::now();
    let fragments = raster_band(p, (textures, lighting), t, tris, band);
    BAND_CPU_NS.fetch_add(started.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    fragments
}

/// Rasterizes `tris` into one band; returns the fragments written.
fn raster_band(
    p: &Pica,
    (textures, lighting): (&[Option<super::texture::Bound>; 3], Option<&super::lighting::Lighting>),
    t: &Target,
    tris: &[Prepared],
    band: Band,
) -> u64 {
    let plan = tev_plan(p);
    let (cbpp, dbpp) = (bytes_per_pixel(t.color_format).unwrap_or(4), depth_bytes(t.depth_format));
    let depth_mask = reg(p, 0x107);
    let alpha_test = reg(p, 0x104);
    let blending = reg(p, 0x100) >> 8 & 1 == 1;
    let (color_write, depth_write) = (reg(p, 0x113) & 0xF != 0, reg(p, 0x115) & 2 != 0 && depth_mask >> 12 & 1 == 1);
    let depth_test = depth_mask & 1 == 1;
    let has_depth = !band.depth.is_empty();
    let stencil = stencil::Stencil::new(reg(p, 0x105), reg(p, 0x106), reg(p, 0x115), t.depth_format);
    let stencil_enabled = has_depth && stencil.enabled;
    let depth_scale = shader::f24(reg(p, 0x4d));
    let depth_offset = shader::f24(reg(p, 0x4e));
    let depth_w_buffer = reg(p, 0x6d) & 1 == 0;
    let unit2_coords = if reg(p, 0x80) >> 13 & 1 == 1 { 1 } else { 2 };
    let max_depth = if t.depth_format == 0 { 0xFFFF } else { 0xFF_FFFF } as f32;
    // a band's memory starts at its first tile: tile rows hold tiles left
    // to right, 64 pixels each
    let base_offset = (band.first_row * t.width + band.first_col * 8) as usize;
    let (x_first, x_end) = (band.first_col, band.first_col + band.cols);
    // window rows this band covers (memory row = height - 1 - y)
    let y_low = t.height - band.first_row - band.rows;
    let y_high = t.height - band.first_row;
    let mut fragments = 0;
    let (mut shaded, mut rejected) = (0u64, 0u64);
    let mut span = Span::default();
    for tri in tris {
        let s = &tri.s;
        let v = &tri.v;
        let min_x = (s.iter().map(|s| s[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32).max(x_first);
        let max_x = (s.iter().map(|s| s[0]).fold(f32::NEG_INFINITY, f32::max).ceil().min(t.width as f32) as u32).min(x_end);
        if min_x >= max_x {
            continue;
        }
        let min_y = (s.iter().map(|s| s[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32).max(y_low);
        let max_y = (s.iter().map(|s| s[1]).fold(f32::NEG_INFINITY, f32::max).ceil().min(t.height as f32) as u32).min(y_high);
        // each edge's weight as a function of x along a row: w(x) = w(x0)
        // + slope (x - x0); the row's span is where all three are >= 0,
        // widened by a pixel and then tested exactly per pixel
        let slope = |a: &[f32; 4], b: &[f32; 4]| -(b[1] - a[1]) / tri.area;
        let slopes = [slope(&s[1], &s[2]), slope(&s[2], &s[0]), slope(&s[0], &s[1])];
        for y in min_y..max_y {
            let py = y as f32 + 0.5;
            let x0 = min_x as f32 + 0.5;
            let at_x0 = |a: &[f32; 4], b: &[f32; 4]| ((b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (x0 - a[0])) / tri.area;
            let w_x0 = [at_x0(&s[1], &s[2]), at_x0(&s[2], &s[0]), at_x0(&s[0], &s[1])];
            let (mut lo, mut hi) = (min_x as f32, max_x as f32);
            let mut empty = false;
            for (w, k) in w_x0.iter().zip(slopes) {
                if k > 0.0 {
                    lo = lo.max(x0 - w / k - 1.0);
                } else if k < 0.0 {
                    hi = hi.min(x0 - w / k + 1.0);
                } else if *w < 0.0 {
                    empty = true;
                }
            }
            if empty || lo.is_nan() || hi.is_nan() || lo > hi {
                continue;
            }
            let (x_lo, x_hi) = (lo.floor().max(min_x as f32) as u32, (hi.ceil() + 1.0).min(max_x as f32) as u32);
            // 1. coverage and fragment tests. A failing test can still write
            // stencil, but alpha rejection must suppress that write. Shade
            // those failures too; keep the early skip when nothing changes.
            // Pixels of a span are distinct, so the passes below see the
            // same stored values a pixel-at-a-time loop would (R-045)
            span.pixel.clear();
            span.z.clear();
            span.stencil_outcome.clear();
            span.weights.clear();
            for x in x_lo..x_hi {
                let e = tri.edges.map(|edge| edge.at(x, y));
                if !(tri.edges[0].covers(e[0]) && tri.edges[1].covers(e[1]) && tri.edges[2].covers(e[2])) {
                    continue;
                }
                let area = tri.area_fixed as f32;
                let (w0, w1, w2) = (e[0] as f32 / area, e[1] as f32 / area, e[2] as f32 / area);
                let window_depth = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                let inv_w = w0 * s[0][3] + w1 * s[1][3] + w2 * s[2][3];
                let depth = map_depth(window_depth, inv_w, depth_scale, depth_offset, depth_w_buffer);
                // the buffers are 8x8 tiles; row 0 is the framebuffer's bottom
                let row = t.height - 1 - y;
                let pixel = tiled_offset(x, row, t.width) as usize - base_offset;
                let z = (depth * max_depth) as u32;
                let d_at = pixel * dbpp;
                let mut outcome = 2;
                if stencil_enabled && !stencil.passes(band.depth[d_at + 3]) {
                    outcome = 0;
                }
                if outcome == 2 && has_depth && depth_test {
                    let stored = &band.depth[d_at..d_at + dbpp.min(3)];
                    let old = stored.iter().rev().fold(0u32, |a, &b| a << 8 | u32::from(b));
                    if !compare((depth_mask >> 4) & 7, z, old) {
                        outcome = 1;
                    }
                }
                if outcome != 2 && (!stencil_enabled || !stencil.changes(outcome)) {
                    continue;
                }
                // perspective-correct weights
                let iw = w0 * s[0][3] + w1 * s[1][3] + w2 * s[2][3];
                span.pixel.push(pixel);
                span.z.push(z);
                if stencil_enabled {
                    span.stencil_outcome.push(outcome);
                }
                span.weights.push([w0 * s[0][3] / iw, w1 * s[1][3] / iw, w2 * s[2][3] / iw]);
            }
            let n = span.pixel.len();
            if n == 0 {
                continue;
            }
            // 2. the inputs: primary color, and each texture the combiners read
            span.color.clear();
            span.color.extend(span.weights.iter().map(|&[b0, b1, b2]| {
                super::arr4(|k| (b0 * v[0].color[k] + b1 * v[1].color[k] + b2 * v[2].color[k]).clamp(0.0, 1.0))
            }));
            for (unit, texture) in textures.iter().enumerate() {
                if !plan.uses[unit] {
                    continue;
                }
                let out = &mut span.sampled[unit];
                out.clear();
                match texture {
                    Some(texture) => {
                        let k = if unit == 2 { unit2_coords } else { unit };
                        out.extend(span.weights.iter().map(|&[b0, b1, b2]| {
                            let u = b0 * v[0].texcoord[k][0] + b1 * v[1].texcoord[k][0] + b2 * v[2].texcoord[k][0];
                            let w = b0 * v[0].texcoord[k][1] + b1 * v[1].texcoord[k][1] + b2 * v[2].texcoord[k][1];
                            texture.sample(u, w)
                        }));
                    }
                    None => out.resize(n, [0.0, 0.0, 0.0, 1.0]),
                }
            }
            // fragment lighting, when a combiner reads it (sources 1, 2)
            if plan.lit {
                for l in span.lit.iter_mut() {
                    l.clear();
                }
                match lighting {
                    Some(lighting) => {
                        for &[b0, b1, b2] in &span.weights {
                            let q = super::arr4(|k| b0 * v[0].quaternion[k] + b1 * v[1].quaternion[k] + b2 * v[2].quaternion[k]);
                            let view = [0, 1, 2].map(|k| b0 * v[0].view[k] + b1 * v[1].view[k] + b2 * v[2].view[k]);
                            let (primary, secondary) = lighting.shade(q, view);
                            span.lit[0].push(primary);
                            span.lit[1].push(secondary);
                        }
                    }
                    None => {
                        for l in span.lit.iter_mut() {
                            l.resize(n, [0.0; 4]);
                        }
                    }
                }
            }
            // 3. the combiner, stage by stage over the span
            combine_span(&plan, &mut span, n);
            // 4. alpha test, blending, writes, per fragment
            for f in 0..n {
                let out = span.previous[f];
                let (pixel, z) = (span.pixel[f], span.z[f]);
                let d_at = pixel * dbpp;
                shaded += 1;
                if alpha_test & 1 == 1 && !compare((alpha_test >> 4) & 7, (out[3] * 255.0) as u32, (alpha_test >> 8) & 0xFF) {
                    rejected += 1;
                    continue;
                }
                if stencil_enabled {
                    let outcome = span.stencil_outcome[f];
                    if stencil.changes(outcome) {
                        band.depth[d_at + 3] = stencil.apply(outcome, band.depth[d_at + 3]);
                    }
                    if outcome != 2 {
                        continue;
                    }
                }
                let c_at = pixel * cbpp;
                // the stored color matters only to blending and to masked
                // channels
                let needs_dst = blending || depth_mask >> 8 & 0xF != 0xF;
                let dst = if needs_dst {
                    let [r, g, b, a] = decode_pixel(t.color_format, &band.color[c_at..c_at + cbpp]);
                    let unit = &super::texture::UNIT;
                    [unit[usize::from(r)], unit[usize::from(g)], unit[usize::from(b)], unit[usize::from(a)]]
                } else {
                    [0.0; 4]
                };
                let result = if blending { blend(&plan, out, dst) } else { out };
                let mut rgba = [0u8; 4];
                for k in 0..4 {
                    let keep = depth_mask >> (8 + k) & 1 == 0;
                    rgba[k] = ((if keep { dst[k] } else { result[k] }) * 255.0 + 0.5) as u8;
                }
                if color_write {
                    band.color[c_at..c_at + cbpp].copy_from_slice(&encode_pixel(t.color_format, rgba));
                }
                if has_depth && depth_write {
                    let n = dbpp.min(3);
                    band.depth[d_at..d_at + n].copy_from_slice(&z.to_le_bytes()[..n]);
                }
                fragments += 1;
            }
        }
    }
    SHADED.fetch_add(shaded, std::sync::atomic::Ordering::Relaxed);
    ALPHA_REJECTED.fetch_add(rejected, std::sync::atomic::Ordering::Relaxed);
    fragments
}

/// Bytes per depth buffer pixel (3dbrew: 0 16-bit, 2 24-bit, 3 24+8).
fn depth_bytes(format: u32) -> usize {
    match format {
        0 => 2,
        2 => 3,
        _ => 4,
    }
}

/// Tile columns per band when a tile row is split (64 pixels).
const COLUMN_TILES: u32 = 8;

/// Rasterizes the prepared triangles of one draw: the color and depth
/// buffers are copied out of guest memory, rasterized in bands (whole
/// tile rows, or runs of one row's tile columns) on the worker pool (each
/// band sees every triangle in draw order, so per-pixel order is kept),
/// and written back.
fn rasterize(p: &mut Pica, mem: &mut Memory, t: &Target, tris: &[Prepared]) -> Result<(), String> {
    if tris.is_empty() {
        return Ok(());
    }
    let cbpp = bytes_per_pixel(t.color_format).unwrap_or(4) as u32;
    let dbpp = depth_bytes(t.depth_format) as u32;
    // only the tile rows the triangles can touch are copied out and back
    let y_min = tris.iter().flat_map(|t| t.s.iter().map(|s| s[1])).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
    let y_max =
        tris.iter().flat_map(|t| t.s.iter().map(|s| s[1])).fold(f32::NEG_INFINITY, f32::max).ceil().min(t.height as f32) as u32;
    if y_min >= y_max {
        return Ok(());
    }
    // memory rows (counted from the top) and their whole tile rows
    let (row_lo, row_hi) = (t.height - y_max, t.height - y_min);
    let (tile_lo, tile_hi) = (row_lo / 8, row_hi.div_ceil(8).min(t.height / 8));
    if tile_lo >= tile_hi {
        return Ok(());
    }
    let row_pixels = 8 * t.width;
    let phase = std::time::Instant::now();
    let color_at = p.pa_to_va(t.color)? + tile_lo * row_pixels * cbpp;
    let span = (tile_hi - tile_lo) * row_pixels;
    let mut color = mem.slice(color_at, span * cbpp).ok_or("color buffer not mapped")?.to_vec();
    let depth_used =
        t.depth != 0 && (reg(p, 0x107) & 1 == 1 || reg(p, 0x115) & 2 != 0 || (t.depth_format == 3 && reg(p, 0x105) & 1 != 0));
    let depth_at = if depth_used { Some(p.pa_to_va(t.depth)? + tile_lo * row_pixels * dbpp) } else { None };
    let mut depth = match depth_at {
        Some(at) => mem.slice(at, span * dbpp).ok_or("depth buffer not mapped")?.to_vec(),
        None => Vec::new(),
    };
    p.phases[1] += phase.elapsed();
    let phase = std::time::Instant::now();
    // textures: decoded (or found current in the cache) for the units the
    // combiners read
    let plan = tev_plan(p);
    {
        let reads = |wanted: &[u32]| {
            plan.stages.iter().zip(plan.pass).any(|(st, pass)| {
                !pass
                    && (st.rgb_source[..st.rgb_args].iter().any(|s| wanted.contains(s))
                        || st.alpha_source[..st.alpha_args].iter().any(|s| wanted.contains(s)))
            })
        };
        use std::sync::atomic::Ordering::Relaxed;
        if reg(p, 0x8F) & 1 == 1 {
            LIT_DRAWS.fetch_add(1, Relaxed);
        }
        if reads(&[1, 2]) {
            LIGHTING_READ_DRAWS.fetch_add(1, Relaxed);
        }
        if reads(&[6]) {
            TEXTURE3_DRAWS.fetch_add(1, Relaxed);
        }
    }
    let mut cache = std::mem::take(&mut p.texture_cache);
    for unit in 0..3 {
        if plan.uses[unit] {
            cache.prepare(p, mem, unit);
        }
    }
    let lighting = if plan.lit { super::lighting::Lighting::new(p) } else { None };
    let textures: [Option<super::texture::Bound>; 3] =
        std::array::from_fn(|u| if plan.uses[u] { super::texture::Bound::new(p, &cache, u) } else { None });
    p.phases[2] += phase.elapsed();
    let phase = std::time::Instant::now();
    let tile_rows = tile_hi - tile_lo;
    let area: f32 = tris.iter().map(|t| t.area.abs() / 2.0).sum();
    let bands = if area < 1024.0 { 1 } else { tile_rows.max(1) };
    let per_band = tile_rows.div_ceil(bands);
    let row_color = (8 * t.width * cbpp) as usize;
    let row_depth = (8 * t.width * dbpp) as usize;
    let pr: &Pica = p;
    let mut work = Vec::with_capacity(bands as usize);
    let mut color_rest: &mut [u8] = &mut color;
    let mut depth_rest: &mut [u8] = &mut depth;
    for b in 0..bands {
        let first = b * per_band;
        if first >= tile_rows {
            break;
        }
        let rows = per_band.min(tile_rows - first);
        let (c, cr) = std::mem::take(&mut color_rest).split_at_mut(rows as usize * row_color);
        color_rest = cr;
        let d = if depth_rest.is_empty() {
            &mut [][..]
        } else {
            let (d, dr) = std::mem::take(&mut depth_rest).split_at_mut(rows as usize * row_depth);
            depth_rest = dr;
            d
        };
        // one tile row splits further into runs of tile columns, contiguous
        // in memory, so busy rows spread over more threads
        let tiles = t.width / 8;
        if bands > 1 && rows == 1 && tiles > COLUMN_TILES {
            let (mut c, mut d) = (c, d);
            let mut col = 0;
            while col < tiles {
                let n = COLUMN_TILES.min(tiles - col);
                let (cc, cr) = std::mem::take(&mut c).split_at_mut((n * 64 * cbpp) as usize);
                c = cr;
                let dd = if d.is_empty() {
                    &mut [][..]
                } else {
                    let (dd, dr) = std::mem::take(&mut d).split_at_mut((n * 64 * dbpp) as usize);
                    d = dr;
                    dd
                };
                let first_row = (tile_lo + first) * 8;
                work.push(Band { first_row, rows: 8, first_col: col * 8, cols: n * 8, color: cc, depth: dd });
                col += n;
            }
            continue;
        }
        work.push(Band { first_row: (tile_lo + first) * 8, rows: rows * 8, first_col: 0, cols: t.width, color: c, depth: d });
    }
    // bands run on the shared worker pool (rayon): no thread is started
    // per draw
    let fragments: u64 = if work.len() == 1 {
        work.into_iter().map(|band| timed_band(pr, (&textures, lighting.as_ref()), t, tris, band)).sum()
    } else {
        use rayon::prelude::*;
        work.into_par_iter().map(|band| timed_band(pr, (&textures, lighting.as_ref()), t, tris, band)).sum()
    };
    p.phases[3] += phase.elapsed();
    let phase = std::time::Instant::now();
    p.texture_cache = cache;
    p.fragments += fragments;
    mem.device_write(color_at, &color).map_err(|f| format!("color buffer: {f}"))?;
    if let Some(at) = depth_at {
        mem.device_write(at, &depth).map_err(|f| format!("depth buffer: {f}"))?;
    }
    p.phases[4] += phase.elapsed();
    Ok(())
}

/// GPUREG_DRAWARRAYS (`elements` false) or GPUREG_DRAWELEMENTS.
pub fn draw(p: &mut Pica, mem: &mut Memory, elements: bool) -> Result<(), String> {
    let started = std::time::Instant::now();
    let result = draw_inner(p, mem, elements);
    p.draw_time += started.elapsed();
    result
}

fn draw_inner(p: &mut Pica, mem: &mut Memory, elements: bool) -> Result<(), String> {
    let geometry = reg(p, 0x229) & 3 == 2;
    let count = reg(p, 0x228);
    if count == 0 || reg(p, 0x11D) & 0x0FFF_FFFF == 0 || reg(p, 0x11E) & 0x7FF == 0 {
        return Ok(());
    }
    let first = reg(p, 0x22A);
    let index_config = reg(p, 0x227);
    let base = (reg(p, 0x200) & 0x1FFF_FFFE) << 3;
    // the indices, then each distinct vertex once (in order of first
    // use): loaded serially from memory, shaded in parallel when there are
    // enough. The first error in that order is the one reported, as when
    // vertices were processed one by one
    let phase = std::time::Instant::now();
    let mut indices = Vec::with_capacity(count as usize);
    // the index buffer in one piece when it lies in one region
    let wide = index_config >> 31 == 1;
    let buffer = if elements {
        let at = p.pa_to_va(base + (index_config & 0x0FFF_FFFF))?;
        mem.slice(at, count * if wide { 2 } else { 1 })
    } else {
        None
    };
    if let Some(bytes) = buffer {
        if wide {
            indices.extend(bytes.as_chunks::<2>().0.iter().map(|b| u32::from(u16::from_le_bytes(*b))));
        } else {
            indices.extend(bytes.iter().map(|&b| u32::from(b)));
        }
    }
    for i in (indices.len() as u32)..count {
        indices.push(if elements {
            let at = p.pa_to_va(base + (index_config & 0x0FFF_FFFF))?;
            let fault = |f: usum_guest::Fault| format!("index buffer: {f}");
            if index_config >> 31 == 1 {
                u32::from(mem.read_u16(at + 2 * i).map_err(fault)?)
            } else {
                u32::from(mem.read_u8(at + i).map_err(fault)?)
            }
        } else {
            first + i
        });
    }
    // each distinct index's slot, in order of first use: an array over
    // the indices' range (they are 8- or 16-bit, or a run from `first`)
    let low = indices.iter().copied().min().unwrap_or(0);
    let span = indices.iter().copied().max().map_or(0, |high| (high - low) as usize + 1);
    let mut slot_of = vec![u32::MAX; span];
    let mut distinct = Vec::new();
    let mut slots = Vec::with_capacity(indices.len());
    for &index in &indices {
        let slot = &mut slot_of[(index - low) as usize];
        if *slot == u32::MAX {
            *slot = distinct.len() as u32;
            distinct.push(index);
        }
        slots.push(*slot as usize);
    }
    p.phases[5] += phase.elapsed();
    let processing = std::time::Instant::now();
    let layout = layout(p);
    #[cfg(feature = "vulkan-compare")]
    if p.vulkan_vertex && p.vulkan_skip_software {
        if geometry {
            p.vulkan_vertex_counts[2] += 1;
        } else {
            let mode = match (reg(p, 0x25E) >> 8) & 3 {
                3 if reg(p, 0x229) >> 8 & 1 != 0 => 0,
                value => value,
            };
            let program = p.vs.gpu_program().and_then(|program| {
                program.validate_uniforms(&p.vs.ints.map(u32::from_le_bytes))?;
                Ok(program)
            });
            match program {
                Ok(program) if mode <= 2 && slots.len() >= 3 => {
                    let triangles: Vec<[usize; 3]> = match mode {
                        0 => (0..slots.len() / 3).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect(),
                        1 => {
                            (0..slots.len() - 2).map(|t| if t % 2 == 0 { [t, t + 1, t + 2] } else { [t + 1, t, t + 2] }).collect()
                        }
                        _ => (1..slots.len() - 1).map(|t| [0, t, t + 1]).collect(),
                    };
                    let plan = if p.vulkan_input_plan && distinct.len() >= 16 {
                        layout.as_ref().ok().map(|layout| input::InputPlan::new(p, layout))
                    } else {
                        None
                    };
                    let cached = if p.vulkan_input_cache_enabled {
                        plan.as_ref().and_then(|plan| p.vulkan_input_cache.get(plan, mem, &distinct))
                    } else {
                        None
                    };
                    let inputs = match cached {
                        Some(inputs) => inputs,
                        None => {
                            let load = |index: &u32| match &plan {
                                Some(plan) => plan.load(mem, *index),
                                None => load_vertex(p, layout.as_ref().map_err(Clone::clone)?, mem, *index),
                            };
                            let inputs: Vec<Result<[Vec4; 16], String>> = if distinct.len() >= input_parallel_min() {
                                use rayon::prelude::*;
                                distinct.par_iter().with_min_len(64).map(load).collect()
                            } else {
                                distinct.iter().map(load).collect()
                            };
                            let inputs = std::sync::Arc::new(inputs.into_iter().collect::<Result<Vec<_>, _>>()?);
                            if p.vulkan_input_cache_enabled {
                                if let Some(plan) = &plan {
                                    p.vulkan_input_cache.insert(plan, mem, &distinct, &inputs);
                                }
                            }
                            inputs
                        }
                    };
                    let indices = triangles.iter().flatten().map(|&index| slots[index] as u32).collect();
                    let dim = reg(p, 0x11E);
                    let target = super::vulkan_compare::CaptureTarget {
                        color_address: (reg(p, 0x11D) & 0x0FFF_FFFF) << 3,
                        depth_address: (reg(p, 0x11C) & 0x0FFF_FFFF) << 3,
                        width: dim & 0x7FF,
                        height: ((dim >> 12) & 0x3FF) + 1,
                        color_format: color_buffer_format((reg(p, 0x117) >> 16) & 7)?,
                        depth_format: reg(p, 0x116) & 3,
                    };
                    super::vulkan_compare::capture_gpu_vertices(p, mem, target, program, inputs, indices)?;
                    if plan.is_some() {
                        p.vulkan_input_plan_counts[0] += 1;
                        p.vulkan_input_plan_counts[1] += distinct.len() as u64;
                    }
                    p.triangles += triangles.len() as u64;
                    p.phases[7] += processing.elapsed();
                    p.phases[0] += phase.elapsed();
                    return Ok(());
                }
                result => {
                    p.vulkan_vertex_counts[3] += 1;
                    if let Err(error) = result {
                        if p.vulkan_vertex_error.is_none() {
                            p.vulkan_vertex_error = Some(error);
                        }
                    }
                }
            }
        }
    }
    let (mut vertices, raw): (Vec<Vertex>, Vec<[Vec4; 16]>) = if geometry {
        let shaded = process_vertices(p, mem, &layout, &distinct, |out| (map_outputs(p, p.vs.output_mask, &out), out))?;
        (slots.iter().map(|&k| shaded[k].0).collect(), slots.iter().map(|&k| shaded[k].1).collect())
    } else {
        let shaded = process_vertices(p, mem, &layout, &distinct, |out| map_outputs(p, p.vs.output_mask, &out))?;
        (slots.iter().map(|&k| shaded[k]).collect(), Vec::new())
    };
    p.phases[7] += processing.elapsed();
    p.phases[0] += phase.elapsed();
    let dim = reg(p, 0x11E);
    let target = Target {
        color: (reg(p, 0x11D) & 0x0FFF_FFFF) << 3,
        depth: (reg(p, 0x11C) & 0x0FFF_FFFF) << 3,
        width: dim & 0x7FF,
        height: ((dim >> 12) & 0x3FF) + 1,
        color_format: color_buffer_format((reg(p, 0x117) >> 16) & 7)?,
        depth_format: reg(p, 0x116) & 3,
    };
    // USUM_GPU_TRACE=n: describe the first n draws on stderr
    let trace = std::env::var("USUM_GPU_TRACE").ok().and_then(|v| v.parse::<u64>().ok()).is_some_and(|n| p.draws <= n);
    if trace {
        eprintln!(
            "draw {} elements={elements} count={count} mode={} cull={} viewport={}x{} at {:#x} target {:#x} {}x{} fmt {} depth {:#x} fmt {} outmask {:#x} outmap total {} {:08x?}",
            p.draws,
            (reg(p, 0x25E) >> 8) & 3,
            reg(p, 0x40) & 3,
            shader::f24(reg(p, 0x41)),
            shader::f24(reg(p, 0x43)),
            reg(p, 0x68),
            target.color,
            target.width,
            target.height,
            target.color_format,
            target.depth,
            target.depth_format,
            p.vs.output_mask,
            reg(p, 0x4F),
            &p.regs[0x50..0x57]
        );
        if let Ok(inputs) = layout.as_ref().map_err(Clone::clone).and_then(|l| load_vertex(p, l, mem, first)) {
            eprintln!("  inputs {:?}", &inputs[..6]);
            shader::TRACE_ONCE.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = shader::run(&p.vs, &inputs);
        }
        eprintln!(
            "  attrib loc {:#x} fmt {:#x} {:#x} loader0 {:#x} {:#x} {:#x} perm {:?} entry {:#x}",
            reg(p, 0x200),
            reg(p, 0x201),
            reg(p, 0x202),
            reg(p, 0x203),
            reg(p, 0x204),
            reg(p, 0x205),
            &p.vs.permutation[..4],
            p.vs.entry
        );
        let e = p.vs.entry as usize;
        eprintln!("  code {:08x?}", &p.vs.code[e..e + 12]);
        let set: Vec<String> =
            p.vs.floats
                .iter()
                .enumerate()
                .filter(|(_, f)| f.iter().any(|&x| x != 0.0))
                .take(10)
                .map(|(i, f)| format!("c{i}={f:?}"))
                .collect();
        eprintln!("  uniforms {}", set.join(" "));
        for v in vertices.iter().take(4) {
            eprintln!("  pos {:?} color {:?} tex0 {:?}", v.position, v.color, v.texcoord[0]);
        }
    }
    for v in &vertices {
        if v.position[3] > 0.0 {
            p.vertices_positive_w += 1;
        } else {
            p.vertices_other_w += 1;
        }
    }
    if geometry {
        vertices = geometry_stage(p, &raw)?;
    }
    // mode 3 without a geometry shader, with GEOSTAGE_CONFIG bit 8
    // ("drawing triangle elements"): the triangle list the SDK uses for
    // indexed triangles (3dbrew "GPU/Programming Guide", Drawing
    // elements; R-033)
    let mode = match (geometry, (reg(p, 0x25E) >> 8) & 3) {
        (true, _) => 0,
        (false, 3) if reg(p, 0x229) >> 8 & 1 == 1 => 0,
        (false, m) => m,
    };
    let triangles: Vec<[usize; 3]> = match mode {
        0 => (0..vertices.len() / 3).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect(),
        1 => (0..vertices.len().saturating_sub(2))
            .map(|t| if t % 2 == 0 { [t, t + 1, t + 2] } else { [t + 1, t, t + 2] })
            .collect(),
        2 => (1..vertices.len().saturating_sub(1)).map(|t| [0, t, t + 1]).collect(),
        _ => {
            return Err(format!(
                "geometry primitives without the geometry shader: GEOSTAGE_CONFIG {:#x}, CONFIG2 {:#x}, GSH_MISC0 {:#x}, PRIMITIVE_CONFIG {:#x}, GSH input {:#x}, VSH_COM_MODE {:#x}, count {count}, elements {elements}",
                reg(p, 0x229),
                reg(p, 0x253),
                reg(p, 0x252),
                reg(p, 0x25E),
                reg(p, 0x289),
                reg(p, 0x244)
            ))
        }
    };
    #[cfg(feature = "vulkan-compare")]
    let vulkan_target = super::vulkan_compare::CaptureTarget {
        color_address: target.color,
        depth_address: target.depth,
        width: target.width,
        height: target.height,
        color_format: target.color_format,
        depth_format: target.depth_format,
    };
    #[cfg(feature = "vulkan-compare")]
    let vulkan_capture = super::vulkan_compare::capture_if_enabled(p, mem, &vertices, &triangles, vulkan_target)?;
    #[cfg(feature = "vulkan-compare")]
    if p.vulkan_skip_software {
        p.triangles += triangles.len() as u64;
        return Ok(());
    }
    let planes: [fn(&Vertex) -> f32; 7] = [
        |v| v.position[3] - 1e-6,
        |v| v.position[3] - v.position[0],
        |v| v.position[3] + v.position[0],
        |v| v.position[3] - v.position[1],
        |v| v.position[3] + v.position[1],
        |v| -v.position[2],
        |v| v.position[3] + v.position[2],
    ];
    let mut prepared = Vec::new();
    for [a, b, c] in triangles {
        let corners = [&vertices[a], &vertices[b], &vertices[c]];
        p.triangles += 1;
        // what clipping would return without running it: the triangle
        // itself when every corner is inside every plane, nothing when all
        // three are outside one plane
        if planes.iter().all(|plane| corners.iter().all(|v| plane(v) >= 0.0)) {
            prepared.extend(prepare(p, corners));
            continue;
        }
        if planes.iter().any(|plane| corners.iter().all(|v| plane(v) < 0.0)) {
            continue;
        }
        let mut poly = vec![vertices[a], vertices[b], vertices[c]];
        for plane in planes {
            if poly.len() < 3 {
                break;
            }
            poly = clip(poly, plane);
        }
        for k in 1..poly.len().saturating_sub(1) {
            prepared.extend(prepare(p, [&poly[0], &poly[k], &poly[k + 1]]));
        }
    }
    let raster_started = std::time::Instant::now();
    let result = rasterize(p, mem, &target, &prepared);
    p.raster_time += raster_started.elapsed();
    #[cfg(feature = "vulkan-compare")]
    if result.is_ok() {
        super::vulkan_compare::complete_capture(p, mem, vulkan_capture, vulkan_target)?;
    }
    result
}

#[cfg(test)]
mod tests {
    #[test]
    fn programmable_depth_bias_and_w_buffer_are_applied_before_quantization() {
        assert_eq!(super::map_depth(0.5, 0.5, -1.0, -0.125, false), 0.375);
        assert_eq!(super::map_depth(0.5, 0.5, -1.0, -0.125, true), 0.75);
        assert_eq!(super::map_depth(0.25, 1.0, -2.0, 0.125, false), 0.625);
        assert_eq!(super::map_depth(0.5, 1.0, -1.0, -1.0, false), 0.0);
        assert_eq!(super::map_depth(0.5, 0.1, -1.0, 0.0, true), 1.0);
        let bias = super::shader::f24(0xb14000);
        let unshifted = (super::map_depth(0.5, 1.0, -1.0, 0.0, false) * 16777215.0) as u32;
        let shifted = (super::map_depth(0.5, 1.0, -1.0, bias, false) * 16777215.0) as u32;
        assert!(shifted < unshifted, "the observed negative decal offset must win a strict less test");
    }

    use super::*;

    #[cfg(feature = "vulkan-compare")]
    #[test]
    fn planned_hardware_inputs_match_captures_for_small_serial_and_parallel_batches() {
        let run = |planned, cached, count| {
            let mut p = Pica::default();
            p.enable_vulkan_acceleration();
            p.enable_vulkan_vertex();
            p.enable_vulkan_capture(usize::MAX);
            p.vulkan_skip_software = true;
            p.vulkan_input_plan = planned;
            p.vulkan_input_cache_enabled = cached;
            p.regs[0x200] = 0x20000000 >> 3;
            p.regs[0x201] = 15;
            p.regs[0x205] = 1 << 28 | 16 << 16;
            p.regs[0x228] = count;
            p.regs[0x11D] = 0x20002000 >> 3;
            p.regs[0x11E] = 8 | 7 << 12;
            p.regs[0x41] = 0x410000;
            p.regs[0x43] = 0x410000;
            p.regs[0x4F] = 1;
            p.regs[0x50] = 0x03020100;
            p.vs.output_mask = 1;
            p.vs.write(0x25, 0);
            p.vs.write(0x26, 15 | 0x1B << 5 | 0x1B << 14 | 0x1B << 23);
            p.vs.write(0x1B, 0);
            p.vs.write(0x1C, 0x13 << 26);
            p.vs.write(0x1C, 0x22 << 26);
            let mut bytes = vec![0; 0x4000];
            for index in 0..count as usize {
                let position = [index as f32 / 512.0, -0.5, -0.25, 1.0];
                for (slot, value) in bytes[index * 16..index * 16 + 16].as_chunks_mut::<4>().0.iter_mut().zip(position) {
                    *slot = value.to_le_bytes();
                }
            }
            let mut memory = Memory::new();
            memory.map(p.linear_base, bytes, usum_guest::Perm::RW).unwrap();
            draw_inner(&mut p, &mut memory, false).unwrap();
            draw_inner(&mut p, &mut memory, false).unwrap();
            memory.write_u32(p.linear_base, 0.125f32.to_bits()).unwrap();
            draw_inner(&mut p, &mut memory, false).unwrap();
            let snapshots = p.take_vulkan_snapshots();
            assert_eq!(snapshots.len(), 3);
            assert_eq!(snapshots[0].gpu_vertex.as_ref().unwrap().inputs, snapshots[1].gpu_vertex.as_ref().unwrap().inputs);
            assert_ne!(snapshots[0].gpu_vertex.as_ref().unwrap().inputs, snapshots[2].gpu_vertex.as_ref().unwrap().inputs);
            (snapshots, p.vulkan_input_plan_stats(), p.vulkan_input_cache_stats())
        };
        for count in [3, 33, 257] {
            let reference = run(false, false, count);
            assert_eq!(reference.1, [0, 0]);
            for cached in [false, true] {
                let planned = run(true, cached, count);
                assert_eq!(reference.0, planned.0);
                assert_eq!(planned.1, if count >= 16 { [3, 3 * u64::from(count)] } else { [0, 0] });
                assert_eq!(&planned.2[..2], if cached && count >= 16 { &[1, 2] } else { &[0, 0] });
            }
        }
    }

    #[cfg(feature = "vulkan-compare")]
    #[test]
    fn hardware_vertices_capture_immutable_inputs_and_fall_back_per_program() {
        let mut p = Pica::default();
        p.enable_vulkan_acceleration();
        p.enable_vulkan_vertex();
        p.enable_vulkan_capture(usize::MAX);
        p.vulkan_skip_software = true;
        p.regs[0x200] = 0x20000000 >> 3;
        p.regs[0x201] = 15;
        p.regs[0x205] = 1 << 28 | 16 << 16;
        p.regs[0x228] = 3;
        p.regs[0x11D] = 0x20001000 >> 3;
        p.regs[0x11E] = 8 | 7 << 12;
        p.regs[0x41] = 0x410000;
        p.regs[0x43] = 0x410000;
        p.regs[0x4F] = 1;
        p.regs[0x50] = 0x03020100;
        p.vs.output_mask = 1;
        p.vs.write(0x25, 0);
        p.vs.write(0x26, 15 | 0x1B << 5 | 0x1B << 14 | 0x1B << 23);
        p.vs.write(0x1B, 0);
        p.vs.write(0x1C, 0x13 << 26);
        p.vs.write(0x1C, 0x22 << 26);
        let positions = [[-0.5f32, -0.5, -0.25, 1.0], [0.5, -0.5, -0.25, 1.0], [0.0, 0.5, -0.25, 1.0]];
        let mut bytes = vec![0; 8192];
        for (output, value) in bytes[..48].as_chunks_mut::<4>().0.iter_mut().zip(positions.iter().flatten()) {
            *output = value.to_le_bytes();
        }
        let mut memory = Memory::new();
        memory.map(0x30000000, bytes, usum_guest::Perm::RW).unwrap();
        draw_inner(&mut p, &mut memory, false).unwrap();
        let captures = p.take_vulkan_snapshots();
        assert_eq!(captures.len(), 1);
        let gpu = captures[0].gpu_vertex.as_ref().unwrap();
        assert_eq!(gpu.indices, [0, 1, 2]);
        assert_eq!(gpu.inputs[1][0], positions[1]);
        assert!(captures[0].vertices.is_empty());
        p.vs.floats[0] = [7.0; 4];
        p.regs[0x50] = 0;
        assert_eq!(gpu.uniforms[0], 0.0f32.to_bits());
        assert_eq!(gpu.uniforms[392], 0x03020100);
        p.vs.code[0] = 0x2D << 26; // backward uniform jump, not taken with bool 0 false
        p.regs[0x50] = 0x03020100;
        draw_inner(&mut p, &mut memory, false).unwrap();
        let fallback = p.take_vulkan_snapshots();
        assert!(fallback[0].gpu_vertex.is_none());
        assert_eq!(fallback[0].vertices.len(), 3);
        assert_eq!(p.vulkan_vertex_counts, [1, 3, 0, 1]);
        assert!(p.vulkan_vertex_error.as_ref().unwrap().contains("backward"));
    }

    #[test]
    fn fused_vertices_preserve_register_bits_order_and_first_memory_error() {
        let mut pica = Pica::default();
        pica.vs.write(0x25, 0);
        pica.vs.write(0x26, 0xF | 0x1B << 5 | 0x1B << 14 | 0x1B << 23);
        pica.vs.write(0x1B, 0);
        pica.vs.write(0x1C, 0x13 << 26); // mov o0, v0
        pica.vs.write(0x1C, 0x22 << 26); // end
        let layout =
            Ok(Layout { total: 1, loaders: vec![Loader { start: 0x1000, stride: 16, span: 16, fields: vec![(0, 3, 4, 0)] }] });
        let data = (0..128)
            .flat_map(|i| {
                [i as f32, -(i as f32), if i % 7 == 0 { f32::NAN } else { i as f32 * 0.5 }, -0.0]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
            })
            .collect();
        let mut memory = Memory::new();
        memory.map(0x1000, data, usum_guest::Perm::R).unwrap();
        for count in [1, 15, 16, 65, 127] {
            let indices: Vec<u32> = (0..count).map(|i| i * 7 % 128).collect();
            let inputs: Vec<_> =
                indices.iter().map(|&index| load_vertex(&pica, layout.as_ref().unwrap(), &memory, index).unwrap()).collect();
            let expected: Vec<_> = inputs.iter().map(|input| shader::run(&pica.vs, input).unwrap()).collect();
            let actual = process_vertices(&pica, &memory, &layout, &indices, |out| out).unwrap();
            let bits = |values: Vec<[Vec4; 16]>| values.into_iter().map(|v| v.map(|r| r.map(f32::to_bits))).collect::<Vec<_>>();
            assert_eq!(bits(actual), bits(expected));
        }
        let mut indices = vec![0; 32];
        indices[4] = 65_535;
        indices[12] = 65_534;
        let expected = load_vertex(&pica, layout.as_ref().unwrap(), &memory, indices[4]).unwrap_err();
        assert_eq!(process_vertices(&pica, &memory, &layout, &indices, |out| out).unwrap_err(), expected);
    }

    /// A small deterministic generator (xorshift), so the test needs no
    /// dependency and repeats exactly.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 32) as u32
        }
        fn pick(&mut self, from: &[u32]) -> u32 {
            from[self.next() as usize % from.len()]
        }
        fn unit(&mut self) -> f32 {
            (self.next() % 1001) as f32 / 1000.0
        }
    }

    #[test]
    fn triangles_sharing_an_edge_cover_each_pixel_once() {
        // a square split along its diagonal, in both windings, and at a
        // sub-pixel offset that puts pixel centres exactly on the diagonal
        for (offset, flip) in [(0.0, false), (0.5, false), (0.25, true), (0.5, true)] {
            let (a, b, c, d) = (
                [2.0 + offset, 2.0 + offset],
                [18.0 + offset, 2.0 + offset],
                [18.0 + offset, 18.0 + offset],
                [2.0 + offset, 18.0 + offset],
            );
            let tris = if flip { [[a, c, b], [a, d, c]] } else { [[a, b, c], [a, c, d]] };
            let mut hits = vec![0u32; 32 * 32];
            for t in tris {
                let (edges, _) = coverage(t).unwrap();
                for y in 0..32 {
                    for x in 0..32 {
                        if edges.iter().all(|e| e.covers(e.at(x, y))) {
                            hits[(y * 32 + x) as usize] += 1;
                        }
                    }
                }
            }
            assert!(hits.iter().all(|&h| h <= 1), "offset {offset}: a pixel drawn twice");
            let inside = (0..32).flat_map(|y| (0..32).map(move |x| (x, y))).filter(|&(x, y)| {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                cx > a[0] && cx < c[0] && cy > a[1] && cy < c[1]
            });
            for (x, y) in inside {
                assert_eq!(hits[(y * 32 + x) as usize], 1, "offset {offset}: ({x}, {y}) inside the square missed");
            }
        }
    }

    #[test]
    fn the_span_combiner_matches_the_per_fragment_one() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let sources = [0, 1, 2, 3, 4, 5, 6, 13, 14, 15];
        let rgb_ops = [0, 1, 2, 3, 4, 5, 8, 9, 12, 13];
        let modes = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        for case in 0..400 {
            let stages: [Stage; 6] = std::array::from_fn(|_| {
                let src = |r: &mut Rng| r.pick(&sources);
                let sources_reg = (0..3).map(|k| src(&mut rng) << (4 * k)).sum::<u32>()
                    | (0..3).map(|k| src(&mut rng) << (16 + 4 * k)).sum::<u32>();
                let operands_reg = (0..3).map(|k| rng.pick(&rgb_ops) << (4 * k)).sum::<u32>()
                    | (0..3).map(|k| (rng.next() & 7) << (12 + 4 * k)).sum::<u32>();
                let combiner_reg = rng.pick(&modes) | rng.pick(&modes) << 16;
                let scale_reg = (rng.next() & 3) | (rng.next() & 3) << 16;
                Stage::new(sources_reg, operands_reg, combiner_reg, rng.next(), scale_reg)
            });
            let pass: [bool; 6] = std::array::from_fn(|i| i > 0 && rng.next().is_multiple_of(4));
            let plan = TevPlan {
                pass,
                uses: [true; 3],
                lit: true,
                stages,
                buffer: [rng.unit(), rng.unit(), rng.unit(), rng.unit()],
                update: rng.next(),
                blend: (0, [0.0; 4]),
            };
            let n = 1 + (rng.next() % 40) as usize;
            let mut span = Span::default();
            let mut inputs = Vec::new();
            let (mut lit0, mut lit1) = (Vec::new(), Vec::new());
            for _ in 0..n {
                let l0 = [rng.unit(), rng.unit(), rng.unit(), rng.unit()];
                let l1 = [rng.unit(), rng.unit(), rng.unit(), rng.unit()];
                span.lit[0].push(l0);
                span.lit[1].push(l1);
                lit0.push(l0);
                lit1.push(l1);
                let color = [rng.unit(), rng.unit(), rng.unit(), rng.unit()];
                let tex: [Vec4; 3] = std::array::from_fn(|_| [rng.unit(), rng.unit(), rng.unit(), rng.unit()]);
                span.color.push(color);
                for (sampled, t) in span.sampled.iter_mut().zip(tex) {
                    sampled.push(t);
                }
                inputs.push((color, tex));
            }
            combine_span(&plan, &mut span, n);
            for (f, (color, tex)) in inputs.iter().enumerate() {
                let want = combine(&plan, *color, tex, [lit0[f], lit1[f]]);
                assert_eq!(span.previous[f].map(f32::to_bits), want.map(f32::to_bits), "case {case}, fragment {f}");
            }
        }
    }
}
