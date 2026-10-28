//! Fragment lighting: the PICA200 computes a primary (diffuse) and a
//! secondary (specular) color per fragment from up to eight lights, the
//! interpolated surface quaternion and view vector, and lookup tables.
//! The texture combiners read them as sources 1 and 2.
//!
//! Registers and their bit fields: 3dbrew "GPU/Internal Registers"
//! (GPUREG_LIGHT*_*, GPUREG_LIGHTING_*). 3dbrew does not give the
//! equations (it points to outside material); they, the table indexing
//! and which tables each light environment configuration allows follow
//! Citra's software renderer, read as documentation (R-051). Not
//! emulated: shadows, bump and tangent maps (the normal is always the
//! quaternion's), the CP input (reads 0).

use super::shader::Vec4;
use super::Pica;

fn reg(p: &Pica, r: u16) -> u32 {
    p.regs[usize::from(r)]
}

/// float1.5.10 to f32.
fn f16(bits: u32) -> f32 {
    let (sign, exp, mantissa) = ((bits >> 15) & 1, (bits >> 10) & 0x1F, bits & 0x3FF);
    let magnitude = match exp {
        0 => mantissa as f32 / 1024.0 * 2f32.powi(-14),
        0x1F => f32::INFINITY,
        e => (1.0 + mantissa as f32 / 1024.0) * 2f32.powi(e as i32 - 15),
    };
    if sign == 1 {
        -magnitude
    } else {
        magnitude
    }
}

/// float1.7.12 (bias 63) to f32.
fn f20(bits: u32) -> f32 {
    let (sign, exp, mantissa) = ((bits >> 19) & 1, (bits >> 12) & 0x7F, bits & 0xFFF);
    let magnitude = if exp == 0 { 0.0 } else { (1.0 + mantissa as f32 / 4096.0) * 2f32.powi(exp as i32 - 63) };
    if sign == 1 {
        -magnitude
    } else {
        magnitude
    }
}

/// fixed1.1.11 (13 bits, two's complement) as Citra scales it (/2047).
fn fixed13(bits: u32) -> f32 {
    (((bits & 0x1FFF) << 19) as i32 >> 19) as f32 / 2047.0
}

/// An unsigned 8-bit color register (B bits 0-7, G 10-17, R 20-27).
fn color(bits: u32) -> [f32; 3] {
    [((bits >> 20) & 0xFF) as f32 / 255.0, ((bits >> 10) & 0xFF) as f32 / 255.0, (bits & 0xFF) as f32 / 255.0]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len == 0.0 {
        v
    } else {
        v.map(|c| c / len)
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// One light's registers, decoded.
#[derive(Debug, Clone, Copy)]
struct Light {
    id: usize,
    specular0: [f32; 3],
    specular1: [f32; 3],
    diffuse: [f32; 3],
    ambient: [f32; 3],
    position: [f32; 3],
    spot_dir: [f32; 3],
    directional: bool,
    two_sided: bool,
    geometric: [bool; 2],
    attenuation_bias: f32,
    attenuation_scale: f32,
}

/// Lookup tables by number (3dbrew GPUREG_LIGHTING_LUT_INDEX).
const D0: usize = 0;
const D1: usize = 1;
const FR: usize = 3;
const RB: usize = 4;
const RG: usize = 5;
const RR: usize = 6;
const SP: usize = 8;
const DA: usize = 16;

/// A draw's lighting state, decoded once per draw.
#[derive(Debug, Clone)]
pub struct Lighting {
    lights: Vec<Light>,
    ambient: [f32; 3],
    /// GPUREG_LIGHTING_CONFIG0 and CONFIG1.
    config0: u32,
    config1: u32,
    abs: u32,
    select: u32,
    scale: u32,
    luts: std::sync::Arc<Vec<[u32; 256]>>,
}

/// One lookup table input: the table, where in the select/abs/scale
/// registers its fields are, and the CONFIG1 bit that disables it.
#[derive(Clone, Copy)]
struct Sampler {
    shift: u32,
    disable_bit: u32,
}

const SAMPLE_D0: Sampler = Sampler { shift: 0, disable_bit: 16 };
const SAMPLE_D1: Sampler = Sampler { shift: 4, disable_bit: 17 };
const SAMPLE_SP: Sampler = Sampler { shift: 8, disable_bit: 0 };
const SAMPLE_FR: Sampler = Sampler { shift: 12, disable_bit: 19 };
const SAMPLE_RB: Sampler = Sampler { shift: 16, disable_bit: 20 };
const SAMPLE_RG: Sampler = Sampler { shift: 20, disable_bit: 21 };
const SAMPLE_RR: Sampler = Sampler { shift: 24, disable_bit: 22 };

impl Lighting {
    /// The draw's lighting, if GPUREG_LIGHTING_ENABLE0 has it on.
    pub fn new(p: &Pica) -> Option<Lighting> {
        if reg(p, 0x8F) & 1 == 0 {
            return None;
        }
        let count = (reg(p, 0x1C2) & 7) as usize + 1;
        let permutation = reg(p, 0x1D9);
        let lights = (0..count)
            .map(|k| {
                let id = ((permutation >> (4 * k)) & 7) as usize;
                let base = 0x140 + 0x10 * id as u16;
                let xy = reg(p, base + 4);
                let spot_xy = reg(p, base + 6);
                let config = reg(p, base + 9);
                Light {
                    id,
                    specular0: color(reg(p, base)),
                    specular1: color(reg(p, base + 1)),
                    diffuse: color(reg(p, base + 2)),
                    ambient: color(reg(p, base + 3)),
                    position: [f16(xy & 0xFFFF), f16(xy >> 16), f16(reg(p, base + 5) & 0xFFFF)],
                    spot_dir: [fixed13(spot_xy), fixed13(spot_xy >> 16), fixed13(reg(p, base + 7))],
                    directional: config & 1 != 0,
                    two_sided: config & 2 != 0,
                    geometric: [config & 4 != 0, config & 8 != 0],
                    attenuation_bias: f20(reg(p, base + 0xA) & 0xF_FFFF),
                    attenuation_scale: f20(reg(p, base + 0xB) & 0xF_FFFF),
                }
            })
            .collect();
        Some(Lighting {
            lights,
            ambient: color(reg(p, 0x1C0)),
            config0: reg(p, 0x1C3),
            config1: reg(p, 0x1C4),
            abs: reg(p, 0x1D0),
            select: reg(p, 0x1D1),
            scale: reg(p, 0x1D2),
            luts: p.light_luts.clone(),
        })
    }

    /// The light environment configuration (CONFIG0 bits 4-7).
    fn environment(&self) -> u32 {
        (self.config0 >> 4) & 0xF
    }

    /// Whether the configuration provides a table (Citra's table;
    /// configuration "7" is the value 8).
    fn supported(&self, table: usize) -> bool {
        let c = self.environment();
        match table {
            D0 => c != 1,
            D1 => !matches!(c, 0 | 1 | 5),
            FR => !matches!(c, 0 | 2 | 4),
            RR => c != 3,
            RG | RB => matches!(c, 4 | 5 | 8),
            SP => !matches!(c, 2 | 3),
            _ => true,
        }
    }

    /// A table's value at `index` plus `delta` toward the next entry:
    /// entries are 12-bit values (/4095) and signed 12-bit differences
    /// (/2047).
    fn entry(&self, table: usize, index: usize, delta: f32) -> f32 {
        let raw = self.luts.get(table).map_or(0, |t| t[index & 0xFF]);
        let value = (raw & 0xFFF) as f32 / 4095.0;
        let diff = ((((raw >> 12) & 0xFFF) << 20) as i32 >> 20) as f32 / 2047.0;
        value + diff * delta
    }

    /// A sampler's table at `input`, scaled. With abs (its ABS bit clear)
    /// the input 0..1 spans the table; without, -1..1 maps to the signed
    /// index (two's complement).
    fn lookup(&self, table: usize, sampler: Sampler, input: f32) -> f32 {
        let abs = (self.abs >> (sampler.shift + 1)) & 1 == 0;
        let (index, delta) = if abs {
            let v = input.abs();
            let index = (v * 256.0).clamp(0.0, 255.0) as usize;
            (index, v * 256.0 - index as f32)
        } else {
            let signed = (input * 128.0).floor().clamp(-128.0, 127.0) as i32;
            (signed as u8 as usize, input * 128.0 - signed as f32)
        };
        let scale = match (self.scale >> sampler.shift) & 7 {
            0 => 1.0,
            1 => 2.0,
            2 => 4.0,
            3 => 8.0,
            6 => 0.25,
            7 => 0.5,
            _ => 1.0,
        };
        scale * self.entry(table, index, delta)
    }

    fn enabled(&self, table: usize, sampler: Sampler) -> bool {
        self.config1 >> sampler.disable_bit & 1 == 0 && self.supported(table)
    }

    /// The primary and secondary colors for a fragment with this
    /// (interpolated) quaternion and view vector.
    pub fn shade(&self, quaternion: Vec4, view: [f32; 3]) -> (Vec4, Vec4) {
        // the surface normal (0, 0, 1) rotated by the normalized quaternion
        let len = quaternion.iter().map(|c| c * c).sum::<f32>().sqrt();
        let q = if len == 0.0 { [0.0, 0.0, 0.0, 1.0] } else { quaternion.map(|c| c / len) };
        let (qv, qw) = ([q[0], q[1], q[2]], q[3]);
        let up = [0.0, 0.0, 1.0];
        let inner = add(cross(qv, up), up.map(|c| c * qw));
        let normal = add(up, cross(qv, inner).map(|c| 2.0 * c));
        let mut diffuse = [0f32; 3];
        let mut specular = [0f32; 3];
        let (mut diffuse_alpha, mut specular_alpha) = (1.0, 1.0);
        for light in &self.lights {
            let to_light = if light.directional { light.position } else { add(light.position, view) };
            let l = normalize(to_light);
            let half = add(normalize(view), l);
            let input = |sampler: Sampler| -> f32 {
                match (self.select >> sampler.shift) & 7 {
                    0 => dot(normal, normalize(half)),
                    1 => dot(normalize(view), normalize(half)),
                    2 => dot(normal, normalize(view)),
                    3 => dot(l, normal),
                    4 => dot(l, light.spot_dir),
                    _ => 0.0, // CP needs a tangent (not emulated)
                }
            };
            // distance and spotlight attenuation, per light
            let distance = if self.config1 >> (24 + light.id) & 1 == 0 {
                let d = dot(to_light, to_light).sqrt();
                let x = (light.attenuation_scale * d + light.attenuation_bias).clamp(0.0, 1.0);
                let index = (x * 256.0).clamp(0.0, 255.0) as usize;
                self.entry(DA + light.id, index, x * 256.0 - index as f32)
            } else {
                1.0
            };
            let spot = if self.config1 >> (8 + light.id) & 1 == 0 && self.supported(SP) {
                self.lookup(SP + light.id, SAMPLE_SP, input(SAMPLE_SP))
            } else {
                1.0
            };
            let raw_dot = dot(l, normal);
            let n_dot_l = if light.two_sided { raw_dot.abs() } else { raw_dot.max(0.0) };
            let clamp = if self.config0 >> 27 & 1 == 1 && raw_dot <= 0.0 { 0.0 } else { 1.0 };
            let geometric = if light.geometric[0] || light.geometric[1] {
                let h = dot(half, half);
                if h == 0.0 {
                    0.0
                } else {
                    (n_dot_l / h).min(1.0)
                }
            } else {
                1.0
            };
            let d0 = if self.enabled(D0, SAMPLE_D0) { self.lookup(D0, SAMPLE_D0, input(SAMPLE_D0)) } else { 1.0 };
            let d1 = if self.enabled(D1, SAMPLE_D1) { self.lookup(D1, SAMPLE_D1, input(SAMPLE_D1)) } else { 1.0 };
            let red = if self.enabled(RR, SAMPLE_RR) { self.lookup(RR, SAMPLE_RR, input(SAMPLE_RR)) } else { 1.0 };
            let green = if self.enabled(RG, SAMPLE_RG) { self.lookup(RG, SAMPLE_RG, input(SAMPLE_RG)) } else { red };
            let blue = if self.enabled(RB, SAMPLE_RB) { self.lookup(RB, SAMPLE_RB, input(SAMPLE_RB)) } else { red };
            let g0 = if light.geometric[0] { geometric } else { 1.0 };
            let g1 = if light.geometric[1] { geometric } else { 1.0 };
            let reflection = [red, green, blue];
            let attenuation = distance * spot;
            for c in 0..3 {
                let s0 = d0 * light.specular0[c] * g0;
                let s1 = d1 * reflection[c] * light.specular1[c] * g1;
                diffuse[c] += (light.diffuse[c] * n_dot_l + light.ambient[c]) * attenuation;
                specular[c] += (s0 + s1) * clamp * attenuation;
            }
            // the Fresnel table sets the alphas (the last light's value)
            if self.enabled(FR, SAMPLE_FR) {
                let fresnel = self.lookup(FR, SAMPLE_FR, input(SAMPLE_FR));
                let selector = (self.config0 >> 2) & 3;
                if selector & 1 != 0 {
                    diffuse_alpha = fresnel;
                }
                if selector & 2 != 0 {
                    specular_alpha = fresnel;
                }
            }
        }
        let primary = [0, 1, 2].map(|c| (diffuse[c] + self.ambient[c]).clamp(0.0, 1.0));
        let secondary = specular.map(|c| c.clamp(0.0, 1.0));
        (
            [primary[0], primary[1], primary[2], diffuse_alpha.clamp(0.0, 1.0)],
            [secondary[0], secondary[1], secondary[2], specular_alpha.clamp(0.0, 1.0)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats() {
        assert_eq!(f16(0x3C00), 1.0);
        assert_eq!(f16(0xC000), -2.0);
        assert_eq!(f20(63 << 12), 1.0);
        assert_eq!(fixed13(0x7FF), 1.0);
        assert_eq!(fixed13(0x1FFF), -1.0 / 2047.0);
        assert_eq!(color(255 << 20 | 128 << 10), [1.0, 128.0 / 255.0, 0.0]);
    }

    #[test]
    fn a_light_in_front_lights_a_surface_facing_it() {
        // one directional light along +z, white diffuse, no tables in use
        // (all disabled), global ambient 0
        let light = Light {
            id: 0,
            specular0: [0.0; 3],
            specular1: [0.0; 3],
            diffuse: [1.0; 3],
            ambient: [0.0; 3],
            position: [0.0, 0.0, 1.0],
            spot_dir: [0.0; 3],
            directional: true,
            two_sided: false,
            geometric: [false; 2],
            attenuation_bias: 0.0,
            attenuation_scale: 0.0,
        };
        let lighting = Lighting {
            lights: vec![light],
            ambient: [0.0; 3],
            config0: 0,
            config1: 0xFFFF_FFFF,
            abs: 0,
            select: 0,
            scale: 0,
            luts: vec![[0; 256]; 24].into(),
        };
        // identity quaternion: the normal is +z, facing the light
        let (primary, _) = lighting.shade([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]);
        assert_eq!(primary, [1.0, 1.0, 1.0, 1.0]);
        // rotated 180 degrees about x: the normal faces away
        let (primary, _) = lighting.shade([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(&primary[..3], &[0.0, 0.0, 0.0]);
    }

    #[test]
    fn tables_interpolate_between_entries() {
        let mut luts = vec![[0u32; 256]; 24];
        // entry 64: value 2048 (~0.5), difference +1024 (~0.5) to the next
        luts[D0][64] = 2048 | 1024 << 12;
        let lighting = Lighting {
            lights: Vec::new(),
            ambient: [0.0; 3],
            config0: 0,
            config1: 0,
            abs: 0,
            select: 0,
            scale: 0,
            luts: luts.into(),
        };
        // abs input 0.25 + half a step: index 64, delta 0.5
        let v = lighting.lookup(D0, SAMPLE_D0, 0.25 + 0.5 / 256.0);
        assert!((v - (2048.0 / 4095.0 + 0.5 * 1024.0 / 2047.0)).abs() < 1e-6, "{v}");
    }

    #[test]
    fn draw_lighting_shares_luts_until_a_register_write() {
        let mut pica = Pica::default();
        pica.regs[0x8F] = 1;
        let lighting = Lighting::new(&pica).unwrap();
        assert!(std::sync::Arc::ptr_eq(&lighting.luts, &pica.light_luts));

        let previous = std::sync::Arc::clone(&pica.light_luts);
        std::sync::Arc::make_mut(&mut pica.light_luts)[0][0] = 1;
        assert!(!std::sync::Arc::ptr_eq(&previous, &pica.light_luts));
        assert_eq!(previous[0][0], 0);
        assert_eq!(pica.light_luts[0][0], 1);
    }
}
