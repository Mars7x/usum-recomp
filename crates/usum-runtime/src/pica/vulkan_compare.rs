//! Immutable handoff from the software PICA vertex/geometry stages to the
//! opt-in Vulkan comparison renderer. Building this module cannot change the
//! software framebuffer; execution and replacement remain separate steps.

use std::sync::Arc;
use usum_guest::Memory;
use usum_render::{
    FragmentInputs, GpuLighting, LightingState, PicaDrawState, RasterBatch, RasterVertex, Renderer, SampledTexture, SamplerState,
};

use super::draw::Vertex;
use super::{bytes_per_pixel, decode_pixel, encode_pixel, tiled_offset, Pica};

#[derive(Clone, Debug, PartialEq)]
pub struct TargetSnapshot {
    pub width: u32,
    pub height: u32,
    pub color: Vec<u8>,
    pub depth: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextureSnapshot {
    pub address: u32,
    pub source_size: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<[u8]>,
    pub sampler: SamplerState,
    pub border: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureTarget {
    pub color_address: u32,
    pub depth_address: u32,
    pub width: u32,
    pub height: u32,
    pub color_format: u32,
    pub depth_format: u32,
}

#[derive(Clone, Debug)]
pub(super) struct LiveTarget {
    info: CaptureTarget,
    gpu: Arc<usum_render::ColorTarget>,
}

/// One triangle-list draw after PICA vertex and optional geometry shading.
/// Raw register and LUT state is decoded and owned, so subsequent command-list
/// writes cannot change a queued comparison draw.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawSnapshot {
    pub draw_number: u64,
    pub target_info: CaptureTarget,
    pub state: PicaDrawState,
    pub vertices: Vec<RasterVertex>,
    pub target: Arc<TargetSnapshot>,
    pub textures: [Option<TextureSnapshot>; 3],
    pub software_result: Option<TargetSnapshot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub pixels: usize,
    pub different_pixels: usize,
    pub pixels_over_one: usize,
    pub maximum_channel_delta: u8,
    pub maximum_at: [u32; 2],
    pub actual_at_maximum: [u8; 4],
    pub expected_at_maximum: [u8; 4],
    pub different_depth_values: usize,
    pub maximum_depth_delta: u32,
    pub maximum_depth_at: [u32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FeedbackStats {
    pub reads: usize,
    pub compatible_color: usize,
    pub other_color: usize,
    pub depth: usize,
    pub partial: usize,
    pub depth_pairs: [usize; 56],
}

impl Default for FeedbackStats {
    fn default() -> Self {
        Self { reads: 0, compatible_color: 0, other_color: 0, depth: 0, partial: 0, depth_pairs: [0; 56] }
    }
}

#[derive(Clone, Debug)]
struct LiveTexture {
    width: u32,
    height: u32,
    pixels: Arc<[u8]>,
    sampler: SamplerState,
    border: [u8; 4],
    gpu: Arc<SampledTexture>,
}

struct LiveFeedbackTexture {
    address: u32,
    sampler: SamplerState,
    border: [u8; 4],
    gpu: Arc<SampledTexture>,
}

#[derive(Clone, Debug)]
struct LiveLighting {
    state: LightingState,
    gpu: Arc<GpuLighting>,
}

fn same_lighting_state(a: &LightingState, b: &LightingState) -> bool {
    a.lights == b.lights
        && a.ambient == b.ambient
        && a.config0 == b.config0
        && a.config1 == b.config1
        && a.abs == b.abs
        && a.select == b.select
        && a.scale == b.scale
        && (Arc::ptr_eq(&a.luts, &b.luts) || a.luts == b.luts)
}

#[derive(Clone, Debug, Default)]
pub(super) struct LiveResources {
    textures: Vec<LiveTexture>,
    lighting: Vec<LiveLighting>,
    texture_uses: u64,
    texture_uploads: u64,
    lighting_uses: u64,
    lighting_uploads: u64,
}

impl LiveResources {
    pub(super) fn clear(&mut self) {
        self.textures.clear();
        self.lighting.clear();
        self.texture_uses = 0;
        self.texture_uploads = 0;
        self.lighting_uses = 0;
        self.lighting_uploads = 0;
    }

    pub(super) fn stats(&self) -> (u64, u64, u64, u64) {
        (self.texture_uses, self.texture_uploads, self.lighting_uses, self.lighting_uploads)
    }

    fn begin_draw(&mut self) {
        if self.textures.len() > 512 {
            self.textures.clear();
        }
        if self.lighting.len() > 64 {
            self.lighting.clear();
        }
    }

    fn texture(&mut self, renderer: &Renderer, snapshot: &TextureSnapshot) -> Result<usize, String> {
        self.texture_uses += 1;
        if let Some(index) = self.textures.iter().position(|entry| {
            entry.width == snapshot.width
                && entry.height == snapshot.height
                && entry.sampler == snapshot.sampler
                && entry.border == snapshot.border
                && Arc::ptr_eq(&entry.pixels, &snapshot.pixels)
        }) {
            return Ok(index);
        }
        let gpu = renderer.create_rgba8_texture_with_border(
            snapshot.width,
            snapshot.height,
            &snapshot.pixels,
            snapshot.sampler,
            snapshot.border,
        )?;
        self.textures.push(LiveTexture {
            width: snapshot.width,
            height: snapshot.height,
            pixels: Arc::clone(&snapshot.pixels),
            sampler: snapshot.sampler,
            border: snapshot.border,
            gpu: Arc::new(gpu),
        });
        self.texture_uploads += 1;
        Ok(self.textures.len() - 1)
    }

    fn lighting(&mut self, renderer: &Renderer, state: &LightingState) -> Result<usize, String> {
        self.lighting_uses += 1;
        if let Some(index) = self.lighting.iter().position(|entry| same_lighting_state(&entry.state, state)) {
            return Ok(index);
        }
        self.lighting.push(LiveLighting { state: state.clone(), gpu: Arc::new(renderer.create_lighting(state)?) });
        self.lighting_uploads += 1;
        Ok(self.lighting.len() - 1)
    }
}

impl DrawSnapshot {
    pub fn from_shaded_vertices(
        pica: &Pica,
        shaded: &[Vertex],
        triangles: &[[usize; 3]],
        target_info: CaptureTarget,
        target: TargetSnapshot,
        textures: [Option<TextureSnapshot>; 3],
    ) -> Result<Self, String> {
        Self::from_shaded_vertices_shared(pica, shaded, triangles, target_info, Arc::new(target), textures)
    }

    fn from_shaded_vertices_shared(
        pica: &Pica,
        shaded: &[Vertex],
        triangles: &[[usize; 3]],
        target_info: CaptureTarget,
        target: Arc<TargetSnapshot>,
        textures: [Option<TextureSnapshot>; 3],
    ) -> Result<Self, String> {
        let state =
            PicaDrawState::from_pica_registers_shared(&pica.regs, Arc::clone(&pica.light_luts), target.width, target.height)?;
        // TEXUNIT_CONFIG bit 13 routes unit 2 through texture coordinates 1.
        // Resolve it in the immutable snapshot, as the software rasterizer does.
        let unit2_coords = if pica.regs[0x80] & (1 << 13) != 0 { 1 } else { 2 };
        let mut vertices = Vec::with_capacity(triangles.len().checked_mul(3).ok_or("triangle list is too large")?);
        for triangle in triangles {
            for &index in triangle {
                let vertex = shaded.get(index).ok_or_else(|| format!("triangle vertex {index} is out of range"))?;
                vertices.push(RasterVertex {
                    position: vertex.position,
                    color: vertex.color,
                    texcoord0: [vertex.texcoord[0][0], vertex.texcoord[0][1]],
                    texcoord1: [vertex.texcoord[1][0], vertex.texcoord[1][1]],
                    texcoord2: [vertex.texcoord[unit2_coords][0], vertex.texcoord[unit2_coords][1]],
                    quaternion: vertex.quaternion,
                    view: vertex.view,
                });
            }
        }
        if vertices.is_empty() {
            return Err("a Vulkan draw snapshot needs at least one triangle".into());
        }
        Ok(Self { draw_number: pica.draws, target_info, state, vertices, target, textures, software_result: None })
    }

    /// Executes the captured draw in isolation and returns top-to-bottom RGBA8.
    /// The caller may compare this with the software framebuffer; no guest
    /// state is referenced or changed.
    pub fn render(&self, renderer: &Renderer) -> Result<Vec<u8>, String> {
        self.render_target(renderer).map(|(color, _)| color)
    }

    fn render_target(&self, renderer: &Renderer) -> Result<(Vec<u8>, Vec<f32>), String> {
        let target = renderer.create_color_target_from_snapshot(
            self.target.width,
            self.target.height,
            &self.target.color,
            &self.target.depth,
        )?;
        let mut textures: [Option<SampledTexture>; 3] = [None, None, None];
        for (output, snapshot) in textures.iter_mut().zip(&self.textures) {
            if let Some(snapshot) = snapshot {
                *output = Some(renderer.create_rgba8_texture_with_border(
                    snapshot.width,
                    snapshot.height,
                    &snapshot.pixels,
                    snapshot.sampler,
                    snapshot.border,
                )?);
            }
        }
        let lighting = self.state.lighting.as_ref().map(|state| renderer.create_lighting(state)).transpose()?;
        let inputs = FragmentInputs {
            texture0: textures[0].as_ref(),
            texture1: textures[1].as_ref(),
            texture2: textures[2].as_ref(),
            lighting: lighting.as_ref(),
        };
        renderer.draw_vertices_with_inputs(&target, &self.vertices, self.state.viewport, self.state.raster, inputs)?;
        renderer.read_color_and_depth_target(&target)
    }

    fn record_live(
        &self,
        renderer: &Renderer,
        batch: &mut RasterBatch<'_>,
        resources: &mut LiveResources,
        feedback: &[LiveFeedbackTexture],
    ) -> Result<(), String> {
        resources.begin_draw();
        let mut texture_indices = [None; 3];
        let mut feedback_inputs: [Option<&SampledTexture>; 3] = [None, None, None];
        for ((output, direct), snapshot) in texture_indices.iter_mut().zip(&mut feedback_inputs).zip(&self.textures) {
            if let Some(snapshot) = snapshot {
                if let Some(entry) = feedback.iter().find(|entry| {
                    entry.address == snapshot.address && entry.sampler == snapshot.sampler && entry.border == snapshot.border
                }) {
                    *direct = Some(entry.gpu.as_ref());
                } else {
                    *output = Some(resources.texture(renderer, snapshot)?);
                }
            }
        }
        let lighting_index = self.state.lighting.as_ref().map(|state| resources.lighting(renderer, state)).transpose()?;
        batch.draw_with_inputs(
            &self.vertices,
            self.state.viewport,
            self.state.raster,
            FragmentInputs {
                texture0: feedback_inputs[0].or_else(|| texture_indices[0].map(|index| resources.textures[index].gpu.as_ref())),
                texture1: feedback_inputs[1].or_else(|| texture_indices[1].map(|index| resources.textures[index].gpu.as_ref())),
                texture2: feedback_inputs[2].or_else(|| texture_indices[2].map(|index| resources.textures[index].gpu.as_ref())),
                lighting: lighting_index.map(|index| resources.lighting[index].gpu.as_ref()),
            },
        )
    }

    /// Saves private, game-derived evidence for one isolated draw. This is
    /// opt-in so normal play and comparison never write capture payloads.
    pub fn write_diagnostic(&self, renderer: &Renderer, directory: &std::path::Path) -> Result<(), String> {
        let directory = directory.join(format!("draw-{}", self.draw_number));
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let write = |name: &str, bytes: &[u8]| std::fs::write(directory.join(name), bytes).map_err(|error| error.to_string());
        write("state.txt", format!("target: {:#?}\nstate: {:#?}\n", self.target_info, self.state).as_bytes())?;
        let mut vertices = Vec::with_capacity(self.vertices.len() * 84);
        for vertex in &self.vertices {
            for value in vertex
                .position
                .into_iter()
                .chain(vertex.color)
                .chain(vertex.texcoord0)
                .chain(vertex.texcoord1)
                .chain(vertex.texcoord2)
                .chain(vertex.quaternion)
                .chain(vertex.view)
            {
                vertices.extend_from_slice(&value.to_le_bytes());
            }
        }
        write("vertices.f32le", &vertices)?;
        let (actual, depth) = self.render_target(renderer)?;
        let depth_bytes = |values: &[f32]| values.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>();
        write("before.rgba", &self.target.color)?;
        write("before-depth.f32le", &depth_bytes(&self.target.depth))?;
        write("vulkan.rgba", &actual)?;
        write("vulkan-depth.f32le", &depth_bytes(&depth))?;
        if let Some(expected) = &self.software_result {
            write("software.rgba", &expected.color)?;
            write("software-depth.f32le", &depth_bytes(&expected.depth))?;
        }
        for (unit, texture) in self.textures.iter().enumerate() {
            if let Some(texture) = texture {
                write(&format!("texture-{unit}.rgba"), &texture.pixels)?;
                write(
                    &format!("texture-{unit}.txt"),
                    format!(
                        "{}x{} format {} address {:#x} sampler {:?} border {:?}\n",
                        texture.width, texture.height, texture.format, texture.address, texture.sampler, texture.border,
                    )
                    .as_bytes(),
                )?;
            }
        }
        Ok(())
    }

    pub fn compare(&self, renderer: &Renderer) -> Result<Comparison, String> {
        let expected = self.software_result.as_ref().ok_or("draw snapshot has no software result")?;
        let (actual, actual_depth) = self.render_target(renderer)?;
        if actual.len() != expected.color.len() {
            return Err("Vulkan and software framebuffer sizes differ".into());
        }
        let mut different_pixels = 0;
        let mut pixels_over_one = 0;
        let mut maximum_channel_delta = 0;
        let mut maximum_at = [0; 2];
        let mut actual_at_maximum = [0; 4];
        let mut expected_at_maximum = [0; 4];
        for (index, (actual, expected)) in actual.as_chunks::<4>().0.iter().zip(expected.color.as_chunks::<4>().0).enumerate() {
            let delta = actual.iter().zip(expected).map(|(&a, &b)| a.abs_diff(b)).max().unwrap_or(0);
            different_pixels += usize::from(delta != 0);
            pixels_over_one += usize::from(delta > 1);
            if delta > maximum_channel_delta {
                maximum_channel_delta = delta;
                maximum_at = [index as u32 % self.target.width, index as u32 / self.target.width];
                actual_at_maximum = *actual;
                expected_at_maximum = *expected;
            }
        }
        if actual_depth.len() != expected.depth.len() {
            return Err("Vulkan and software depth-buffer sizes differ".into());
        }
        let depth_maximum = if self.target_info.depth_format == 0 { 0xFFFF } else { 0xFF_FFFF };
        let quantize_actual = |value: f32| (value.clamp(0.0, 1.0) * depth_maximum as f32) as u32;
        let quantize_expected = |value: f32| (value.clamp(0.0, 1.0) * depth_maximum as f32).round() as u32;
        let mut different_depth_values = 0;
        let mut maximum_depth_delta = 0;
        let mut maximum_depth_at = [0; 2];
        for (index, (&actual, &expected)) in actual_depth.iter().zip(&expected.depth).enumerate() {
            let delta = quantize_actual(actual).abs_diff(quantize_expected(expected));
            different_depth_values += usize::from(delta != 0);
            if delta > maximum_depth_delta {
                maximum_depth_delta = delta;
                maximum_depth_at = [index as u32 % self.target.width, index as u32 / self.target.width];
            }
        }
        Ok(Comparison {
            pixels: actual.len() / 4,
            different_pixels,
            pixels_over_one,
            maximum_channel_delta,
            maximum_at,
            actual_at_maximum,
            expected_at_maximum,
            different_depth_values,
            maximum_depth_delta,
            maximum_depth_at,
        })
    }
}

fn target_snapshot(pica: &Pica, mem: &Memory, target: CaptureTarget) -> Result<TargetSnapshot, String> {
    let CaptureTarget { color_address, depth_address, width, height, color_format, depth_format } = target;
    let pixels = width.checked_mul(height).ok_or("framebuffer is too large")?;
    let color_bpp = bytes_per_pixel(color_format).ok_or_else(|| format!("unsupported color format {color_format}"))? as u32;
    let color_at = pica.pa_to_va(color_address)?;
    let color_raw = mem
        .slice(color_at, pixels.checked_mul(color_bpp).ok_or("color framebuffer is too large")?)
        .ok_or("color framebuffer not mapped")?;
    let mut color = Vec::with_capacity(pixels as usize * 4);
    for row in 0..height {
        for x in 0..width {
            let at = tiled_offset(x, row, width) as usize * color_bpp as usize;
            color.extend_from_slice(&decode_pixel(color_format, &color_raw[at..at + color_bpp as usize]));
        }
    }

    let mut depth = vec![1.0; pixels as usize];
    if depth_address != 0 {
        let depth_bpp = match depth_format {
            0 => 2,
            2 => 3,
            _ => 4,
        };
        let depth_at = pica.pa_to_va(depth_address)?;
        let depth_raw = mem
            .slice(depth_at, pixels.checked_mul(depth_bpp).ok_or("depth framebuffer is too large")?)
            .ok_or("depth framebuffer not mapped")?;
        let value_bytes = depth_bpp.min(3) as usize;
        let maximum = if depth_format == 0 { 0xFFFF } else { 0xFF_FFFF } as f32;
        for row in 0..height {
            for x in 0..width {
                let pixel = tiled_offset(x, row, width) as usize;
                let at = pixel * depth_bpp as usize;
                let mut bytes = [0u8; 4];
                bytes[..value_bytes].copy_from_slice(&depth_raw[at..at + value_bytes]);
                depth[(row * width + x) as usize] = u32::from_le_bytes(bytes) as f32 / maximum;
            }
        }
    }
    Ok(TargetSnapshot { width, height, color, depth })
}

pub(crate) fn capture_if_enabled(
    pica: &mut Pica,
    mem: &Memory,
    shaded: &[Vertex],
    triangles: &[[usize; 3]],
    target_info: CaptureTarget,
) -> Result<Option<usize>, String> {
    if pica.vulkan_capture_remaining == 0 || triangles.is_empty() {
        return Ok(None);
    }
    if pica.vulkan_capture_skip != 0 {
        pica.vulkan_capture_skip -= 1;
        return Ok(None);
    }
    // A live command-list replay only needs the target contents from before
    // the first draw to a framebuffer. Share that immutable seed across all
    // later draws instead of decoding and copying the same buffers each time.
    let target = if pica.vulkan_live {
        match pica.vulkan_snapshots.iter().find(|snapshot| snapshot.target_info == target_info) {
            Some(snapshot) => Arc::clone(&snapshot.target),
            None => Arc::new(target_snapshot(pica, mem, target_info)?),
        }
    } else {
        Arc::new(target_snapshot(pica, mem, target_info)?)
    };
    let textures = std::array::from_fn(|unit| {
        super::texture::decoded_snapshot(pica, mem, unit).map(|snapshot| TextureSnapshot {
            address: snapshot.address,
            source_size: snapshot.source_size,
            format: snapshot.format,
            width: snapshot.width,
            height: snapshot.height,
            pixels: snapshot.pixels,
            sampler: SamplerState::from_pica(snapshot.parameters),
            border: snapshot.border,
        })
    });
    let snapshot = DrawSnapshot::from_shaded_vertices_shared(pica, shaded, triangles, target_info, target, textures)?;
    pica.vulkan_capture_remaining -= 1;
    pica.vulkan_snapshots.push(snapshot);
    Ok(Some(pica.vulkan_snapshots.len() - 1))
}

pub(crate) fn complete_capture(
    pica: &mut Pica,
    mem: &Memory,
    index: Option<usize>,
    target_info: CaptureTarget,
) -> Result<(), String> {
    let Some(index) = index else { return Ok(()) };
    // Live replay writes its Vulkan result over the software result and does
    // not compare individual draws, so retaining a second full framebuffer is
    // pure overhead. Normal comparison captures still keep it.
    if pica.vulkan_live {
        return Ok(());
    }
    let result = target_snapshot(pica, mem, target_info)?;
    pica.vulkan_snapshots.get_mut(index).ok_or("Vulkan capture index was lost")?.software_result = Some(result);
    Ok(())
}

fn write_color_result(pica: &Pica, mem: &mut Memory, target: CaptureTarget, color: &[u8]) -> Result<(), String> {
    let pixels = target.width.checked_mul(target.height).ok_or("framebuffer is too large")?;
    if color.len() != pixels as usize * 4 {
        return Err("Vulkan color result has the wrong size".into());
    }
    let bytes_per_pixel = bytes_per_pixel(target.color_format).ok_or("unsupported Vulkan live color format")?;
    let mut tiled = vec![0; pixels as usize * bytes_per_pixel];
    for row in 0..target.height {
        for x in 0..target.width {
            let source = (row * target.width + x) as usize * 4;
            let encoded = encode_pixel(target.color_format, color[source..source + 4].try_into().expect("RGBA pixel"));
            let destination = tiled_offset(x, row, target.width) as usize * bytes_per_pixel;
            tiled[destination..destination + bytes_per_pixel].copy_from_slice(&encoded);
        }
    }
    let address = pica.pa_to_va(target.color_address)?;
    mem.device_write(address, &tiled).map_err(|fault| format!("Vulkan live color buffer: {fault}"))
}

fn write_depth_result(pica: &Pica, mem: &mut Memory, target: CaptureTarget, depth: &[f32]) -> Result<(), String> {
    if target.depth_address == 0 {
        return Ok(());
    }
    let pixels = target.width.checked_mul(target.height).ok_or("framebuffer is too large")?;
    if depth.len() != pixels as usize {
        return Err("Vulkan depth result has the wrong size".into());
    }
    let bytes_per_pixel = match target.depth_format {
        0 => 2,
        2 => 3,
        _ => 4,
    };
    let address = pica.pa_to_va(target.depth_address)?;
    let mut tiled = mem
        .slice(address, pixels.checked_mul(bytes_per_pixel).ok_or("depth framebuffer is too large")?)
        .ok_or("depth framebuffer not mapped")?
        .to_vec();
    let maximum = if target.depth_format == 0 { 0xFFFF } else { 0xFF_FFFF } as f32;
    let value_bytes = bytes_per_pixel.min(3) as usize;
    for row in 0..target.height {
        for x in 0..target.width {
            let source = (row * target.width + x) as usize;
            let value = (depth[source].clamp(0.0, 1.0) * maximum) as u32;
            let destination = tiled_offset(x, row, target.width) as usize * bytes_per_pixel as usize;
            tiled[destination..destination + value_bytes].copy_from_slice(&value.to_le_bytes()[..value_bytes]);
        }
    }
    mem.device_write(address, &tiled).map_err(|fault| format!("Vulkan live depth buffer: {fault}"))
}

/// Reinterprets a rendered D24S8 target exactly as PICA texture format RGBA8.
/// Depth is replaced by the GPU result while stencil remains the guest byte;
/// rows are bottom-first, matching decoded texture snapshots.
#[cfg(test)]
fn d24s8_rgba8_texture(pica: &Pica, mem: &Memory, target: CaptureTarget, depth: &[f32]) -> Result<Arc<[u8]>, String> {
    if target.depth_format != 3 || target.depth_address == 0 {
        return Err("depth feedback needs a D24S8 target".into());
    }
    let pixels = target.width.checked_mul(target.height).ok_or("depth feedback target is too large")?;
    if depth.len() != pixels as usize {
        return Err("depth feedback result has the wrong size".into());
    }
    let address = pica.pa_to_va(target.depth_address)?;
    let raw = mem
        .slice(address, pixels.checked_mul(4).ok_or("depth feedback target is too large")?)
        .ok_or("depth feedback target is not mapped")?;
    let mut decoded = Vec::with_capacity(pixels as usize * 4);
    for y in 0..target.height {
        let row = target.height - 1 - y;
        for x in 0..target.width {
            let source = (row * target.width + x) as usize;
            let raw_at = tiled_offset(x, row, target.width) as usize * 4;
            let value = (depth[source].clamp(0.0, 1.0) * 0xFF_FFFF as f32) as u32;
            decoded.extend_from_slice(&[raw[raw_at + 3], (value >> 16) as u8, (value >> 8) as u8, value as u8]);
        }
    }
    Ok(decoded.into())
}

fn d24s8_stencil(pica: &Pica, mem: &Memory, target: CaptureTarget) -> Result<Vec<u8>, String> {
    if target.depth_format != 3 || target.depth_address == 0 {
        return Err("depth feedback needs a D24S8 target".into());
    }
    let pixels = target.width.checked_mul(target.height).ok_or("depth feedback target is too large")?;
    let address = pica.pa_to_va(target.depth_address)?;
    let raw = mem
        .slice(address, pixels.checked_mul(4).ok_or("depth feedback target is too large")?)
        .ok_or("depth feedback target is not mapped")?;
    let mut stencil = Vec::with_capacity(pixels as usize);
    for row in 0..target.height {
        for x in 0..target.width {
            stencil.push(raw[tiled_offset(x, row, target.width) as usize * 4 + 3]);
        }
    }
    Ok(stencil)
}

/// Replays the current command list's captured draws in Vulkan, batching all
/// draws for each target into one submission, and writes only color back.
/// Software depth remains authoritative during this experimental live mode.
pub(crate) fn replay_live(pica: &mut Pica, mem: &mut Memory) -> Result<(usize, usize, FeedbackStats), String> {
    static RENDERER: std::sync::OnceLock<Result<Renderer, String>> = std::sync::OnceLock::new();
    let renderer = RENDERER.get_or_init(Renderer::new).as_ref().map_err(Clone::clone)?;
    let mut snapshots = pica.take_vulkan_snapshots();
    let draws = snapshots.len();
    let feedback = feedback_reads_in(&snapshots, pica)?;
    if feedback.reads != 0 && (!pica.vulkan_live_depth || !feedback_supported_in(&snapshots, pica)?) {
        return Err("unsupported render-target texture feedback".into());
    }
    let mut latest = Vec::new();
    let mut pending = Vec::new();
    let mut written = Vec::new();
    let mut feedback_textures = Vec::new();
    let mut targets = 0;
    for mut snapshot in snapshots.drain(..) {
        let mut depends_on_pending = false;
        for texture in snapshot.textures.iter().flatten() {
            for &target in &written {
                depends_on_pending |= depth_texture_matches(pica, texture, target)?;
            }
        }
        if depends_on_pending {
            targets += render_live_segment(pica, renderer, std::mem::take(&mut pending), &mut latest, &feedback_textures, None)?;
            for target in &written {
                if target.depth_address != 0 {
                    let address = pica.pa_to_va(target.depth_address)?;
                    feedback_textures.retain(|entry| entry.address != address);
                }
            }
            written.clear();
        }
        for texture in snapshot.textures.iter_mut().flatten() {
            let mut dependency = None;
            for (index, result) in latest.iter().enumerate() {
                if depth_texture_matches(pica, texture, *result)? {
                    dependency = Some(index);
                    break;
                }
            }
            if let Some(index) = dependency {
                let info = latest[index];
                if !feedback_textures.iter().any(|entry| {
                    entry.address == texture.address && entry.sampler == texture.sampler && entry.border == texture.border
                }) {
                    let target = pica
                        .vulkan_live_target_cache
                        .iter()
                        .find(|target| target.info == info)
                        .ok_or("depth feedback target was not created")?;
                    let stencil = d24s8_stencil(pica, mem, info)?;
                    let gpu = renderer.create_d24s8_feedback_texture(&target.gpu, &stencil, texture.sampler, texture.border)?;
                    feedback_textures.push(LiveFeedbackTexture {
                        address: texture.address,
                        sampler: texture.sampler,
                        border: texture.border,
                        gpu: Arc::new(gpu),
                    });
                }
            }
        }
        written.push(snapshot.target_info);
        pending.push(snapshot);
    }
    let mut results = Vec::new();
    targets += render_live_segment(pica, renderer, pending, &mut latest, &feedback_textures, Some(&mut results))?;
    // Do not modify guest buffers until every segment succeeded, so an
    // accelerated list can still rerun in software after a GPU error.
    for info in latest {
        if results.iter().any(|result| result.info == info) {
            continue;
        }
        let target = pica
            .vulkan_live_target_cache
            .iter()
            .find(|target| target.info == info)
            .ok_or("live target disappeared before readback")?;
        let (color, depth) = if pica.vulkan_live_depth {
            let (color, depth) = renderer.read_color_and_depth_target(&target.gpu)?;
            (color, Some(depth))
        } else {
            (renderer.read_color_target(&target.gpu)?, None)
        };
        results.push(LiveResult { info, color, depth });
    }
    for result in results {
        write_color_result(pica, mem, result.info, &result.color)?;
        if let Some(depth) = result.depth {
            write_depth_result(pica, mem, result.info, &depth)?;
        }
    }
    Ok((draws, targets, feedback))
}

struct LiveResult {
    info: CaptureTarget,
    color: Vec<u8>,
    depth: Option<Vec<f32>>,
}

fn depth_texture_matches(pica: &Pica, texture: &TextureSnapshot, target: CaptureTarget) -> Result<bool, String> {
    if target.depth_address == 0 || target.depth_format != 3 || texture.format != 0 {
        return Ok(false);
    }
    Ok(texture.address == pica.pa_to_va(target.depth_address)?
        && texture.width == target.width
        && texture.height == target.height)
}

fn render_live_segment(
    pica: &mut Pica,
    renderer: &Renderer,
    snapshots: Vec<DrawSnapshot>,
    latest: &mut Vec<CaptureTarget>,
    feedback: &[LiveFeedbackTexture],
    mut final_results: Option<&mut Vec<LiveResult>>,
) -> Result<usize, String> {
    if snapshots.is_empty() {
        return Ok(0);
    }
    let mut groups: Vec<(CaptureTarget, Vec<DrawSnapshot>)> = Vec::new();
    for snapshot in snapshots {
        match groups.iter_mut().find(|(target, _)| *target == snapshot.target_info) {
            Some((_, draws)) => draws.push(snapshot),
            None => groups.push((snapshot.target_info, vec![snapshot])),
        }
    }
    let targets = groups.len();
    for (target_info, draws) in groups {
        let initialized = latest.contains(&target_info);
        let initial = &draws[0].target;
        let target_index = match pica.vulkan_live_target_cache.iter().position(|target| target.info == target_info) {
            Some(index) => index,
            None => {
                let target = Arc::new(renderer.create_color_target(initial.width, initial.height)?);
                pica.vulkan_live_target_cache.push(LiveTarget { info: target_info, gpu: target });
                pica.vulkan_live_target_cache.len() - 1
            }
        };
        let target = Arc::clone(&pica.vulkan_live_target_cache[target_index].gpu);
        // A target already written in this list stays resident across feedback
        // barriers. Only the first segment imports the guest snapshot.
        let mut batch = if initialized {
            renderer.begin_batch(&target)
        } else {
            renderer.begin_batch_from_snapshot(&target, &initial.color, &initial.depth)?
        };
        for draw in &draws {
            draw.record_live(renderer, &mut batch, &mut pica.vulkan_live_resources, feedback)?;
        }
        if let Some(results) = final_results.as_deref_mut() {
            let (color, depth) = if pica.vulkan_live_depth {
                let (color, depth) = batch.finish_and_read_color_and_depth()?;
                (color, Some(depth))
            } else {
                batch.finish()?;
                (renderer.read_color_target(&target)?, None)
            };
            results.push(LiveResult { info: target_info, color, depth });
        } else {
            batch.submit()?;
        }
        if !initialized {
            latest.push(target_info);
        }
    }
    Ok(targets)
}

#[derive(Clone, Copy)]
enum FeedbackTarget {
    Color(CaptureTarget),
    Depth(CaptureTarget),
}

fn compatible_color_texture(texture: &TextureSnapshot, target: CaptureTarget) -> bool {
    let texture_format = match target.color_format {
        0 | 1 | 4 => target.color_format,
        2 => 3,
        3 => 2,
        _ => return false,
    };
    texture.format == texture_format && texture.width == target.width && texture.height == target.height
}

fn feedback_reads_in(snapshots: &[DrawSnapshot], pica: &Pica) -> Result<FeedbackStats, String> {
    let mut prior_targets: Vec<(u32, u32, FeedbackTarget)> = Vec::new();
    let mut stats = FeedbackStats::default();
    for snapshot in snapshots {
        for texture in snapshot.textures.iter().flatten() {
            let mut category = None;
            for &(address, size, target) in &prior_targets {
                if !ranges_overlap(texture.address, texture.source_size, address, size) {
                    continue;
                }
                let candidate = match target {
                    FeedbackTarget::Color(info) if texture.address == address && compatible_color_texture(texture, info) => 0,
                    FeedbackTarget::Color(_) if texture.address == address => 1,
                    FeedbackTarget::Depth(_) if texture.address == address => 2,
                    _ => 3,
                };
                category = Some(category.map_or(candidate, |current: u8| current.min(candidate)));
                if category == Some(0) {
                    break;
                }
            }
            match category {
                Some(0) => stats.compatible_color += 1,
                Some(1) => stats.other_color += 1,
                Some(2) => {
                    stats.depth += 1;
                    if let Some((_, _, FeedbackTarget::Depth(info))) = prior_targets
                        .iter()
                        .find(|&&(address, _, target)| texture.address == address && matches!(target, FeedbackTarget::Depth(_)))
                    {
                        if texture.format < 14 && info.depth_format < 4 {
                            stats.depth_pairs[(texture.format * 4 + info.depth_format) as usize] += 1;
                        }
                    }
                }
                Some(3) => stats.partial += 1,
                _ => continue,
            }
            stats.reads += 1;
        }
        let target = snapshot.target_info;
        let pixels = target.width.checked_mul(target.height).ok_or("framebuffer is too large")?;
        let color_size = pixels
            .checked_mul(bytes_per_pixel(target.color_format).ok_or("unsupported Vulkan live color format")? as u32)
            .ok_or("color framebuffer is too large")?;
        prior_targets.push((pica.pa_to_va(target.color_address)?, color_size, FeedbackTarget::Color(target)));
        if target.depth_address != 0 {
            let depth_bpp = match target.depth_format {
                0 => 2,
                2 => 3,
                _ => 4,
            };
            let depth_size = pixels.checked_mul(depth_bpp).ok_or("depth framebuffer is too large")?;
            prior_targets.push((pica.pa_to_va(target.depth_address)?, depth_size, FeedbackTarget::Depth(target)));
        }
    }
    Ok(stats)
}

fn ranges_overlap(a: u32, a_size: u32, b: u32, b_size: u32) -> bool {
    let (a, a_end) = (u64::from(a), u64::from(a) + u64::from(a_size));
    let (b, b_end) = (u64::from(b), u64::from(b) + u64::from(b_size));
    a < b_end && b < a_end
}

pub(crate) fn pending_feedback_reads(pica: &Pica) -> Result<FeedbackStats, String> {
    feedback_reads_in(&pica.vulkan_snapshots, pica)
}

pub(crate) fn pending_feedback_supported(pica: &Pica) -> Result<bool, String> {
    feedback_supported_in(&pica.vulkan_snapshots, pica)
}

fn feedback_supported_in(snapshots: &[DrawSnapshot], pica: &Pica) -> Result<bool, String> {
    let mut prior_targets: Vec<CaptureTarget> = Vec::new();
    for snapshot in snapshots {
        for texture in snapshot.textures.iter().flatten() {
            for &target in &prior_targets {
                let pixels = target.width.checked_mul(target.height).ok_or("framebuffer is too large")?;
                let color_size = pixels
                    .checked_mul(bytes_per_pixel(target.color_format).ok_or("unsupported Vulkan live color format")? as u32)
                    .ok_or("color framebuffer is too large")?;
                let color_address = pica.pa_to_va(target.color_address)?;
                if ranges_overlap(texture.address, texture.source_size, color_address, color_size) {
                    return Ok(false);
                }
                if target.depth_address == 0 {
                    continue;
                }
                let depth_bpp = match target.depth_format {
                    0 => 2,
                    2 => 3,
                    _ => 4,
                };
                let depth_size = pixels.checked_mul(depth_bpp).ok_or("depth framebuffer is too large")?;
                let depth_address = pica.pa_to_va(target.depth_address)?;
                if ranges_overlap(texture.address, texture.source_size, depth_address, depth_size)
                    && !(texture.address == depth_address
                        && texture.format == 0
                        && target.depth_format == 3
                        && texture.width == target.width
                        && texture.height == target.height)
                {
                    return Ok(false);
                }
            }
        }
        prior_targets.push(snapshot.target_info);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use usum_guest::Perm;

    #[test]
    fn snapshot_owns_triangle_order_and_complete_state() {
        let mut pica = Pica::default();
        pica.regs[0x41] = 0x430000; // float24 16.0
        pica.regs[0x43] = 0x430000;
        pica.regs[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
        pica.regs[0x113] = 1;
        pica.regs[0x115] = 2;
        let vertices = [
            Vertex { position: [-1.0, -1.0, 0.0, 1.0], color: [1.0, 0.0, 0.0, 1.0], ..Vertex::default() },
            Vertex { position: [0.0, 1.0, 0.0, 1.0], color: [0.0, 1.0, 0.0, 1.0], ..Vertex::default() },
            Vertex {
                position: [1.0, -1.0, 0.0, 1.0],
                color: [0.0, 0.0, 1.0, 1.0],
                texcoord: [[0.0; 3], [0.25, 0.75, 0.0], [0.5, 0.125, 0.0]],
                ..Vertex::default()
            },
        ];
        let target_info = CaptureTarget {
            color_address: 0x2000_0000,
            depth_address: 0,
            width: 32,
            height: 32,
            color_format: 0,
            depth_format: 0,
        };
        let target = TargetSnapshot { width: 32, height: 32, color: vec![0; 32 * 32 * 4], depth: vec![1.0; 32 * 32] };
        let snapshot =
            DrawSnapshot::from_shaded_vertices(&pica, &vertices, &[[2, 0, 1]], target_info, target.clone(), [None, None, None])
                .unwrap();
        assert_eq!(snapshot.vertices.len(), 3);
        assert_eq!(snapshot.vertices[0].color, vertices[2].color);
        assert_eq!(snapshot.vertices[1].position, vertices[0].position);
        assert_eq!(snapshot.state.viewport.width, 32.0);
        assert!(snapshot.state.raster.depth.write);
        assert_eq!(snapshot.vertices[0].texcoord2, [0.5, 0.125]);
        pica.regs[0x80] |= 1 << 13;
        let routed =
            DrawSnapshot::from_shaded_vertices(&pica, &vertices, &[[2, 0, 1]], target_info, target.clone(), [None, None, None])
                .unwrap();
        assert_eq!(routed.vertices[0].texcoord2, [0.25, 0.75]);
        // A later register change cannot alter the previous draw's coordinates.
        assert_eq!(snapshot.vertices[0].texcoord2, [0.5, 0.125]);

        assert!(
            DrawSnapshot::from_shaded_vertices(&pica, &vertices, &[[0, 1, 3]], target_info, target, [None, None, None]).is_err()
        );

        let mut memory = Memory::new();
        memory.map(pica.linear_base, vec![0; 32 * 32 * 4], Perm::RW).unwrap();
        pica.enable_vulkan_capture_after(1, 1);
        let target = target_info;
        let skipped = capture_if_enabled(&mut pica, &memory, &vertices, &[[2, 1, 0]], target).unwrap();
        assert_eq!(skipped, None);
        let index = capture_if_enabled(&mut pica, &memory, &vertices, &[[0, 1, 2]], target).unwrap();
        complete_capture(&mut pica, &memory, index, target).unwrap();
        let second = capture_if_enabled(&mut pica, &memory, &vertices, &[[2, 1, 0]], target).unwrap();
        assert_eq!(second, None);
        let captured = pica.take_vulkan_snapshots();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].vertices[0].position, vertices[0].position);
        assert_eq!(captured[0].target.color.len(), 32 * 32 * 4);
        assert!(captured[0].target.depth.iter().all(|&depth| depth == 1.0));
        assert!(captured[0].software_result.is_some());
    }

    #[test]
    fn live_depth_writeback_tiles_values_and_preserves_stencil() {
        let pica = Pica::default();
        let mut memory = Memory::new();
        memory.map(pica.linear_base, vec![0xA5; 8 * 8 * 4], Perm::RW).unwrap();
        let target =
            CaptureTarget { color_address: 0, depth_address: 0x2000_0000, width: 8, height: 8, color_format: 0, depth_format: 3 };
        let mut depth = vec![0.0; 8 * 8];
        depth[1] = 0.5;
        depth[2] = 1.0;

        write_depth_result(&pica, &mut memory, target, &depth).unwrap();
        let raw = memory.slice(pica.linear_base, 8 * 8 * 4).unwrap();
        for (x, expected) in [(0, 0), (1, 0x7F_FFFF), (2, 0xFF_FFFF)] {
            let at = tiled_offset(x, 0, 8) as usize * 4;
            assert_eq!(&raw[at..at + 3], &u32::to_le_bytes(expected)[..3]);
            assert_eq!(raw[at + 3], 0xA5);
        }
    }

    #[test]
    fn d24s8_feedback_decodes_depth_and_stencil_as_rgba8() {
        let pica = Pica::default();
        let mut memory = Memory::new();
        memory.map(pica.linear_base, vec![0xA5; 8 * 8 * 4], Perm::RW).unwrap();
        let target =
            CaptureTarget { color_address: 0, depth_address: 0x2000_0000, width: 8, height: 8, color_format: 0, depth_format: 3 };
        let mut depth = vec![0.0; 8 * 8];
        depth[7 * 8] = 0.5;
        depth[6 * 8] = 1.0;

        let texture = d24s8_rgba8_texture(&pica, &memory, target, &depth).unwrap();
        assert_eq!(&texture[0..4], &[0xA5, 0x7F, 0xFF, 0xFF]);
        assert_eq!(&texture[8 * 4..8 * 4 + 4], &[0xA5, 0xFF, 0xFF, 0xFF]);
        assert_eq!(&texture[(7 * 8 * 4)..][..4], &[0xA5, 0, 0, 0]);
    }

    #[test]
    fn feedback_detection_finds_a_texture_reading_an_earlier_target() {
        let mut pica = Pica::default();
        pica.regs[0x41] = 0x430000;
        pica.regs[0x43] = 0x430000;
        let target_info = CaptureTarget {
            color_address: 0x2000_0000,
            depth_address: 0,
            width: 32,
            height: 32,
            color_format: 0,
            depth_format: 0,
        };
        let target = TargetSnapshot { width: 32, height: 32, color: vec![0; 32 * 32 * 4], depth: vec![1.0; 32 * 32] };
        let vertices = [
            Vertex { position: [-1.0, -1.0, 0.0, 1.0], ..Vertex::default() },
            Vertex { position: [0.0, 1.0, 0.0, 1.0], ..Vertex::default() },
            Vertex { position: [1.0, -1.0, 0.0, 1.0], ..Vertex::default() },
        ];
        let first =
            DrawSnapshot::from_shaded_vertices(&pica, &vertices, &[[0, 1, 2]], target_info, target.clone(), [None, None, None])
                .unwrap();
        let texture = TextureSnapshot {
            address: pica.pa_to_va(target_info.color_address).unwrap(),
            source_size: 32 * 32 * 4,
            format: 0,
            width: 32,
            height: 32,
            pixels: vec![255; 4].into(),
            sampler: SamplerState::default(),
            border: [0; 4],
        };
        let second =
            DrawSnapshot::from_shaded_vertices(&pica, &vertices, &[[0, 1, 2]], target_info, target, [Some(texture), None, None])
                .unwrap();
        pica.vulkan_snapshots = vec![first, second];
        assert_eq!(
            pending_feedback_reads(&pica).unwrap(),
            FeedbackStats { reads: 1, compatible_color: 1, ..FeedbackStats::default() }
        );
    }

    #[test]
    fn feedback_detection_finds_partial_color_and_depth_aliases() {
        assert!(ranges_overlap(0x1004, 4, 0x1000, 8));
        assert!(ranges_overlap(0x0FFC, 8, 0x1000, 8));
        assert!(!ranges_overlap(0x0FF8, 8, 0x1000, 8));
        assert!(!ranges_overlap(0x1008, 8, 0x1000, 8));
    }

    #[test]
    fn lighting_cache_matches_equal_luts_with_distinct_allocations() {
        let mut pica = Pica::default();
        pica.regs[0x41] = 0x430000;
        pica.regs[0x43] = 0x430000;
        pica.regs[0x8F] = 1;
        let first = PicaDrawState::from_pica_registers_shared(&pica.regs, Arc::clone(&pica.light_luts), 32, 32)
            .unwrap()
            .lighting
            .unwrap();
        let second = PicaDrawState::from_pica_registers_shared(&pica.regs, Arc::new((*pica.light_luts).clone()), 32, 32)
            .unwrap()
            .lighting
            .unwrap();
        assert!(!Arc::ptr_eq(&first.luts, &second.luts));
        assert!(same_lighting_state(&first, &second));
    }
}
