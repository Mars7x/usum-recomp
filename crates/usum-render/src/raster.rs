mod color;
mod depth;
mod display;
mod pack;
mod readback;
mod seed;
mod stencil;
pub use stencil::{StencilOperation, StencilState};
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32FloatStencil8;
pub use display::DisplayCopy;
pub use readback::{ColorDepthReadback, PendingReadback};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use super::Renderer;

/// The output of the emulated PICA vertex/geometry stages needed by the
/// first Vulkan raster path. Positions are clip-space coordinates; colors
/// are perspective-correct interpolated by the host GPU.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    pub texcoord0: [f32; 2],
    pub texcoord1: [f32; 2],
    pub texcoord2: [f32; 2],
    pub quaternion: [f32; 4],
    pub view: [f32; 3],
}

impl RasterVertex {
    pub const fn new(position: [f32; 4], color: [f32; 4]) -> Self {
        Self {
            position,
            color,
            texcoord0: [0.0; 2],
            texcoord1: [0.0; 2],
            texcoord2: [0.0; 2],
            quaternion: [0.0, 0.0, 0.0, 1.0],
            view: [0.0, 0.0, 1.0],
        }
    }

    pub const fn with_texcoord0(mut self, texcoord0: [f32; 2]) -> Self {
        self.texcoord0 = texcoord0;
        self
    }

    pub const fn with_texcoord1(mut self, texcoord1: [f32; 2]) -> Self {
        self.texcoord1 = texcoord1;
        self
    }

    pub const fn with_texcoord2(mut self, texcoord2: [f32; 2]) -> Self {
        self.texcoord2 = texcoord2;
        self
    }

    pub const fn with_lighting_vectors(mut self, quaternion: [f32; 4], view: [f32; 3]) -> Self {
        self.quaternion = quaternion;
        self.view = view;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrapMode {
    #[default]
    ClampToEdge,
    ClampToBorder,
    Repeat,
    MirrorRepeat,
}

impl WrapMode {
    pub fn from_pica(value: u32) -> Self {
        match value & 7 {
            0 => Self::ClampToEdge,
            1 => Self::ClampToBorder,
            2 => Self::Repeat,
            _ => Self::MirrorRepeat,
        }
    }

    fn wgpu(self, border_supported: bool) -> Result<wgpu::AddressMode, String> {
        match self {
            Self::ClampToEdge => Ok(wgpu::AddressMode::ClampToEdge),
            Self::Repeat => Ok(wgpu::AddressMode::Repeat),
            Self::MirrorRepeat => Ok(wgpu::AddressMode::MirrorRepeat),
            Self::ClampToBorder if border_supported => Ok(wgpu::AddressMode::ClampToBorder),
            Self::ClampToBorder => Err("arbitrary PICA texture border colors need manual shader sampling".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterMode {
    #[default]
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SamplerState {
    pub wrap_u: WrapMode,
    pub wrap_v: WrapMode,
    pub filter: FilterMode,
}

impl SamplerState {
    pub fn from_pica(parameters: u32) -> Self {
        Self {
            wrap_u: WrapMode::from_pica(parameters >> 12),
            wrap_v: WrapMode::from_pica(parameters >> 8),
            filter: if parameters >> 1 & 1 == 1 { FilterMode::Linear } else { FilterMode::Nearest },
        }
    }
}

#[derive(Debug)]
pub struct SampledTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FragmentInputs<'a> {
    pub texture0: Option<&'a SampledTexture>,
    pub texture1: Option<&'a SampledTexture>,
    pub texture2: Option<&'a SampledTexture>,
    pub lighting: Option<&'a GpuLighting>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightingLight {
    pub id: u32,
    pub specular0: [f32; 3],
    pub specular1: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub position: [f32; 3],
    pub spot_direction: [f32; 3],
    pub directional: bool,
    pub two_sided: bool,
    pub geometric: [bool; 2],
    pub attenuation_bias: f32,
    pub attenuation_scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LightingState {
    pub lights: Vec<LightingLight>,
    pub ambient: [f32; 3],
    pub config0: u32,
    pub config1: u32,
    pub abs: u32,
    pub select: u32,
    pub scale: u32,
    /// Up to 24 PICA lookup tables, each containing 256 packed value/delta entries.
    pub luts: Arc<Vec<[u32; 256]>>,
}

impl LightingState {
    pub fn from_pica_registers(registers: &[u32], luts: &[[u32; 256]]) -> Result<Option<Self>, String> {
        Self::from_pica_registers_shared(registers, Arc::new(luts.to_vec()))
    }

    pub fn from_pica_registers_shared(registers: &[u32], luts: Arc<Vec<[u32; 256]>>) -> Result<Option<Self>, String> {
        let register = |index: usize| registers.get(index).copied().ok_or_else(|| format!("missing PICA register {index:#x}"));
        if register(0x8F)? & 1 == 0 {
            return Ok(None);
        }
        if luts.len() > LUT_COUNT {
            return Err("PICA supports exactly 24 fragment-lighting lookup tables".into());
        }
        let count = (register(0x1C2)? & 7) as usize + 1;
        let permutation = register(0x1D9)?;
        let mut lights = Vec::with_capacity(count);
        for slot in 0..count {
            let id = (permutation >> (4 * slot) & 7) as usize;
            let base = 0x140 + 0x10 * id;
            let xy = register(base + 4)?;
            let spot_xy = register(base + 6)?;
            let config = register(base + 9)?;
            lights.push(LightingLight {
                id: id as u32,
                specular0: lighting_color(register(base)?),
                specular1: lighting_color(register(base + 1)?),
                diffuse: lighting_color(register(base + 2)?),
                ambient: lighting_color(register(base + 3)?),
                position: [lighting_f16(xy), lighting_f16(xy >> 16), lighting_f16(register(base + 5)?)],
                spot_direction: [
                    lighting_fixed13(spot_xy),
                    lighting_fixed13(spot_xy >> 16),
                    lighting_fixed13(register(base + 7)?),
                ],
                directional: config & 1 != 0,
                two_sided: config & 2 != 0,
                geometric: [config & 4 != 0, config & 8 != 0],
                attenuation_bias: lighting_f20(register(base + 0xA)?),
                attenuation_scale: lighting_f20(register(base + 0xB)?),
            });
        }
        Ok(Some(Self {
            lights,
            ambient: lighting_color(register(0x1C0)?),
            config0: register(0x1C3)?,
            config1: register(0x1C4)?,
            abs: register(0x1D0)?,
            select: register(0x1D1)?,
            scale: register(0x1D2)?,
            luts,
        }))
    }
}

#[derive(Debug)]
pub struct GpuLighting {
    buffer: wgpu::Buffer,
}

/// The five raw register words that define one PICA texture-environment stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TevStage {
    pub sources: u32,
    pub operands: u32,
    pub combiner: u32,
    pub constant: u32,
    pub scale: u32,
}

impl TevStage {
    pub const fn replace(source: u32) -> Self {
        Self { sources: source | source << 16, operands: 0, combiner: 0, constant: 0, scale: 0 }
    }
}

/// The six PICA texture-environment stages and their shared combiner buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TevState {
    pub stages: [TevStage; 6],
    pub buffer_color: u32,
    pub buffer_update: u32,
}

impl TevState {
    /// A bring-up configuration equivalent to primary color × texture unit 0.
    pub const fn modulate_texture0() -> Self {
        let mut stages = [TevStage::replace(15); 6];
        stages[0] = TevStage { sources: 3 << 4 | 3 << 20, operands: 0, combiner: 1 | 1 << 16, constant: 0, scale: 0 };
        Self { stages, buffer_color: 0, buffer_update: 0 }
    }

    pub fn from_pica_registers(registers: &[u32]) -> Result<Self, String> {
        const STAGES: [usize; 6] = [0xC0, 0xC8, 0xD0, 0xD8, 0xF0, 0xF8];
        let register = |index: usize| registers.get(index).copied().ok_or_else(|| format!("missing PICA register {index:#x}"));
        let mut stages = [TevStage::replace(15); 6];
        for (stage, base) in stages.iter_mut().zip(STAGES) {
            *stage = TevStage {
                sources: register(base)?,
                operands: register(base + 1)?,
                combiner: register(base + 2)?,
                constant: register(base + 3)?,
                scale: register(base + 4)?,
            };
        }
        Ok(Self { stages, buffer_color: register(0xFD)?, buffer_update: register(0xE0)? })
    }
}

impl Default for TevState {
    fn default() -> Self {
        let mut stages = [TevStage::replace(15); 6];
        stages[0] = TevStage::replace(0);
        Self { stages, buffer_color: 0, buffer_update: 0 }
    }
}

/// Persistent RGBA8, float depth and stencil host-GPU targets. Multiple PICA draws can
/// accumulate in them before a comparison readback or eventual presentation.
#[derive(Debug)]
pub struct ColorTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    depth_sample: wgpu::TextureView,
    stencil_sample: wgpu::TextureView,
    color_readback: wgpu::Buffer,
    depth_readback: wgpu::Buffer,
    width: u32,
    height: u32,
}

/// An immutable GPU-only depth/stencil snapshot, independent of a mutable framebuffer.
pub struct DepthSnapshot {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

impl ColorTarget {
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Multiple PICA draws recorded into one Vulkan command buffer and submitted
/// together. This is the path intended for real command lists.
pub struct RasterBatch<'a> {
    renderer: &'a Renderer,
    target: &'a ColorTarget,
    encoder: Option<wgpu::CommandEncoder>,
    errors: Option<wgpu::ErrorScopeGuard>,
    draws: u32,
}

enum VertexSource<'a> {
    Cpu(&'a [RasterVertex]),
    Pica(crate::PicaVertexDraw<'a>),
}

/// A host viewport in top-left target coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// PICA's triangle culling modes (register 0x40 bits 0-1).
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub enum CullMode {
    #[default]
    None,
    Front,
    Back,
}

/// PICA's eight depth comparison functions.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum DepthCompare {
    Never,
    Always,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl DepthCompare {
    pub fn from_pica(value: u32) -> Self {
        match value & 7 {
            0 => Self::Never,
            1 => Self::Always,
            2 => Self::Equal,
            3 => Self::NotEqual,
            4 => Self::Less,
            5 => Self::LessEqual,
            6 => Self::Greater,
            _ => Self::GreaterEqual,
        }
    }

    fn wgpu(self) -> wgpu::CompareFunction {
        match self {
            Self::Never => wgpu::CompareFunction::Never,
            Self::Always => wgpu::CompareFunction::Always,
            Self::Equal => wgpu::CompareFunction::Equal,
            Self::NotEqual => wgpu::CompareFunction::NotEqual,
            Self::Less => wgpu::CompareFunction::Less,
            Self::LessEqual => wgpu::CompareFunction::LessEqual,
            Self::Greater => wgpu::CompareFunction::Greater,
            Self::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        }
    }
}

/// Depth state decoded from GPUREG_DEPTH_COLOR_MASK (0x107) and
/// GPUREG_FRAMEBUFFER_INVALIDATE (0x115).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct DepthState {
    pub compare: DepthCompare,
    pub write: bool,
}

/// PICA alpha testing (register 0x104). The comparison encoding is identical
/// to the depth comparison encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlphaTest {
    pub compare: DepthCompare,
    pub reference: u8,
}

impl AlphaTest {
    pub fn from_pica(value: u32) -> Option<Self> {
        (value & 1 != 0).then(|| Self { compare: DepthCompare::from_pica(value >> 4), reference: (value >> 8) as u8 })
    }

    fn uniform(self) -> [u32; 4] {
        [1, self.compare as u32, u32::from(self.reference), 0]
    }
}

impl DepthState {
    /// Decodes the two PICA registers which jointly control the depth test and
    /// writes. A disabled test passes every fragment, independently of writes.
    pub fn from_pica(depth_color_mask: u32, framebuffer_permissions: u32) -> Self {
        let compare =
            if depth_color_mask & 1 == 0 { DepthCompare::Always } else { DepthCompare::from_pica(depth_color_mask >> 4) };
        let write = depth_color_mask >> 12 & 1 == 1 && framebuffer_permissions & 2 != 0;
        Self { compare, write }
    }

    pub const fn less_equal_write() -> Self {
        Self { compare: DepthCompare::LessEqual, write: true }
    }
}

impl CullMode {
    pub fn from_pica(value: u32) -> Self {
        match value & 3 {
            1 => Self::Front,
            2 => Self::Back,
            _ => Self::None,
        }
    }

    fn wgpu(self) -> Option<wgpu::Face> {
        match self {
            Self::None => None,
            Self::Front => Some(wgpu::Face::Front),
            Self::Back => Some(wgpu::Face::Back),
        }
    }
}

/// PICA's independent red, green, blue and alpha write enables.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ColorMask(u8);

impl ColorMask {
    pub const ALL: Self = Self(0xF);

    pub const fn from_bits(value: u32) -> Self {
        Self((value & 0xF) as u8)
    }

    /// Decodes the channel mask in GPUREG_DEPTH_COLOR_MASK together with the
    /// framebuffer-wide color-write permission in register 0x113.
    pub const fn from_pica(depth_color_mask: u32, color_write_permissions: u32) -> Self {
        if color_write_permissions & 0xF == 0 {
            Self(0)
        } else {
            Self::from_bits(depth_color_mask >> 8)
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    fn wgpu(self) -> wgpu::ColorWrites {
        wgpu::ColorWrites::from_bits_truncate(u32::from(self.0))
    }
}

/// One of PICA's source or destination blend factors.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    Src,
    OneMinusSrc,
    Dst,
    OneMinusDst,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    Constant,
    OneMinusConstant,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturated,
}

impl BlendFactor {
    pub fn from_pica(value: u32) -> Self {
        match value & 0xF {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Src,
            3 => Self::OneMinusSrc,
            4 => Self::Dst,
            5 => Self::OneMinusDst,
            6 => Self::SrcAlpha,
            7 => Self::OneMinusSrcAlpha,
            8 => Self::DstAlpha,
            9 => Self::OneMinusDstAlpha,
            10 => Self::Constant,
            11 => Self::OneMinusConstant,
            12 => Self::ConstantAlpha,
            13 => Self::OneMinusConstantAlpha,
            _ => Self::SrcAlphaSaturated,
        }
    }

    fn wgpu(self) -> wgpu::BlendFactor {
        match self {
            Self::Zero => wgpu::BlendFactor::Zero,
            Self::One => wgpu::BlendFactor::One,
            Self::Src => wgpu::BlendFactor::Src,
            Self::OneMinusSrc => wgpu::BlendFactor::OneMinusSrc,
            Self::Dst => wgpu::BlendFactor::Dst,
            Self::OneMinusDst => wgpu::BlendFactor::OneMinusDst,
            Self::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
            Self::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
            Self::DstAlpha => wgpu::BlendFactor::DstAlpha,
            Self::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
            Self::Constant => wgpu::BlendFactor::Constant,
            Self::OneMinusConstant => wgpu::BlendFactor::OneMinusConstant,
            Self::ConstantAlpha => wgpu::BlendFactor::Constant,
            Self::OneMinusConstantAlpha => wgpu::BlendFactor::OneMinusConstant,
            Self::SrcAlphaSaturated => wgpu::BlendFactor::SrcAlphaSaturated,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BlendOperation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

impl BlendOperation {
    pub fn from_pica(value: u32) -> Self {
        match value & 7 {
            1 => Self::Subtract,
            2 => Self::ReverseSubtract,
            3 => Self::Min,
            4 => Self::Max,
            _ => Self::Add,
        }
    }

    fn wgpu(self) -> wgpu::BlendOperation {
        match self {
            Self::Add => wgpu::BlendOperation::Add,
            Self::Subtract => wgpu::BlendOperation::Subtract,
            Self::ReverseSubtract => wgpu::BlendOperation::ReverseSubtract,
            Self::Min => wgpu::BlendOperation::Min,
            Self::Max => wgpu::BlendOperation::Max,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct BlendComponent {
    pub source: BlendFactor,
    pub destination: BlendFactor,
    pub operation: BlendOperation,
}

impl BlendComponent {
    fn wgpu(self) -> wgpu::BlendComponent {
        wgpu::BlendComponent {
            src_factor: self.source.wgpu(),
            dst_factor: self.destination.wgpu(),
            operation: self.operation.wgpu(),
        }
    }
}

/// PICA blend functions plus the dynamic RGBA blend constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendState {
    pub color: BlendComponent,
    pub alpha: BlendComponent,
    pub constant: [f32; 4],
}

impl BlendState {
    /// Returns no blend state when GPUREG_COLOR_OPERATION has blending off.
    pub fn from_pica(color_operation: u32, functions: u32, constant: u32) -> Option<Self> {
        if color_operation >> 8 & 1 == 0 {
            return None;
        }
        let color = BlendComponent {
            source: BlendFactor::from_pica(functions >> 16),
            destination: BlendFactor::from_pica(functions >> 20),
            operation: BlendOperation::from_pica(functions),
        };
        let alpha = BlendComponent {
            source: BlendFactor::from_pica(functions >> 24),
            destination: BlendFactor::from_pica(functions >> 28),
            operation: BlendOperation::from_pica(functions >> 8),
        };
        let constant = [0, 8, 16, 24].map(|shift| ((constant >> shift) & 0xFF) as f32 / 255.0);
        Some(Self { color, alpha, constant })
    }

    fn key(self) -> BlendKey {
        BlendKey { color: self.color, alpha: self.alpha }
    }

    fn host_constant(self) -> Result<[f32; 4], String> {
        let factors = [self.color.source, self.color.destination];
        let uses_color = factors.iter().any(|f| matches!(f, BlendFactor::Constant | BlendFactor::OneMinusConstant));
        let uses_alpha = factors.iter().any(|f| matches!(f, BlendFactor::ConstantAlpha | BlendFactor::OneMinusConstantAlpha));
        if uses_color && uses_alpha {
            return Err(
                "Vulkan fixed-function blending cannot use constant color and constant alpha as separate RGB factors".into()
            );
        }
        if uses_alpha {
            Ok([self.constant[3]; 4])
        } else {
            Ok(self.constant)
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct BlendKey {
    color: BlendComponent,
    alpha: BlendComponent,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct PipelineKey {
    vertex_program: u64,
    vertex_mapping: [u32; 10],
    cull: CullMode,
    depth: DepthState,
    stencil: StencilState,
    color_mask: ColorMask,
    blend: Option<BlendKey>,
}

pub(super) struct RasterPipelines {
    shader: wgpu::ShaderModule,
    fragment_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    vertex_layout: wgpu::BindGroupLayout,
    vertex_pipeline_layout: wgpu::PipelineLayout,
    vertex_shaders: Mutex<HashMap<(u64, [u32; 10]), wgpu::ShaderModule>>,
    depth_upload_layout: wgpu::BindGroupLayout,
    depth_upload_pipeline: wgpu::RenderPipeline,
    depth_feedback_layout: wgpu::BindGroupLayout,
    depth_feedback_pipeline: wgpu::RenderPipeline,
    color_feedback: color::ColorFeedback,
    resolved_readback: readback::ResolvePipeline,
    scaled_seed: seed::SeedPipeline,
    display_copy: display::DisplayPipeline,
    _dummy_texture: wgpu::Texture,
    dummy_view: wgpu::TextureView,
    dummy_sampler: wgpu::Sampler,
    dummy_lighting: wgpu::Buffer,
    pipelines: Mutex<HashMap<PipelineKey, wgpu::RenderPipeline>>,
}

/// Fixed raster state for one PICA draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterState {
    pub cull: CullMode,
    pub depth: DepthState,
    pub stencil: StencilState,
    /// Integer guest depth range; zero keeps native float precision.
    pub depth_max: u32,
    /// PICA's programmable mapping from signed clip Z/W to stored depth.
    pub depth_scale: f32,
    pub depth_offset: f32,
    pub depth_w_buffer: bool,
    pub color_mask: ColorMask,
    pub blend: Option<BlendState>,
    pub alpha_test: Option<AlphaTest>,
    pub tev: TevState,
}

/// The PICA registers needed to construct the currently supported fixed
/// raster state. Keeping the raw values together prevents similarly named
/// framebuffer permissions and channel masks from being interchanged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaRasterRegisters {
    pub cull_mode: u32,
    pub depth_color_mask: u32,
    pub color_write_permissions: u32,
    pub depth_write_permissions: u32,
    pub stencil_test: u32,
    pub stencil_operations: u32,
    pub depth_format: u32,
    pub color_operation: u32,
    pub blend_functions: u32,
    pub blend_constant: u32,
    pub alpha_test: u32,
}

/// Immutable renderer state decoded from one raw PICA register/LUT snapshot.
/// This is the handoff object for a diagnostic runtime mirror: it contains no
/// guest-memory references and cannot mutate the software renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaDrawState {
    pub viewport: Viewport,
    pub raster: RasterState,
    pub lighting: Option<LightingState>,
}

impl PicaDrawState {
    pub fn from_pica_registers(
        registers: &[u32],
        luts: &[[u32; 256]],
        target_width: u32,
        target_height: u32,
    ) -> Result<Self, String> {
        Self::from_pica_registers_shared(registers, Arc::new(luts.to_vec()), target_width, target_height)
    }

    pub fn from_pica_registers_shared(
        registers: &[u32],
        luts: Arc<Vec<[u32; 256]>>,
        target_width: u32,
        target_height: u32,
    ) -> Result<Self, String> {
        validate_target(target_width, target_height)?;
        let register = |index: usize| registers.get(index).copied().ok_or_else(|| format!("missing PICA register {index:#x}"));
        let xy = register(0x68)?;
        let signed_10 = |value: u32| ((value << 22) as i32 >> 22) as f32;
        let viewport = Viewport::from_pica_dimensions(
            target_height,
            pica_f24(register(0x41)?),
            pica_f24(register(0x43)?),
            signed_10(xy & 0x3FF),
            signed_10((xy >> 16) & 0x3FF),
        );
        viewport.validate_dimensions(target_width, target_height)?;
        let mut raster = RasterState::from_pica(PicaRasterRegisters::from_pica_registers(registers)?);
        raster.tev = TevState::from_pica_registers(registers)?;
        raster.depth_max = if register(0x116)? & 3 == 0 { 0xFFFF } else { 0xFF_FFFF };
        raster.depth_scale = pica_f24(register(0x4d)?);
        raster.depth_offset = pica_f24(register(0x4e)?);
        raster.depth_w_buffer = register(0x6d)? & 1 == 0;
        let lighting = LightingState::from_pica_registers_shared(registers, luts)?;
        Ok(Self { viewport, raster, lighting })
    }
}

impl PicaRasterRegisters {
    pub fn from_pica_registers(registers: &[u32]) -> Result<Self, String> {
        let register = |index: usize| registers.get(index).copied().ok_or_else(|| format!("missing PICA register {index:#x}"));
        Ok(Self {
            cull_mode: register(0x40)?,
            depth_color_mask: register(0x107)?,
            color_write_permissions: register(0x113)?,
            depth_write_permissions: register(0x115)?,
            stencil_test: register(0x105)?,
            stencil_operations: register(0x106)?,
            depth_format: register(0x116)?,
            color_operation: register(0x100)?,
            blend_functions: register(0x101)?,
            blend_constant: register(0x103)?,
            alpha_test: register(0x104)?,
        })
    }
}

impl RasterState {
    pub fn from_pica(registers: PicaRasterRegisters) -> Self {
        Self {
            cull: CullMode::from_pica(registers.cull_mode),
            depth: DepthState::from_pica(registers.depth_color_mask, registers.depth_write_permissions),
            stencil: StencilState::from_pica(
                registers.stencil_test,
                registers.stencil_operations,
                registers.depth_write_permissions,
                registers.depth_format,
            ),
            depth_max: 0,
            depth_scale: -1.0,
            depth_offset: 0.0,
            depth_w_buffer: false,
            color_mask: ColorMask::from_pica(registers.depth_color_mask, registers.color_write_permissions),
            blend: BlendState::from_pica(registers.color_operation, registers.blend_functions, registers.blend_constant),
            alpha_test: AlphaTest::from_pica(registers.alpha_test),
            tev: TevState::default(),
        }
    }
}

impl Default for RasterState {
    fn default() -> Self {
        Self {
            cull: CullMode::None,
            depth: DepthState::less_equal_write(),
            stencil: StencilState::default(),
            depth_max: 0,
            depth_scale: -1.0,
            depth_offset: 0.0,
            depth_w_buffer: false,
            color_mask: ColorMask::ALL,
            blend: None,
            alpha_test: None,
            tev: TevState::default(),
        }
    }
}

impl Viewport {
    pub fn full(target: &ColorTarget) -> Self {
        Self { x: 0.0, y: 0.0, width: target.width as f32, height: target.height as f32 }
    }

    /// Converts PICA's half-size and bottom-left offset registers to the
    /// top-left coordinate system used by wgpu render passes.
    pub fn from_pica(
        target: &ColorTarget,
        half_width: f32,
        half_height: f32,
        offset_x: f32,
        offset_y: f32,
    ) -> Result<Self, String> {
        let viewport = Self::from_pica_dimensions(target.height, half_width, half_height, offset_x, offset_y);
        viewport.validate(target)?;
        Ok(viewport)
    }

    fn from_pica_dimensions(target_height: u32, half_width: f32, half_height: f32, offset_x: f32, offset_y: f32) -> Self {
        Self {
            x: offset_x,
            y: target_height as f32 - offset_y - 2.0 * half_height,
            width: 2.0 * half_width,
            height: 2.0 * half_height,
        }
    }

    fn validate(self, target: &ColorTarget) -> Result<(), String> {
        self.validate_dimensions(target.width, target.height)
    }

    fn validate_dimensions(self, target_width: u32, target_height: u32) -> Result<(), String> {
        let values = [self.x, self.y, self.width, self.height];
        if values.iter().any(|v| !v.is_finite()) || self.x < 0.0 || self.y < 0.0 || self.width <= 0.0 || self.height <= 0.0 {
            return Err("viewport must be finite, positive and inside the target".into());
        }
        if self.x + self.width > target_width as f32 || self.y + self.height > target_height as f32 {
            return Err("viewport must be finite, positive and inside the target".into());
        }
        Ok(())
    }
}

impl Renderer {
    /// Uploads decoded PICA fragment-lighting state and lookup tables.
    pub fn create_lighting(&self, lighting: &LightingState) -> Result<GpuLighting, String> {
        let words = lighting_words(lighting)?;
        let bytes = words_bytes(&words);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PICA fragment lighting"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        buffer.get_mapped_range_mut(..).map_err(|e| e.to_string())?.copy_from_slice(&bytes);
        buffer.unmap();
        Ok(GpuLighting { buffer })
    }

    /// Uploads decoded, tightly packed RGBA8 texels for PICA texture unit 0.
    pub fn create_rgba8_texture(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
        sampler_state: SamplerState,
    ) -> Result<SampledTexture, String> {
        self.create_rgba8_texture_impl(width, height, pixels, sampler_state, None)
    }

    /// Uploads a texture with a PICA clamp-to-border color. Vulkan exposes
    /// transparent black, opaque black and opaque white directly; other
    /// colors remain explicit unsupported cases until manual sampling lands.
    pub fn create_rgba8_texture_with_border(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
        sampler_state: SamplerState,
        border: [u8; 4],
    ) -> Result<SampledTexture, String> {
        self.create_rgba8_texture_impl(width, height, pixels, sampler_state, Some(border))
    }

    fn create_rgba8_texture_impl(
        &self,
        width: u32,
        height: u32,
        pixels: &[u8],
        sampler_state: SamplerState,
        border: Option<[u8; 4]>,
    ) -> Result<SampledTexture, String> {
        validate_target(width, height)?;
        let expected = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or("texture dimensions are too large")?;
        if pixels.len() != expected {
            return Err(format!("RGBA8 texture needs {expected} bytes, got {}", pixels.len()));
        }
        let bytes_per_row = width.checked_mul(4).ok_or("texture row is too large")?;
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("decoded PICA texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) },
            extent,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = create_sampler(&self.device, sampler_state, border)?;
        Ok(SampledTexture { _texture: texture, view, sampler })
    }

    /// Creates and clears a persistent RGBA8 render target.
    pub fn create_color_target(&self, width: u32, height: u32) -> Result<ColorTarget, String> {
        validate_target(width, height)?;
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PICA color target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PICA depth target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_sample =
            depth.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::DepthOnly, ..Default::default() });
        let stencil_sample =
            depth.create_view(&wgpu::TextureViewDescriptor { aspect: wgpu::TextureAspect::StencilOnly, ..Default::default() });
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback_size = u64::from(padded_row).checked_mul(u64::from(height)).ok_or("render target is too large")?;
        let readback = |label| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: readback_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let color_readback = readback("PICA persistent color readback");
        let depth_readback = readback("PICA persistent depth readback");
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("clear PICA color target") });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear PICA color target"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let submitted = self.queue.submit([encoder.finish()]);
        self.wait(submitted)?;
        check_errors(errors, "create color target")?;
        Ok(ColorTarget {
            texture,
            view,
            depth,
            depth_view,
            depth_sample,
            stencil_sample,
            color_readback,
            depth_readback,
            width,
            height,
        })
    }

    /// Converts rendered D24 depth and stencil into PICA texture-format RGBA8
    /// entirely on the GPU. Output rows
    /// are flipped so direct texture coordinates retain PICA's bottom origin.
    /// Conversion is queued without a CPU wait; subsequent queue submissions
    /// observe the result in order, and readback supplies the completion wait.
    pub fn create_d24s8_feedback_texture(
        &self,
        target: &ColorTarget,
        sampler_state: SamplerState,
        border: [u8; 4],
    ) -> Result<SampledTexture, String> {
        let (width, height) = target.dimensions();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("PICA D24S8-as-RGBA8 feedback"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PICA depth feedback conversion"),
            layout: &self.raster_pipelines.depth_feedback_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&target.depth_sample) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&target.stencil_sample) },
            ],
        });
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA depth feedback conversion") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("PICA depth feedback conversion"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.raster_pipelines.depth_feedback_pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        check_errors(errors, "depth feedback conversion")?;
        let sampler = create_sampler(&self.device, sampler_state, Some(border))?;
        Ok(SampledTexture { _texture: texture, view, sampler })
    }

    /// Replaces a persistent target with a guest framebuffer snapshot.
    /// Color rows are tightly packed, top-to-bottom RGBA8. Depth values use
    /// Vulkan's normalized 0..1 convention (the negated PICA clip-space z/w).
    pub fn upload_target(&self, target: &ColorTarget, color: &[u8], depth: &[f32]) -> Result<(), String> {
        let (width, height) = target.dimensions();
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
            .ok_or("render target is too large")?;
        let color_bytes = pixels.checked_mul(4).ok_or("render target is too large")?;
        if color.len() != color_bytes {
            return Err(format!("color snapshot has {} bytes; expected {color_bytes}", color.len()));
        }
        if depth.len() != pixels {
            return Err(format!("depth snapshot has {} values; expected {pixels}", depth.len()));
        }
        if depth.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
            return Err("depth snapshot values must be finite and in 0..=1".into());
        }
        let depth_bytes: Vec<u8> = depth.iter().flat_map(|value| value.to_ne_bytes()).collect();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.queue.write_texture(
            target.texture.as_image_copy(),
            color,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            extent,
        );
        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("guest depth snapshot"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            depth_texture.as_image_copy(),
            &depth_bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            extent,
        );
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("guest depth snapshot"),
            layout: &self.raster_pipelines.depth_upload_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_view) }],
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upload guest depth snapshot") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("upload guest depth snapshot"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.raster_pipelines.depth_upload_pipeline);
            pass.set_bind_group(0, &depth_group, &[]);
            pass.draw(0..3, 0..1);
        }
        let submitted = self.queue.submit([encoder.finish()]);
        self.wait(submitted)?;
        check_errors(errors, "target upload")
    }

    /// Creates a persistent target initialized from a guest framebuffer.
    pub fn create_color_target_from_snapshot(
        &self,
        width: u32,
        height: u32,
        color: &[u8],
        depth: &[f32],
    ) -> Result<ColorTarget, String> {
        let target = self.create_color_target(width, height)?;
        self.upload_target(&target, color, depth)?;
        Ok(target)
    }

    /// Adds one triangle-list draw to an existing target without clearing
    /// earlier draws.
    pub fn draw_vertices(&self, target: &ColorTarget, vertices: &[RasterVertex]) -> Result<(), String> {
        self.draw_vertices_in(target, vertices, Viewport::full(target))
    }

    /// Adds one triangle-list draw in a PICA-derived viewport.
    pub fn draw_vertices_in(&self, target: &ColorTarget, vertices: &[RasterVertex], viewport: Viewport) -> Result<(), String> {
        self.draw_vertices_with(target, vertices, viewport, CullMode::None)
    }

    /// Adds one triangle-list draw with PICA viewport and culling state.
    pub fn draw_vertices_with(
        &self,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        cull: CullMode,
    ) -> Result<(), String> {
        self.draw_vertices_with_raster_state(target, vertices, viewport, RasterState { cull, ..RasterState::default() })
    }

    /// Adds one triangle-list draw with PICA viewport, culling and depth state.
    pub fn draw_vertices_with_state(
        &self,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        cull: CullMode,
        depth: DepthState,
    ) -> Result<(), String> {
        self.draw_vertices_with_raster_state(target, vertices, viewport, RasterState { cull, depth, ..RasterState::default() })
    }

    /// Adds one triangle-list draw with PICA viewport, culling, depth and
    /// per-channel color-write state.
    pub fn draw_vertices_with_color_mask(
        &self,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        cull: CullMode,
        depth: DepthState,
        color_mask: ColorMask,
    ) -> Result<(), String> {
        self.draw_vertices_with_raster_state(
            target,
            vertices,
            viewport,
            RasterState { cull, depth, color_mask, ..RasterState::default() },
        )
    }

    /// Adds one triangle-list draw with all currently supported PICA raster
    /// state, including optional fixed-function blending.
    pub fn draw_vertices_with_raster_state(
        &self,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        state: RasterState,
    ) -> Result<(), String> {
        self.draw_vertices_with_inputs(target, vertices, viewport, state, FragmentInputs::default())
    }

    /// Draws with fixed raster state and optional decoded fragment inputs.
    pub fn draw_vertices_with_inputs(
        &self,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        state: RasterState,
        inputs: FragmentInputs<'_>,
    ) -> Result<(), String> {
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA raster commands") });
        self.encode_vertices(&mut encoder, target, vertices, viewport, state, inputs)?;
        let submitted = self.queue.submit([encoder.finish()]);
        self.wait(submitted)?;
        check_errors(errors, "raster draw")
    }

    fn encode_vertices(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &ColorTarget,
        vertices: &[RasterVertex],
        viewport: Viewport,
        state: RasterState,
        inputs: FragmentInputs<'_>,
    ) -> Result<(), String> {
        self.encode_draw(encoder, target, VertexSource::Cpu(vertices), viewport, state, inputs)
    }

    fn encode_draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &ColorTarget,
        source: VertexSource<'_>,
        viewport: Viewport,
        state: RasterState,
        inputs: FragmentInputs<'_>,
    ) -> Result<(), String> {
        viewport.validate(target)?;
        let blend_constant = state.blend.map(BlendState::host_constant).transpose()?;
        let (vertex_buffer, index_buffer, vertex_group, count, program, vertex_mapping) = match source {
            VertexSource::Cpu(vertices) => {
                validate_vertices(vertices.len())?;
                let buffer =
                    self.upload_draw_buffer("PICA shaded vertices", &vertex_bytes(vertices), wgpu::BufferUsages::VERTEX)?;
                (Some(buffer), None, None, vertices.len() as u32, None, [0; 10])
            }
            VertexSource::Pica(draw) => {
                draw.validate()?;
                let bytes = pack::vertex_inputs(draw.inputs);
                if bytes.len() as u64 > self.device.limits().max_storage_buffer_binding_size {
                    return Err("PICA vertex input exceeds device storage limit".into());
                }
                let input = self.upload_draw_buffer("PICA vertex inputs", &bytes, wgpu::BufferUsages::STORAGE)?;
                let uniform =
                    self.upload_draw_buffer("PICA vertex uniforms", &words_bytes(draw.uniforms), wgpu::BufferUsages::STORAGE)?;
                let index =
                    self.upload_draw_buffer("PICA triangle indices", &words_bytes(draw.indices), wgpu::BufferUsages::INDEX)?;
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("PICA vertex inputs/uniforms"),
                    layout: &self.raster_pipelines.vertex_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
                    ],
                });
                (
                    None,
                    Some(index),
                    Some(group),
                    draw.indices.len() as u32,
                    Some(draw.program),
                    draw.uniforms[389..399].try_into().expect("output mapping words"),
                )
            }
        };
        let mut fragment_words = [0; 40];
        fragment_words[36] = state.depth_max;
        fragment_words[37] = state.depth_scale.to_bits();
        fragment_words[38] = state.depth_offset.to_bits();
        fragment_words[39] = u32::from(state.depth_w_buffer);
        fragment_words[..4].copy_from_slice(&state.alpha_test.map(AlphaTest::uniform).unwrap_or([0; 4]));
        fragment_words[3] = u32::from(inputs.texture0.is_some())
            | u32::from(inputs.texture1.is_some()) << 1
            | u32::from(inputs.texture2.is_some()) << 2;
        fragment_words[4] = state.tev.buffer_color;
        fragment_words[5] = state.tev.buffer_update;
        for (index, stage) in state.tev.stages.into_iter().enumerate() {
            let at = 6 + index * 5;
            fragment_words[at..at + 5].copy_from_slice(&[
                stage.sources,
                stage.operands,
                stage.combiner,
                stage.constant,
                stage.scale,
            ]);
        }
        let fragment_bytes = words_bytes(&fragment_words);
        let fragment_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("PICA fragment state"),
            size: fragment_bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: true,
        });
        fragment_buffer.get_mapped_range_mut(..).map_err(|e| e.to_string())?.copy_from_slice(&fragment_bytes);
        fragment_buffer.unmap();
        let (texture0_view, texture0_sampler) = match inputs.texture0 {
            Some(texture) => (&texture.view, &texture.sampler),
            None => (&self.raster_pipelines.dummy_view, &self.raster_pipelines.dummy_sampler),
        };
        let (texture1_view, texture1_sampler) = match inputs.texture1 {
            Some(texture) => (&texture.view, &texture.sampler),
            None => (&self.raster_pipelines.dummy_view, &self.raster_pipelines.dummy_sampler),
        };
        let (texture2_view, texture2_sampler) = match inputs.texture2 {
            Some(texture) => (&texture.view, &texture.sampler),
            None => (&self.raster_pipelines.dummy_view, &self.raster_pipelines.dummy_sampler),
        };
        let lighting_buffer = inputs.lighting.map_or(&self.raster_pipelines.dummy_lighting, |lighting| &lighting.buffer);
        let fragment_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("PICA fragment state"),
            layout: &self.raster_pipelines.fragment_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: fragment_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(texture0_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(texture0_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(texture1_view) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(texture1_sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(texture2_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(texture2_sampler) },
                wgpu::BindGroupEntry { binding: 7, resource: lighting_buffer.as_entire_binding() },
            ],
        });
        let key = PipelineKey {
            vertex_program: program.map_or(0, |program| program.id),
            vertex_mapping,
            cull: state.cull,
            depth: state.depth,
            stencil: state.stencil.key(),
            color_mask: state.color_mask,
            blend: state.blend.map(BlendState::key),
        };
        let mut pipelines = self.raster_pipelines.pipelines.lock().map_err(|_| "Vulkan pipeline cache lock was poisoned")?;
        let pipeline = pipelines.entry(key).or_insert_with(|| {
            if let Some(program) = program {
                let mut shaders = self.raster_pipelines.vertex_shaders.lock().expect("vertex shader cache lock");
                let shader = shaders.entry((program.id, vertex_mapping)).or_insert_with(|| {
                    self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("translated PICA vertex program"),
                        source: wgpu::ShaderSource::Wgsl(
                            format!(
                                "{RASTER_SHADER}\n{}\n{}\n{}",
                                program.source,
                                crate::vertex::BINDINGS,
                                crate::vertex::vertex_entry(&vertex_mapping)
                            )
                            .into(),
                        ),
                    })
                });
                create_pipeline(&self.device, shader, &self.raster_pipelines.vertex_pipeline_layout, key)
            } else {
                create_pipeline(&self.device, &self.raster_pipelines.shader, &self.raster_pipelines.pipeline_layout, key)
            }
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("PICA raster pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_stencil_reference(u32::from(state.stencil.reference));
            if let Some([r, g, b, a]) = blend_constant {
                pass.set_blend_constant(wgpu::Color { r: r.into(), g: g.into(), b: b.into(), a: a.into() });
            }
            pass.set_bind_group(0, &fragment_group, &[]);
            pass.set_viewport(viewport.x, viewport.y, viewport.width, viewport.height, 0.0, 1.0);
            if let (Some(indices), Some(group)) = (&index_buffer, &vertex_group) {
                pass.set_bind_group(1, group, &[]);
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..count, 0, 0..1);
            } else if let Some(vertices) = &vertex_buffer {
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.draw(0..count, 0..1);
            }
        }
        Ok(())
    }

    fn upload_draw_buffer(&self, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> Result<wgpu::Buffer, String> {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.len() as u64,
            usage,
            mapped_at_creation: true,
        });
        buffer.get_mapped_range_mut(..).map_err(|e| e.to_string())?.copy_from_slice(bytes);
        buffer.unmap();
        Ok(buffer)
    }

    /// Starts a command buffer which can collect many draws before one submit.
    pub fn begin_batch<'a>(&'a self, target: &'a ColorTarget) -> RasterBatch<'a> {
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA raster batch") });
        RasterBatch { renderer: self, target, encoder: Some(encoder), errors: Some(errors), draws: 0 }
    }

    /// Starts a draw batch by replacing its color and depth from a guest
    /// snapshot. The depth upload pass and subsequent draws share one command
    /// buffer and device wait.
    pub fn begin_batch_from_snapshot<'a>(
        &'a self,
        target: &'a ColorTarget,
        color: &[u8],
        depth: &[f32],
    ) -> Result<RasterBatch<'a>, String> {
        let (width, height) = target.dimensions();
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))
            .ok_or("render target is too large")?;
        let color_bytes = pixels.checked_mul(4).ok_or("render target is too large")?;
        if color.len() != color_bytes {
            return Err(format!("color snapshot has {} bytes; expected {color_bytes}", color.len()));
        }
        if depth.len() != pixels {
            return Err(format!("depth snapshot has {} values; expected {pixels}", depth.len()));
        }
        if depth.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
            return Err("depth snapshot values must be finite and in 0..=1".into());
        }
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        self.queue.write_texture(
            target.texture.as_image_copy(),
            color,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            extent,
        );
        let depth_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("batched guest depth snapshot"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let depth_bytes: Vec<u8> = depth.iter().flat_map(|value| value.to_ne_bytes()).collect();
        self.queue.write_texture(
            depth_texture.as_image_copy(),
            &depth_bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
            extent,
        );
        let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("batched guest depth snapshot"),
            layout: &self.raster_pipelines.depth_upload_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth_view) }],
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA snapshot raster batch") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("batched guest depth upload"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.raster_pipelines.depth_upload_pipeline);
            pass.set_bind_group(0, &depth_group, &[]);
            pass.draw(0..3, 0..1);
        }
        Ok(RasterBatch { renderer: self, target, encoder: Some(encoder), errors: Some(errors), draws: 0 })
    }

    /// Reads a target as tightly packed, top-to-bottom RGBA8 rows.
    pub fn read_color_target(&self, target: &ColorTarget) -> Result<Vec<u8>, String> {
        let (width, height) = target.dimensions();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = &target.color_readback;
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA readback commands") });
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(height) },
            },
            extent,
        );
        let submitted = self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
        self.wait(submitted)?;
        receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        check_errors(errors, "color readback")?;
        let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut image = vec![0; row_bytes as usize * height as usize];
        for row in 0..height as usize {
            let source = &mapped[row * padded_row as usize..][..row_bytes as usize];
            image[row * row_bytes as usize..][..row_bytes as usize].copy_from_slice(source);
        }
        drop(mapped);
        readback.unmap();
        Ok(image)
    }

    /// Reads normalized depth as tightly packed, top-to-bottom f32 values.
    /// This is kept separate from color readback so callers can validate the
    /// depth path before allowing Vulkan to replace guest depth memory.
    pub fn read_depth_target(&self, target: &ColorTarget) -> Result<Vec<f32>, String> {
        let (width, height) = target.dimensions();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = &target.depth_readback;
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA depth readback commands") });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(height) },
            },
            extent,
        );
        let submitted = self.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
        self.wait(submitted)?;
        receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        check_errors(errors, "depth readback")?;
        let mapped = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut image = Vec::with_capacity(width as usize * height as usize);
        for row in 0..height as usize {
            let source = &mapped[row * padded_row as usize..][..row_bytes as usize];
            image.extend(source.as_chunks::<4>().0.iter().map(|bytes| f32::from_ne_bytes(*bytes)));
        }
        drop(mapped);
        readback.unmap();
        Ok(image)
    }

    /// Reads color and depth with one copy submission and one device wait.
    pub fn read_color_and_depth_target(&self, target: &ColorTarget) -> Result<(Vec<u8>, Vec<f32>), String> {
        let (width, height) = target.dimensions();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let color_readback = &target.color_readback;
        let depth_readback = &target.depth_readback;
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("PICA combined readback commands") });
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(height) };
        encoder.copy_texture_to_buffer(
            target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: color_readback, layout },
            extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo { buffer: depth_readback, layout },
            extent,
        );
        let submitted = self.queue.submit([encoder.finish()]);
        let color_slice = color_readback.slice(..);
        let depth_slice = depth_readback.slice(..);
        let (color_send, color_receive) = std::sync::mpsc::sync_channel(1);
        let (depth_send, depth_receive) = std::sync::mpsc::sync_channel(1);
        color_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = color_send.send(result);
        });
        depth_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = depth_send.send(result);
        });
        self.wait(submitted)?;
        color_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        depth_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        check_errors(errors, "combined color/depth readback")?;
        let color_mapped = color_slice.get_mapped_range().map_err(|e| e.to_string())?;
        let depth_mapped = depth_slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut color = vec![0; row_bytes as usize * height as usize];
        let mut depth = Vec::with_capacity(width as usize * height as usize);
        for row in 0..height as usize {
            let range = row * padded_row as usize..row * padded_row as usize + row_bytes as usize;
            color[row * row_bytes as usize..][..row_bytes as usize].copy_from_slice(&color_mapped[range.clone()]);
            depth.extend(depth_mapped[range].as_chunks::<4>().0.iter().map(|bytes| f32::from_ne_bytes(*bytes)));
        }
        drop(color_mapped);
        drop(depth_mapped);
        color_readback.unmap();
        depth_readback.unmap();
        Ok((color, depth))
    }

    /// Convenience for one isolated draw used by bring-up tests.
    pub fn render_vertices(&self, vertices: &[RasterVertex], width: u32, height: u32) -> Result<Vec<u8>, String> {
        validate_vertices(vertices.len())?;
        let target = self.create_color_target(width, height)?;
        self.draw_vertices(&target, vertices)?;
        self.read_color_target(&target)
    }

    fn wait(&self, submission: wgpu::SubmissionIndex) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: None })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl RasterBatch<'_> {
    pub fn dimensions(&self) -> (u32, u32) {
        self.target.dimensions()
    }

    pub fn draw_pica_vertices(
        &mut self,
        vertices: crate::PicaVertexDraw<'_>,
        viewport: Viewport,
        state: RasterState,
        inputs: FragmentInputs<'_>,
    ) -> Result<(), String> {
        let encoder = self.encoder.as_mut().ok_or("Vulkan raster batch was already submitted")?;
        self.renderer.encode_draw(encoder, self.target, VertexSource::Pica(vertices), viewport, state, inputs)?;
        self.draws = self.draws.checked_add(1).ok_or("too many draws in one Vulkan batch")?;
        Ok(())
    }
    pub fn draw(&mut self, vertices: &[RasterVertex], viewport: Viewport, state: RasterState) -> Result<(), String> {
        self.draw_with_inputs(vertices, viewport, state, FragmentInputs::default())
    }

    pub fn draw_with_inputs(
        &mut self,
        vertices: &[RasterVertex],
        viewport: Viewport,
        state: RasterState,
        inputs: FragmentInputs<'_>,
    ) -> Result<(), String> {
        let encoder = self.encoder.as_mut().ok_or("Vulkan raster batch was already submitted")?;
        self.renderer.encode_vertices(encoder, self.target, vertices, viewport, state, inputs)?;
        self.draws = self.draws.checked_add(1).ok_or("too many draws in one Vulkan batch")?;
        Ok(())
    }

    /// Queues draws without waiting for GPU completion. Submissions on this
    /// renderer's queue remain ordered; the caller must wait through a later
    /// readback before exposing results to the guest. Validation is checked
    /// here so a failed list can still take the software fallback.
    pub fn submit(mut self) -> Result<u32, String> {
        let encoder = self.encoder.take().ok_or("Vulkan raster batch was already submitted")?;
        let errors = self.errors.take().ok_or("Vulkan raster batch has no validation scope")?;
        self.renderer.queue.submit([encoder.finish()]);
        check_errors(errors, "raster batch submission")?;
        Ok(self.draws)
    }

    /// Submits every recorded draw and waits for completion. Returns the draw
    /// count so callers can account for command-list batching.
    pub fn finish(mut self) -> Result<u32, String> {
        let encoder = self.encoder.take().ok_or("Vulkan raster batch was already submitted")?;
        let errors = self.errors.take().ok_or("Vulkan raster batch has no validation scope")?;
        let submitted = self.renderer.queue.submit([encoder.finish()]);
        self.renderer.wait(submitted)?;
        check_errors(errors, "raster batch")?;
        Ok(self.draws)
    }

    /// Submits the recorded draws and copies their color and depth results in
    /// the same command buffer, requiring only one device wait.
    pub fn finish_and_read_color_and_depth(mut self) -> Result<(Vec<u8>, Vec<f32>), String> {
        let mut encoder = self.encoder.take().ok_or("Vulkan raster batch was already submitted")?;
        let errors = self.errors.take().ok_or("Vulkan raster batch has no validation scope")?;
        let (width, height) = self.target.dimensions();
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let row_bytes = width.checked_mul(4).ok_or("render target row is too large")?;
        let padded_row = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let color_readback = &self.target.color_readback;
        let depth_readback = &self.target.depth_readback;
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(height) };
        encoder.copy_texture_to_buffer(
            self.target.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: color_readback, layout },
            extent,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target.depth,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo { buffer: depth_readback, layout },
            extent,
        );

        let submitted = self.renderer.queue.submit([encoder.finish()]);
        let color_slice = color_readback.slice(..);
        let depth_slice = depth_readback.slice(..);
        let (color_send, color_receive) = std::sync::mpsc::sync_channel(1);
        let (depth_send, depth_receive) = std::sync::mpsc::sync_channel(1);
        color_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = color_send.send(result);
        });
        depth_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = depth_send.send(result);
        });
        self.renderer.wait(submitted)?;
        color_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        depth_receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        check_errors(errors, "raster batch and readback")?;

        let color_mapped = color_slice.get_mapped_range().map_err(|e| e.to_string())?;
        let depth_mapped = depth_slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut color = vec![0; row_bytes as usize * height as usize];
        let mut depth = Vec::with_capacity(width as usize * height as usize);
        for row in 0..height as usize {
            let range = row * padded_row as usize..row * padded_row as usize + row_bytes as usize;
            color[row * row_bytes as usize..][..row_bytes as usize].copy_from_slice(&color_mapped[range.clone()]);
            depth.extend(depth_mapped[range].as_chunks::<4>().0.iter().map(|bytes| f32::from_ne_bytes(*bytes)));
        }
        drop(color_mapped);
        drop(depth_mapped);
        color_readback.unmap();
        depth_readback.unmap();
        Ok((color, depth))
    }
}

fn check_errors(scope: wgpu::ErrorScopeGuard, operation: &str) -> Result<(), String> {
    match pollster::block_on(scope.pop()) {
        Some(error) => Err(format!("Vulkan {operation} failed: {error}")),
        None => Ok(()),
    }
}

pub(super) fn create_pipelines(device: &wgpu::Device, queue: &wgpu::Queue) -> RasterPipelines {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("PICA shaded-vertex raster shader"),
        source: wgpu::ShaderSource::Wgsl(RASTER_SHADER.into()),
    });
    let fragment_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("PICA fragment-state layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 7,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("PICA raster pipeline layout"),
        bind_group_layouts: &[Some(&fragment_layout)],
        immediate_size: 0,
    });
    let depth_upload_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("guest depth snapshot shader"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_UPLOAD_SHADER.into()),
    });
    let depth_upload_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("guest depth snapshot layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let depth_upload_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("guest depth snapshot pipeline layout"),
        bind_group_layouts: &[Some(&depth_upload_layout)],
        immediate_size: 0,
    });
    let depth_upload_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("guest depth snapshot pipeline"),
        layout: Some(&depth_upload_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &depth_upload_shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &depth_upload_shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        multiview_mask: None,
        cache: None,
    });
    let depth_feedback_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("PICA D24S8 feedback conversion shader"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_FEEDBACK_SHADER.into()),
    });
    let depth_feedback_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("PICA D24S8 feedback conversion layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Uint,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let depth_feedback_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("PICA D24S8 feedback conversion pipeline layout"),
        bind_group_layouts: &[Some(&depth_feedback_layout)],
        immediate_size: 0,
    });
    let depth_feedback_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("PICA D24S8 feedback conversion pipeline"),
        layout: Some(&depth_feedback_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &depth_feedback_shader,
            entry_point: Some("vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &depth_feedback_shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let dummy_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("PICA dummy white texture"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        dummy_texture.as_image_copy(),
        &[255; 4],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    let dummy_view = dummy_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let dummy_sampler = create_sampler(device, SamplerState::default(), None).expect("default sampler is supported");
    let dummy_lighting = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("disabled PICA fragment lighting"),
        size: ((LUT_BASE + LUT_COUNT * LUT_SIZE) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: true,
    });
    dummy_lighting.get_mapped_range_mut(..).expect("new lighting buffer is mapped").copy_from_slice(&vec![
        0;
        (LUT_BASE
            + LUT_COUNT
                * LUT_SIZE)
            * 4
    ]);
    dummy_lighting.unmap();
    let vertex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("PICA raw vertex layout"),
        entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }),
    });
    let vertex_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("PICA hardware vertex pipeline layout"),
        bind_group_layouts: &[Some(&fragment_layout), Some(&vertex_layout)],
        immediate_size: 0,
    });
    RasterPipelines {
        shader,
        fragment_layout,
        pipeline_layout,
        vertex_layout,
        vertex_pipeline_layout,
        vertex_shaders: Mutex::new(HashMap::new()),
        depth_upload_layout,
        depth_upload_pipeline,
        depth_feedback_layout,
        depth_feedback_pipeline,
        color_feedback: color::ColorFeedback::new(device),
        resolved_readback: readback::ResolvePipeline::new(device),
        scaled_seed: seed::SeedPipeline::new(device),
        display_copy: display::DisplayPipeline::new(device),
        _dummy_texture: dummy_texture,
        dummy_view,
        dummy_sampler,
        dummy_lighting,
        pipelines: Mutex::new(HashMap::new()),
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    key: PipelineKey,
) -> wgpu::RenderPipeline {
    let layout = wgpu::VertexBufferLayout {
        array_stride: 84,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 32, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 40, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 48, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 56, shader_location: 5 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 72, shader_location: 6 },
        ],
    };
    let cpu_buffers = [Some(layout)];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("PICA shaded-vertex raster pipeline"),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(if key.vertex_program == 0 { "vertex" } else { "pica_vertex" }),
            compilation_options: Default::default(),
            buffers: if key.vertex_program == 0 { &cpu_buffers } else { &[] },
        },
        primitive: wgpu::PrimitiveState { cull_mode: key.cull.wgpu(), ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(key.depth.write),
            depth_compare: Some(key.depth.compare.wgpu()),
            stencil: key.stencil.wgpu(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: key.blend.map(|blend| wgpu::BlendState { color: blend.color.wgpu(), alpha: blend.alpha.wgpu() }),
                write_mask: key.color_mask.wgpu(),
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn validate_vertices(vertices: usize) -> Result<(), String> {
    if vertices == 0 || !vertices.is_multiple_of(3) {
        return Err("a non-empty triangle list needs a multiple of three vertices".into());
    }
    Ok(())
}

fn validate_target(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("render target dimensions must be nonzero".into());
    }
    Ok(())
}

fn create_sampler(device: &wgpu::Device, state: SamplerState, border: Option<[u8; 4]>) -> Result<wgpu::Sampler, String> {
    let filter = match state.filter {
        FilterMode::Nearest => wgpu::FilterMode::Nearest,
        FilterMode::Linear => wgpu::FilterMode::Linear,
    };
    let mipmap_filter = match state.filter {
        FilterMode::Nearest => wgpu::MipmapFilterMode::Nearest,
        FilterMode::Linear => wgpu::MipmapFilterMode::Linear,
    };
    let uses_border = state.wrap_u == WrapMode::ClampToBorder || state.wrap_v == WrapMode::ClampToBorder;
    let border_color = if uses_border {
        if !device.features().contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER) {
            return Err("Vulkan adapter does not support clamp-to-border samplers".into());
        }
        Some(match border {
            Some([0, 0, 0, 0]) => wgpu::SamplerBorderColor::TransparentBlack,
            Some([0, 0, 0, 255]) => wgpu::SamplerBorderColor::OpaqueBlack,
            Some([255, 255, 255, 255]) => wgpu::SamplerBorderColor::OpaqueWhite,
            _ => return Err("arbitrary PICA texture border colors need manual shader sampling".into()),
        })
    } else {
        None
    };
    Ok(device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("PICA texture sampler"),
        address_mode_u: state.wrap_u.wgpu(border_color.is_some())?,
        address_mode_v: state.wrap_v.wgpu(border_color.is_some())?,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter,
        border_color,
        ..Default::default()
    }))
}

fn vertex_bytes(vertices: &[RasterVertex]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vertices.len() * 84);
    for vertex in vertices {
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
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
    }
    bytes
}

fn words_bytes(words: &[u32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    bytes
}

const LIGHT_HEADER_WORDS: usize = 12;
const LIGHT_WORDS: usize = 24;
const MAX_LIGHTS: usize = 8;
const LUT_COUNT: usize = 24;
const LUT_SIZE: usize = 256;
const LUT_BASE: usize = LIGHT_HEADER_WORDS + MAX_LIGHTS * LIGHT_WORDS;

fn lighting_f16(bits: u32) -> f32 {
    let (sign, exponent, mantissa) = ((bits >> 15) & 1, (bits >> 10) & 0x1F, bits & 0x3FF);
    let magnitude = match exponent {
        0 => mantissa as f32 / 1024.0 * 2f32.powi(-14),
        0x1F => f32::INFINITY,
        value => (1.0 + mantissa as f32 / 1024.0) * 2f32.powi(value as i32 - 15),
    };
    if sign == 1 {
        -magnitude
    } else {
        magnitude
    }
}

/// PICA float1.7.16 (bias 63) as f32.
fn pica_f24(bits: u32) -> f32 {
    let bits = bits & 0xFF_FFFF;
    let sign = bits >> 23;
    let exponent = bits >> 16 & 0x7F;
    let mantissa = bits & 0xFFFF;
    let out = match exponent {
        0 => sign << 31,
        0x7F => sign << 31 | 0xFF << 23 | mantissa << 7,
        value => sign << 31 | (value + 127 - 63) << 23 | mantissa << 7,
    };
    f32::from_bits(out)
}

fn lighting_f20(bits: u32) -> f32 {
    let (sign, exponent, mantissa) = ((bits >> 19) & 1, (bits >> 12) & 0x7F, bits & 0xFFF);
    let magnitude = if exponent == 0 { 0.0 } else { (1.0 + mantissa as f32 / 4096.0) * 2f32.powi(exponent as i32 - 63) };
    if sign == 1 {
        -magnitude
    } else {
        magnitude
    }
}

fn lighting_fixed13(bits: u32) -> f32 {
    (((bits & 0x1FFF) << 19) as i32 >> 19) as f32 / 2047.0
}

fn lighting_color(bits: u32) -> [f32; 3] {
    [((bits >> 20) & 0xFF) as f32 / 255.0, ((bits >> 10) & 0xFF) as f32 / 255.0, (bits & 0xFF) as f32 / 255.0]
}

fn lighting_words(lighting: &LightingState) -> Result<Vec<u32>, String> {
    if lighting.lights.len() > MAX_LIGHTS {
        return Err("PICA supports at most eight fragment lights".into());
    }
    if lighting.luts.len() > LUT_COUNT {
        return Err("PICA supports exactly 24 fragment-lighting lookup tables".into());
    }
    let mut words = vec![0u32; LUT_BASE + LUT_COUNT * LUT_SIZE];
    words[0] = 1;
    words[1] = lighting.lights.len() as u32;
    words[2] = lighting.config0;
    words[3] = lighting.config1;
    words[4] = lighting.abs;
    words[5] = lighting.select;
    words[6] = lighting.scale;
    for (slot, value) in words[8..11].iter_mut().zip(lighting.ambient) {
        *slot = value.to_bits();
    }
    for (index, light) in lighting.lights.iter().enumerate() {
        let at = LIGHT_HEADER_WORDS + index * LIGHT_WORDS;
        words[at] = light.id;
        words[at + 1] = u32::from(light.directional)
            | u32::from(light.two_sided) << 1
            | u32::from(light.geometric[0]) << 2
            | u32::from(light.geometric[1]) << 3;
        let fields = [light.position, light.spot_direction, light.diffuse, light.ambient, light.specular0, light.specular1];
        for (field, values) in fields.into_iter().enumerate() {
            for (component, value) in values.into_iter().enumerate() {
                words[at + 2 + field * 3 + component] = value.to_bits();
            }
        }
        words[at + 20] = light.attenuation_bias.to_bits();
        words[at + 21] = light.attenuation_scale.to_bits();
    }
    for (table, entries) in lighting.luts.iter().enumerate() {
        let at = LUT_BASE + table * LUT_SIZE;
        words[at..at + LUT_SIZE].copy_from_slice(entries);
    }
    Ok(words)
}

const DEPTH_UPLOAD_SHADER: &str = r#"
@group(0) @binding(0) var guest_depth: texture_2d<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(guest_depth, vec2<i32>(position.xy), 0).r;
}
"#;

const DEPTH_FEEDBACK_SHADER: &str = r#"
@group(0) @binding(0) var rendered_depth: texture_depth_2d;
@group(0) @binding(1) var guest_stencil: texture_2d<u32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(rendered_depth);
    let x = u32(position.x);
    let source_y = dimensions.y - 1u - u32(position.y);
    let at = vec2<i32>(i32(x), i32(source_y));
    let depth = textureLoad(rendered_depth, at, 0);
    let stencil = textureLoad(guest_stencil, at, 0).r;
    let value = u32(clamp(depth, 0.0, 1.0) * 16777215.0);
    return vec4<f32>(
        f32(stencil) / 255.0,
        f32((value >> 16u) & 255u) / 255.0,
        f32((value >> 8u) & 255u) / 255.0,
        f32(value & 255u) / 255.0,
    );
}
"#;

const RASTER_SHADER: &str = r#"
struct FragmentState {
    words: array<u32>,
}

@group(0) @binding(0) var<storage, read> state: FragmentState;
@group(0) @binding(1) var texture0: texture_2d<f32>;
@group(0) @binding(2) var texture0_sampler: sampler;
@group(0) @binding(3) var texture1: texture_2d<f32>;
@group(0) @binding(4) var texture1_sampler: sampler;
@group(0) @binding(5) var texture2: texture_2d<f32>;
@group(0) @binding(6) var texture2_sampler: sampler;
@group(0) @binding(7) var<storage, read> lighting_state: FragmentState;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) texcoord0: vec2<f32>,
    @location(2) texcoord1: vec2<f32>,
    @location(3) texcoord2: vec2<f32>,
    @location(4) quaternion: vec4<f32>,
    @location(5) view: vec3<f32>,
}

@vertex
fn vertex(
    @location(0) position: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) texcoord0: vec2<f32>,
    @location(3) texcoord1: vec2<f32>,
    @location(4) texcoord2: vec2<f32>,
    @location(5) quaternion: vec4<f32>,
    @location(6) view: vec3<f32>,
) -> VertexOut {
    var out: VertexOut;
    // PICA clip depth is -w..0 and the stored depth is -z/w; WebGPU clip
    // depth is 0..w, so negating clip z preserves the PICA depth value.
    out.position = vec4<f32>(position.xy, -position.z, position.w);
    out.color = color;
    out.texcoord0 = texcoord0;
    out.texcoord1 = texcoord1;
    out.texcoord2 = texcoord2;
    out.quaternion = quaternion;
    out.view = view;
    return out;
}

fn compare(value: u32, reference: u32, op: u32) -> bool {
    if op == 0u { return false; }
    if op == 1u { return true; }
    if op == 2u { return value == reference; }
    if op == 3u { return value != reference; }
    if op == 4u { return value < reference; }
    if op == 5u { return value <= reference; }
    if op == 6u { return value > reference; }
    return value >= reference;
}

fn unpack_color(value: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(value & 255u),
        f32((value >> 8u) & 255u),
        f32((value >> 16u) & 255u),
        f32((value >> 24u) & 255u)
    ) / 255.0;
}

fn tev_source(
    source: u32,
    primary: vec4<f32>,
    lit0: vec4<f32>,
    lit1: vec4<f32>,
    sampled0: vec4<f32>,
    sampled1: vec4<f32>,
    sampled2: vec4<f32>,
    buffer: vec4<f32>,
    constant: vec4<f32>,
    previous: vec4<f32>,
) -> vec4<f32> {
    if source == 0u { return primary; }
    if source == 1u { return lit0; }
    if source == 2u { return lit1; }
    if source == 3u { return sampled0; }
    if source == 4u { return sampled1; }
    if source == 5u { return sampled2; }
    if source == 6u { return vec4<f32>(1.0); }
    if source == 13u { return buffer; }
    if source == 14u { return constant; }
    return previous;
}

fn rgb_operand(value: vec4<f32>, operand: u32) -> vec3<f32> {
    if operand == 1u { return vec3<f32>(1.0) - value.rgb; }
    if operand == 2u { return vec3<f32>(value.a); }
    if operand == 3u { return vec3<f32>(1.0 - value.a); }
    if operand == 4u { return vec3<f32>(value.r); }
    if operand == 5u { return vec3<f32>(1.0 - value.r); }
    if operand == 8u { return vec3<f32>(value.g); }
    if operand == 9u { return vec3<f32>(1.0 - value.g); }
    if operand == 12u { return vec3<f32>(value.b); }
    if operand == 13u { return vec3<f32>(1.0 - value.b); }
    return value.rgb;
}

fn alpha_operand(value: vec4<f32>, operand: u32) -> f32 {
    var component = value.a;
    if ((operand >> 1u) & 3u) == 1u { component = value.r; }
    if ((operand >> 1u) & 3u) == 2u { component = value.g; }
    if ((operand >> 1u) & 3u) == 3u { component = value.b; }
    if (operand & 1u) != 0u { return 1.0 - component; }
    return component;
}

fn combine(mode: u32, a: f32, b: f32, c: f32) -> f32 {
    if mode == 1u { return a * b; }
    if mode == 2u { return a + b; }
    if mode == 3u { return a + b - 0.5; }
    if mode == 4u { return a * c + b * (1.0 - c); }
    if mode == 5u { return a - b; }
    if mode == 8u { return a * b + c; }
    if mode == 9u { return min(a + b, 1.0) * c; }
    return a;
}

fn scale_factor(bits: u32) -> f32 {
    if (bits & 3u) == 1u { return 2.0; }
    if (bits & 3u) == 2u { return 4.0; }
    return 1.0;
}

fn tev(
    primary: vec4<f32>,
    lit0: vec4<f32>,
    lit1: vec4<f32>,
    sampled0: vec4<f32>,
    sampled1: vec4<f32>,
    sampled2: vec4<f32>,
) -> vec4<f32> {
    var previous = vec4<f32>(0.0);
    var buffer = unpack_color(state.words[4]);
    let update = state.words[5];
    for (var stage = 0u; stage < 6u; stage += 1u) {
        let at = 6u + stage * 5u;
        let sources = state.words[at];
        let operands = state.words[at + 1u];
        let modes = state.words[at + 2u];
        let constant = unpack_color(state.words[at + 3u]);
        let scales = state.words[at + 4u];
        var rgb_args: array<vec3<f32>, 3>;
        var alpha_args: array<f32, 3>;
        for (var argument = 0u; argument < 3u; argument += 1u) {
            let rgb_source = (sources >> (4u * argument)) & 15u;
            let alpha_source = (sources >> (16u + 4u * argument)) & 15u;
            rgb_args[argument] = rgb_operand(
                tev_source(rgb_source, primary, lit0, lit1, sampled0, sampled1, sampled2, buffer, constant, previous),
                (operands >> (4u * argument)) & 15u
            );
            alpha_args[argument] = alpha_operand(
                tev_source(alpha_source, primary, lit0, lit1, sampled0, sampled1, sampled2, buffer, constant, previous),
                (operands >> (12u + 4u * argument)) & 7u
            );
        }
        let rgb_mode = modes & 15u;
        let alpha_mode = (modes >> 16u) & 15u;
        var rgb = vec3<f32>(0.0);
        var alpha = 0.0;
        if rgb_mode == 6u || rgb_mode == 7u {
            let dot = 4.0 * dot(rgb_args[0] - vec3<f32>(0.5), rgb_args[1] - vec3<f32>(0.5));
            rgb = vec3<f32>(dot);
            if rgb_mode == 7u { alpha = dot; }
            else { alpha = combine(alpha_mode, alpha_args[0], alpha_args[1], alpha_args[2]); }
        } else {
            for (var channel = 0u; channel < 3u; channel += 1u) {
                rgb[channel] = combine(rgb_mode, rgb_args[0][channel], rgb_args[1][channel], rgb_args[2][channel]);
            }
            alpha = combine(alpha_mode, alpha_args[0], alpha_args[1], alpha_args[2]);
        }
        // Latch the prior stage only after this stage has read the buffer.
        if stage >= 1u && stage <= 4u {
            if ((update >> (7u + stage)) & 1u) != 0u { buffer = vec4<f32>(previous.rgb, buffer.a); }
            if ((update >> (11u + stage)) & 1u) != 0u { buffer.a = previous.a; }
        }
        previous = vec4<f32>(
            clamp(rgb * scale_factor(scales), vec3<f32>(0.0), vec3<f32>(1.0)),
            clamp(alpha * scale_factor(scales >> 16u), 0.0, 1.0)
        );
    }
    return previous;
}

struct LightingResult {
    primary: vec4<f32>,
    secondary: vec4<f32>,
}

fn lighting_vec3(at: u32) -> vec3<f32> {
    return vec3<f32>(
        bitcast<f32>(lighting_state.words[at]),
        bitcast<f32>(lighting_state.words[at + 1u]),
        bitcast<f32>(lighting_state.words[at + 2u])
    );
}

fn safe_normalize(value: vec3<f32>) -> vec3<f32> {
    let length_squared = dot(value, value);
    if length_squared == 0.0 { return value; }
    return value * inverseSqrt(length_squared);
}

const LIGHT_LUT_BASE: u32 = 204u;

fn lighting_environment_supports(table: u32) -> bool {
    let environment = (lighting_state.words[2] >> 4u) & 15u;
    if table == 0u { return environment != 1u; }
    if table == 1u { return environment != 0u && environment != 1u && environment != 5u; }
    if table == 3u { return environment != 0u && environment != 2u && environment != 4u; }
    if table == 6u { return environment != 3u; }
    if table == 4u || table == 5u { return environment == 4u || environment == 5u || environment == 8u; }
    if table == 8u { return environment != 2u && environment != 3u; }
    return true;
}

fn lighting_lut_entry(table: u32, index: u32, delta: f32) -> f32 {
    let raw = lighting_state.words[LIGHT_LUT_BASE + table * 256u + (index & 255u)];
    let value = f32(raw & 4095u) / 4095.0;
    let magnitude=f32((raw>>12u)&2047u)/2047.0;
    let difference=select(magnitude,-magnitude,(raw&8388608u)!=0u);
    return value + difference * delta;
}

fn lighting_lut_lookup(table: u32, shift: u32, input: f32) -> f32 {
    var index = 0u;
    var delta = 0.0;
    if ((lighting_state.words[4] >> (shift + 1u)) & 1u) == 0u {
        let scaled = abs(input) * 256.0;
        index = u32(clamp(scaled, 0.0, 255.0));
        delta = scaled - f32(index);
    } else {
        let scaled = input * 128.0;
        let signed_index = i32(clamp(floor(scaled), -128.0, 127.0));
        index = bitcast<u32>(signed_index) & 255u;
        delta = scaled - f32(signed_index);
    }
    let scale_bits = (lighting_state.words[6] >> shift) & 7u;
    var scale = 1.0;
    if scale_bits == 1u { scale = 2.0; }
    if scale_bits == 2u { scale = 4.0; }
    if scale_bits == 3u { scale = 8.0; }
    if scale_bits == 6u { scale = 0.25; }
    if scale_bits == 7u { scale = 0.5; }
    return scale * lighting_lut_entry(table, index, delta);
}

fn lighting_lut_enabled(table: u32, disable_bit: u32) -> bool {
    return ((lighting_state.words[3] >> disable_bit) & 1u) == 0u && lighting_environment_supports(table);
}

fn lighting_lut_input(
    shift: u32,
    normal: vec3<f32>,
    view: vec3<f32>,
    half_vector: vec3<f32>,
    light: vec3<f32>,
    spot_direction: vec3<f32>,
) -> f32 {
    let selector = (lighting_state.words[5] >> shift) & 7u;
    if selector == 0u { return dot(normal, safe_normalize(half_vector)); }
    if selector == 1u { return dot(view, safe_normalize(half_vector)); }
    if selector == 2u { return dot(normal, view); }
    if selector == 3u { return dot(light, normal); }
    if selector == 4u { return dot(light, spot_direction); }
    return 0.0;
}

fn shade_lighting(quaternion_input: vec4<f32>, view_input: vec3<f32>) -> LightingResult {
    var result: LightingResult;
    result.primary = vec4<f32>(0.0);
    result.secondary = vec4<f32>(0.0);
    if lighting_state.words[0] == 0u { return result; }

    var quaternion = quaternion_input;
    let quaternion_length = dot(quaternion, quaternion);
    if quaternion_length == 0.0 { quaternion = vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    else { quaternion *= inverseSqrt(quaternion_length); }
    let up = vec3<f32>(0.0, 0.0, 1.0);
    let inner = cross(quaternion.xyz, up) + up * quaternion.w;
    let normal = up + 2.0 * cross(quaternion.xyz, inner);
    let view = safe_normalize(view_input);
    var diffuse = vec3<f32>(0.0);
    var specular = vec3<f32>(0.0);
    var diffuse_alpha = 1.0;
    var specular_alpha = 1.0;
    let count = min(lighting_state.words[1], 8u);
    for (var index = 0u; index < count; index += 1u) {
        let at = 12u + index * 24u;
        let flags = lighting_state.words[at + 1u];
        let position = lighting_vec3(at + 2u);
        let spot_direction = lighting_vec3(at + 5u);
        let to_light = select(position + view_input, position, (flags & 1u) != 0u);
        let light = safe_normalize(to_light);
        let raw_dot = dot(light, normal);
        var n_dot_l = max(raw_dot, 0.0);
        if (flags & 2u) != 0u { n_dot_l = abs(raw_dot); }
        let half_vector = view + light;
        let half_length = dot(half_vector, half_vector);
        var geometric = 0.0;
        if half_length != 0.0 { geometric = min(n_dot_l / half_length, 1.0); }
        let geometric0 = select(1.0, geometric, (flags & 4u) != 0u);
        let geometric1 = select(1.0, geometric, (flags & 8u) != 0u);
        let clamp_specular = select(1.0, 0.0, (lighting_state.words[2] & (1u << 27u)) != 0u && raw_dot <= 0.0);
        let light_diffuse = lighting_vec3(at + 8u);
        let light_ambient = lighting_vec3(at + 11u);
        let specular0 = lighting_vec3(at + 14u);
        let specular1 = lighting_vec3(at + 17u);
        let id = lighting_state.words[at];
        var distance = 1.0;
        if ((lighting_state.words[3] >> (24u + id)) & 1u) == 0u {
            let attenuation_bias = bitcast<f32>(lighting_state.words[at + 20u]);
            let attenuation_scale = bitcast<f32>(lighting_state.words[at + 21u]);
            let x = clamp(attenuation_scale * length(to_light) + attenuation_bias, 0.0, 1.0);
            let lut_index = u32(clamp(x * 256.0, 0.0, 255.0));
            distance = lighting_lut_entry(16u + id, lut_index, x * 256.0 - f32(lut_index));
        }
        var spot = 1.0;
        if ((lighting_state.words[3] >> (8u + id)) & 1u) == 0u && lighting_environment_supports(8u) {
            spot = lighting_lut_lookup(8u + id, 8u, lighting_lut_input(8u, normal, view, half_vector, light, spot_direction));
        }
        let input_d0 = lighting_lut_input(0u, normal, view, half_vector, light, spot_direction);
        let input_d1 = lighting_lut_input(4u, normal, view, half_vector, light, spot_direction);
        let d0 = select(1.0, lighting_lut_lookup(0u, 0u, input_d0), lighting_lut_enabled(0u, 16u));
        let d1 = select(1.0, lighting_lut_lookup(1u, 4u, input_d1), lighting_lut_enabled(1u, 17u));
        let input_rr = lighting_lut_input(24u, normal, view, half_vector, light, spot_direction);
        let red = select(1.0, lighting_lut_lookup(6u, 24u, input_rr), lighting_lut_enabled(6u, 22u));
        let input_rg = lighting_lut_input(20u, normal, view, half_vector, light, spot_direction);
        let green = select(red, lighting_lut_lookup(5u, 20u, input_rg), lighting_lut_enabled(5u, 21u));
        let input_rb = lighting_lut_input(16u, normal, view, half_vector, light, spot_direction);
        let blue = select(red, lighting_lut_lookup(4u, 16u, input_rb), lighting_lut_enabled(4u, 20u));
        let attenuation = distance * spot;
        diffuse += (light_diffuse * n_dot_l + light_ambient) * attenuation;
        specular += (d0 * specular0 * geometric0 + d1 * vec3<f32>(red, green, blue) * specular1 * geometric1)
            * clamp_specular * attenuation;
        if lighting_lut_enabled(3u, 19u) {
            let fresnel = lighting_lut_lookup(
                3u,
                12u,
                lighting_lut_input(12u, normal, view, half_vector, light, spot_direction)
            );
            let selector = (lighting_state.words[2] >> 2u) & 3u;
            if (selector & 1u) != 0u { diffuse_alpha = fresnel; }
            if (selector & 2u) != 0u { specular_alpha = fresnel; }
        }
    }
    let global_ambient = lighting_vec3(8u);
    result.primary = vec4<f32>(clamp(diffuse + global_ambient, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(diffuse_alpha, 0.0, 1.0));
    result.secondary = vec4<f32>(clamp(specular, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(specular_alpha, 0.0, 1.0));
    return result;
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fragment(in: VertexOut) -> FragmentOut {
    var sampled0 = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var sampled1 = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var sampled2 = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if (state.words[3] & 1u) != 0u { sampled0 = textureSample(texture0, texture0_sampler, in.texcoord0); }
    if (state.words[3] & 2u) != 0u { sampled1 = textureSample(texture1, texture1_sampler, in.texcoord1); }
    if (state.words[3] & 4u) != 0u { sampled2 = textureSample(texture2, texture2_sampler, in.texcoord2); }
    let lit = shade_lighting(in.quaternion, in.view);
    let color = tev(in.color, lit.primary, lit.secondary, sampled0, sampled1, sampled2);
    let alpha = u32(clamp(color.a, 0.0, 1.0) * 255.0);
    if state.words[0] != 0u && !compare(alpha, state.words[2], state.words[1]) {
        discard;
    }
    var out: FragmentOut;
    out.color = color;
    var mapped_depth=-in.position.z*bitcast<f32>(state.words[37])+bitcast<f32>(state.words[38]);
    if state.words[39]!=0u {mapped_depth/=in.position.w;}
    out.depth = clamp(mapped_depth,0.0,1.0);
    if state.words[36] != 0u {
        let maximum = f32(state.words[36]);
        let integer_depth = floor(out.depth * maximum);
        // GPU division can use an approximate reciprocal, which breaks
        // equality against CPU-normalized guest depth. For D24, q/2^24 is
        // exact and q/(2^24-1) rounds to its next representable float for
        // every nonzero q. D16 uses a convergent power-of-two expansion.
        if state.words[36] == 16777215u {
            let base = integer_depth * (1.0 / 16777216.0);
            out.depth = bitcast<f32>(bitcast<u32>(base) + select(0u, 1u, integer_depth != 0.0));
        } else {
            let base = integer_depth * (1.0 / 65536.0);
            out.depth = base + (base * (1.0 / 65536.0) + base * (1.0 / 4294967296.0));
        }
    }
    return out;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout_includes_lighting_vectors() {
        let bytes =
            vertex_bytes(&[RasterVertex::new([1.0, -2.0, 3.0, 1.0], [0.25, 0.5, 0.75, 1.0]).with_texcoord0([0.125, 0.875])]);
        assert_eq!(bytes.len(), 84);
        assert_eq!(&bytes[0..4], &1.0f32.to_ne_bytes());
        assert_eq!(&bytes[16..20], &0.25f32.to_ne_bytes());
        assert_eq!(&bytes[32..36], &0.125f32.to_ne_bytes());
        assert_eq!(&bytes[68..72], &1.0f32.to_ne_bytes());
        assert_eq!(&bytes[72..76], &0.0f32.to_ne_bytes());
    }

    #[test]
    fn lighting_buffer_reserves_all_lights_and_luts() {
        let mut luts = vec![[0; LUT_SIZE]; LUT_COUNT];
        luts[23][255] = 0x00AB_CDEF;
        let state = LightingState {
            lights: Vec::new(),
            ambient: [0.25, 0.5, 0.75],
            config0: 1,
            config1: 2,
            abs: 3,
            select: 4,
            scale: 5,
            luts: luts.into(),
        };
        let words = lighting_words(&state).unwrap();
        assert_eq!(words.len(), LUT_BASE + LUT_COUNT * LUT_SIZE);
        assert_eq!(words[8], 0.25f32.to_bits());
        assert_eq!(words[LUT_BASE + 23 * LUT_SIZE + 255], 0x00AB_CDEF);
    }

    #[test]
    fn raw_pica_register_snapshots_decode_at_the_renderer_boundary() {
        let mut registers = vec![0u32; 0x300];
        registers[0x40] = 2;
        registers[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
        registers[0x113] = 1;
        registers[0x115] = 2;
        let raster = PicaRasterRegisters::from_pica_registers(&registers).unwrap();
        assert_eq!(RasterState::from_pica(raster).depth, DepthState::less_equal_write());

        for (index, base) in [0xC0, 0xC8, 0xD0, 0xD8, 0xF0, 0xF8].into_iter().enumerate() {
            registers[base] = index as u32;
            registers[base + 3] = 0x0102_0304 + index as u32;
        }
        registers[0xFD] = 0xAABB_CCDD;
        registers[0xE0] = 0x1234;
        let tev = TevState::from_pica_registers(&registers).unwrap();
        assert_eq!(tev.stages[4].sources, 4);
        assert_eq!(tev.stages[5].constant, 0x0102_0309);
        assert_eq!(tev.buffer_color, 0xAABB_CCDD);

        assert!(LightingState::from_pica_registers(&registers, &[]).unwrap().is_none());
        registers[0x8F] = 1;
        registers[0x1C2] = 0;
        registers[0x1D9] = 0;
        registers[0x142] = 255 << 20 | 255 << 10 | 255;
        registers[0x145] = 0x3C00;
        registers[0x149] = 1;
        let lighting = LightingState::from_pica_registers(&registers, &[[0; 256]; 24]).unwrap().unwrap();
        assert_eq!(lighting.lights.len(), 1);
        assert_eq!(lighting.lights[0].diffuse, [1.0; 3]);
        assert_eq!(lighting.lights[0].position, [0.0, 0.0, 1.0]);
        assert!(lighting.lights[0].directional);
    }

    #[test]
    fn complete_pica_draw_state_decodes_one_immutable_snapshot() {
        let mut registers = vec![0u32; 0x300];
        registers[0x41] = 0x469000; // float24 200.0
        registers[0x43] = 0x45E000; // float24 120.0
        registers[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
        registers[0x113] = 1;
        registers[0x115] = 2;
        registers[0xC0] = 3;
        let state = PicaDrawState::from_pica_registers(&registers, &[], 400, 240).unwrap();
        assert_eq!(state.viewport, Viewport { x: 0.0, y: 0.0, width: 400.0, height: 240.0 });
        assert_eq!(state.raster.depth, DepthState::less_equal_write());
        assert_eq!(state.raster.tev.stages[0].sources, 3);
        assert_eq!(state.lighting, None);
        assert_eq!(state.raster.depth_max, 0xFFFF);
        for format in [2, 3] {
            registers[0x116] = format;
            assert_eq!(PicaDrawState::from_pica_registers(&registers, &[], 400, 240).unwrap().raster.depth_max, 0xFF_FFFF);
        }

        registers[0x68] = 0x3FF; // signed viewport x offset -1
        assert!(PicaDrawState::from_pica_registers(&registers, &[], 400, 240).is_err());
    }

    #[test]
    fn invalid_triangle_lists_are_rejected_before_a_gpu_is_needed() {
        assert!(validate_vertices(0).is_err());
        assert!(validate_vertices(2).is_err());
        assert!(validate_vertices(3).is_ok());
        assert!(validate_target(0, 16).is_err());
        assert!(validate_target(16, 0).is_err());
        assert!(validate_target(16, 16).is_ok());
    }

    #[test]
    fn pica_viewport_flips_the_bottom_origin() {
        assert_eq!(
            Viewport::from_pica_dimensions(240, 200.0, 120.0, 0.0, 0.0),
            Viewport { x: 0.0, y: 0.0, width: 400.0, height: 240.0 }
        );
        assert_eq!(
            Viewport::from_pica_dimensions(240, 100.0, 60.0, 50.0, 30.0),
            Viewport { x: 50.0, y: 90.0, width: 200.0, height: 120.0 }
        );
    }

    #[test]
    fn pica_culling_modes_decode() {
        assert_eq!(CullMode::from_pica(0), CullMode::None);
        assert_eq!(CullMode::from_pica(1), CullMode::Front);
        assert_eq!(CullMode::from_pica(2), CullMode::Back);
        assert_eq!(CullMode::from_pica(3), CullMode::None);
    }

    #[test]
    fn pica_depth_state_decodes_both_registers() {
        assert_eq!(DepthState::from_pica(0, 0), DepthState { compare: DepthCompare::Always, write: false });
        assert_eq!(DepthState::from_pica(1 | 4 << 4, 0), DepthState { compare: DepthCompare::Less, write: false });
        assert_eq!(
            DepthState::from_pica(1 | 7 << 4 | 1 << 12, 2),
            DepthState { compare: DepthCompare::GreaterEqual, write: true }
        );
        assert_eq!(DepthState::from_pica(1 | 5 << 4 | 1 << 12, 0), DepthState { compare: DepthCompare::LessEqual, write: false });
    }

    #[test]
    fn pica_alpha_test_decodes_enable_compare_and_reference() {
        assert_eq!(AlphaTest::from_pica(0xFFFF_FFFE), None);
        assert_eq!(
            AlphaTest::from_pica(1 | 6 << 4 | 0xA5 << 8),
            Some(AlphaTest { compare: DepthCompare::Greater, reference: 0xA5 })
        );
    }

    #[test]
    fn pica_color_mask_keeps_four_channels() {
        assert_eq!(ColorMask::from_bits(0xFFFF_FFF5).bits(), 0b0101);
        assert_eq!(ColorMask::from_pica(0b0101 << 8, 1).bits(), 0b0101);
        assert_eq!(ColorMask::from_pica(0b1111 << 8, 0).bits(), 0);
        assert_eq!(ColorMask::ALL.bits(), 0b1111);
    }

    #[test]
    fn pica_blend_state_decodes_equations_factors_and_constant() {
        assert_eq!(BlendState::from_pica(0, u32::MAX, u32::MAX), None);
        let functions = 2 | 4 << 8 | 6 << 16 | 7 << 20 | 1 << 24 | 9 << 28;
        let blend = BlendState::from_pica(1 << 8, functions, 0x4433_2211).unwrap();
        assert_eq!(
            blend.color,
            BlendComponent {
                source: BlendFactor::SrcAlpha,
                destination: BlendFactor::OneMinusSrcAlpha,
                operation: BlendOperation::ReverseSubtract,
            }
        );
        assert_eq!(
            blend.alpha,
            BlendComponent {
                source: BlendFactor::One,
                destination: BlendFactor::OneMinusDstAlpha,
                operation: BlendOperation::Max,
            }
        );
        assert_eq!(blend.constant, [0x11 as f32 / 255.0, 0x22 as f32 / 255.0, 0x33 as f32 / 255.0, 0x44 as f32 / 255.0]);

        let conflicting = BlendState::from_pica(1 << 8, 10 << 16 | 12 << 20, 0).unwrap();
        assert!(conflicting.host_constant().is_err());
    }

    #[test]
    fn pica_register_bundle_builds_one_raster_state() {
        let registers = PicaRasterRegisters {
            cull_mode: 2,
            depth_color_mask: 1 | 4 << 4 | 0b0101 << 8 | 1 << 12,
            color_write_permissions: 1,
            depth_write_permissions: 2,
            color_operation: 1 << 8,
            blend_functions: 6 << 16 | 7 << 20 | 1 << 24,
            blend_constant: 0x4433_2211,
            alpha_test: 1 | 6 << 4 | 0x80 << 8,
            ..Default::default()
        };
        let state = RasterState::from_pica(registers);
        assert_eq!(state.cull, CullMode::Back);
        assert_eq!(state.depth, DepthState { compare: DepthCompare::Less, write: true });
        assert_eq!(state.color_mask.bits(), 0b0101);
        assert!(state.blend.is_some());
        assert_eq!(state.alpha_test, Some(AlphaTest { compare: DepthCompare::Greater, reference: 0x80 }));
    }

    #[test]
    fn pica_sampler_state_decodes_wrap_and_filter() {
        assert_eq!(
            SamplerState::from_pica(2 << 12 | 3 << 8 | 1 << 1),
            SamplerState { wrap_u: WrapMode::Repeat, wrap_v: WrapMode::MirrorRepeat, filter: FilterMode::Linear }
        );
        assert!(WrapMode::ClampToBorder.wgpu(false).is_err());
        assert_eq!(WrapMode::ClampToBorder.wgpu(true).unwrap(), wgpu::AddressMode::ClampToBorder);
        assert_eq!(WrapMode::ClampToEdge.wgpu(false).unwrap(), wgpu::AddressMode::ClampToEdge);
    }

    #[test]
    fn default_tev_passes_primary_and_modulate_uses_texture_zero() {
        let default = TevState::default();
        assert_eq!(default.stages[0], TevStage::replace(0));
        assert!(default.stages[1..].iter().all(|stage| *stage == TevStage::replace(15)));
        let modulate = TevState::modulate_texture0();
        assert_eq!(modulate.stages[0].sources & 0xFF, 3 << 4);
        assert_eq!(modulate.stages[0].combiner, 1 | 1 << 16);
    }
}
