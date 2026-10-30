//! Logical guest device writes backed by immutable GPU readback tickets.
use super::*;
use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Default)]
pub(super) struct Stats {
    queued: AtomicU64,
    materialized: AtomicU64,
    nanos: AtomicU64,
    pub(super) uniform_depth_imports: AtomicU64,
}

impl Stats {
    pub(super) fn get(&self) -> (u64, u64, std::time::Duration) {
        (
            self.queued.load(Ordering::Relaxed),
            self.materialized.load(Ordering::Relaxed),
            std::time::Duration::from_nanos(self.nanos.load(Ordering::Relaxed)),
        )
    }
}
use usum_guest::{memory::DeferredBytes, Fault};

type EncodedPlanes = (Vec<u8>, Vec<u8>);

#[derive(Debug)]
pub(super) struct DeferredTarget {
    info: CaptureTarget,
    stats: Arc<Stats>,
    ticket: Arc<usum_render::PendingReadback>,
    /// Identity-only seed for an already resident target. Empty pixel arrays
    /// deliberately fail upload validation if the residency proof is lost.
    seed: Arc<TargetSnapshot>,
    encoded: OnceLock<Result<EncodedPlanes, Fault>>,
    split_encode: bool,
    color_encoded: OnceLock<Result<Vec<u8>, Fault>>,
    depth_encoded: OnceLock<Result<Vec<u8>, Fault>>,
    counted: OnceLock<()>,
    views: Mutex<Vec<FrozenView>>,
}

#[derive(Debug)]
struct Plane {
    target: Arc<DeferredTarget>,
    depth: bool,
}

impl DeferredBytes for Plane {
    fn len(&self) -> usize {
        self.target.info.width as usize * self.target.info.height as usize * 4
    }
    fn bytes(&self) -> Result<&[u8], Fault> {
        self.target.bytes(self.depth)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DeferredTarget {
    fn bytes(&self, depth: bool) -> Result<&[u8], Fault> {
        if !self.split_encode {
            let (color, z) = self.encoded.get_or_init(|| self.encode()).as_ref().map_err(|f| *f)?;
            return Ok(if depth { z } else { color });
        }
        let cache = if depth { &self.depth_encoded } else { &self.color_encoded };
        cache.get_or_init(|| self.encode_plane(depth)).as_deref().map_err(|f| *f)
    }

    fn encode_plane(&self, depth: bool) -> Result<Vec<u8>, Fault> {
        let started = std::time::Instant::now();
        let read = self.ticket.read().map_err(|_| Fault::Device { address: self.info.color_address })?;
        let mut bytes = if depth { vec![0; read.depth.len() * 4] } else { vec![0; read.color.len()] };
        if depth {
            encode_d24s8_target(&read.depth, Some(&read.stencil), &mut bytes, self.info.width);
        } else {
            encode_rgba8_target(&read.color, &mut bytes, self.info.width);
        }
        // Count a frozen target once, even when concurrent readers request both
        // planes. The ticket still maps the complete immutable GPU snapshot.
        self.counted.get_or_init(|| {
            self.stats.materialized.fetch_add(1, Ordering::Relaxed);
        });
        self.stats.nanos.fetch_add(started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
        Ok(bytes)
    }

    fn encode(&self) -> Result<(Vec<u8>, Vec<u8>), Fault> {
        let started = std::time::Instant::now();
        let read = self.ticket.read().map_err(|_| Fault::Device { address: self.info.color_address })?;
        let mut color = vec![0; read.color.len()];
        let mut depth = vec![0; read.depth.len() * 4];
        encode_rgba8_target(&read.color, &mut color, self.info.width);
        encode_d24s8_target(&read.depth, Some(&read.stencil), &mut depth, self.info.width);
        self.stats.materialized.fetch_add(1, Ordering::Relaxed);
        self.stats.nanos.fetch_add(started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
        Ok((color, depth))
    }
}

pub(super) fn eligible(pica: &Pica, info: CaptureTarget) -> bool {
    pica.vulkan_defer_readback
        && pica.vulkan_accelerate
        && pica.vulkan_live_depth
        && (pica.vulkan_resolution_scale == 1 || std::env::var("USUM_VULKAN_NATIVE_READBACK").as_deref() != Ok("0"))
        && info.color_format == 0
        && info.depth_format == 3
        && info.depth_address != 0
        && info.width.is_multiple_of(8)
        && info.height.is_multiple_of(8)
        && info.width != 0
        && info.height != 0
        && info
            .width
            .checked_mul(info.height)
            .and_then(|n| n.checked_mul(4))
            .is_some_and(|size| !ranges_overlap(info.color_address, size, info.depth_address, size))
}

pub(super) fn resident_color<'a>(pica: &'a Pica, mem: &Memory, info: CaptureTarget) -> Option<&'a Arc<DeferredTarget>> {
    if !eligible(pica, info) {
        return None;
    }
    let pending = pica.vulkan_live_target_cache.iter().find(|t| t.info == info)?.pending.as_ref()?;
    let len = info.width as usize * info.height as usize * 4;
    let address = pica.pa_to_va(info.color_address).ok()?;
    let plane = mem.deferred_source(address, len)?.as_any().downcast_ref::<Plane>()?;
    (!plane.depth && Arc::ptr_eq(&plane.target, pending)).then_some(pending)
}

/// The current physical depth allocation can belong to a different color
/// target. Match the actual immutable guest-plane owner, not just its address
/// or native sample values: those cannot prove the subpixel generation.
pub(super) fn depth_source<'a>(pica: &'a Pica, mem: &Memory, info: CaptureTarget) -> Option<&'a LiveTarget> {
    if !eligible(pica, info) {
        return None;
    }
    let len = info.width as usize * info.height as usize * 4;
    let address = pica.pa_to_va(info.depth_address).ok()?;
    let plane = mem.deferred_source(address, len)?.as_any().downcast_ref::<Plane>()?;
    if !plane.depth {
        return None;
    }
    pica.vulkan_live_target_cache.iter().find(|target| {
        target.info.depth_address == info.depth_address
            && target.info.depth_format == info.depth_format
            && target.info.width == info.width
            && target.info.height == info.height
            && target.pending.as_ref().is_some_and(|pending| Arc::ptr_eq(pending, &plane.target))
    })
}

pub(super) fn resident_seed(pica: &Pica, mem: &Memory, info: CaptureTarget) -> Option<Arc<TargetSnapshot>> {
    let color = resident_color(pica, mem, info)?;
    depth_source(pica, mem, info)?;
    Some(Arc::clone(&color.seed))
}

pub(super) fn color_matches(pica: &Pica, mem: &Memory, info: CaptureTarget, seed: &Arc<TargetSnapshot>) -> bool {
    matches(resident_color(pica, mem, info), seed)
}

pub(super) fn matches(pending: Option<&Arc<DeferredTarget>>, seed: &Arc<TargetSnapshot>) -> bool {
    pending.is_some_and(|p| Arc::ptr_eq(&p.seed, seed))
}

pub(super) fn install(
    pica: &mut Pica,
    mem: &mut Memory,
    info: CaptureTarget,
    ticket: Arc<usum_render::PendingReadback>,
) -> Result<(), String> {
    if ticket.dimensions() != (info.width, info.height) {
        return Err("deferred result must retain native guest dimensions".into());
    }
    let pending = Arc::new(DeferredTarget {
        info,
        stats: Arc::clone(&pica.vulkan_live_resources.deferred),
        ticket,
        seed: Arc::new(TargetSnapshot {
            width: info.width,
            height: info.height,
            color: Vec::new(),
            depth: Vec::new(),
            stencil: Vec::new(),
        }),
        encoded: OnceLock::new(),
        split_encode: pica.vulkan_split_encode,
        color_encoded: OnceLock::new(),
        depth_encoded: OnceLock::new(),
        counted: OnceLock::new(),
        views: Mutex::default(),
    });
    let color = pica.pa_to_va(info.color_address)?;
    let depth = pica.pa_to_va(info.depth_address)?;
    mem.defer_device_write(color, Arc::new(Plane { target: Arc::clone(&pending), depth: false })).map_err(|e| e.to_string())?;
    mem.defer_device_write(depth, Arc::new(Plane { target: Arc::clone(&pending), depth: true })).map_err(|e| e.to_string())?;
    let target = pica.vulkan_live_target_cache.iter_mut().find(|t| t.info == info).ok_or("deferred target disappeared")?;
    target.contents = None;
    target.pending = Some(pending);
    pica.vulkan_live_resources.deferred.queued.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// CPU texture/display readers may already have materialized an immutable
/// ticket. Publish its native proof for scaled GPU texture/presentation reuse,
/// without introducing another wait or reading an unused ticket.
pub(super) fn hydrate_ready(pica: &mut Pica) {
    for target in &mut pica.vulkan_live_target_cache {
        if target.contents.is_some() {
            continue;
        }
        let Some(pending) = &target.pending else {
            continue;
        };
        let Some(read) = pending.ticket.ready() else {
            continue;
        };
        target.contents = Some(Arc::new(TargetSnapshot {
            width: target.info.width,
            height: target.info.height,
            color: read.color.clone(),
            depth: read.depth.clone(),
            stencil: read.stencil.clone(),
        }));
    }
}

#[derive(Debug)]
struct FrozenView {
    depth: bool,
    sampler: SamplerState,
    border: [u8; 4],
    gpu: Arc<SampledTexture>,
}

/// Immutable GPU view with an on-demand native reference for diagnostics.
#[derive(Debug)]
pub struct FrozenTexture {
    source: Arc<DeferredTarget>,
    depth: bool,
    pub(super) gpu: Arc<SampledTexture>,
    pixels: OnceLock<Result<Arc<[u8]>, String>>,
}

impl PartialEq for FrozenTexture {
    fn eq(&self, other: &Self) -> bool {
        self.depth == other.depth && Arc::ptr_eq(&self.source, &other.source)
    }
}

impl FrozenTexture {
    pub(super) fn pixels(&self) -> Result<&Arc<[u8]>, String> {
        self.pixels
            .get_or_init(|| {
                // Use the normal provider so accounting and later guest reads also
                // observe this materialization, including deferred device failures.
                self.source.bytes(self.depth).map_err(|e| e.to_string())?;
                let read = self.source.ticket.read()?;
                let (w, h) = (self.source.info.width as usize, self.source.info.height as usize);
                let mut pixels = Vec::with_capacity(w * h * 4);
                for y in (0..h).rev() {
                    if self.depth {
                        for x in 0..w {
                            let at = y * w + x;
                            let depth = (read.depth[at].clamp(0.0, 1.0) * 0xFF_FFFF as f32) as u32;
                            pixels.extend_from_slice(&(depth.swap_bytes() | u32::from(read.stencil[at])).to_le_bytes());
                        }
                    } else {
                        pixels.extend_from_slice(&read.color[y * w * 4..(y + 1) * w * 4]);
                    }
                }
                Ok(pixels.into())
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

pub(super) fn texture_snapshot(pica: &Pica, mem: &Memory, unit: usize, enabled: bool) -> Result<Option<TextureSnapshot>, String> {
    if !pica.vulkan_live
        || !pica.vulkan_defer_readback
        || !enabled
        || std::env::var("USUM_VULKAN_RESIDENT_TEXTURES").as_deref() == Ok("0")
    {
        return Ok(None);
    }
    let Some((address, format, width, height)) = super::super::texture::unit_key(pica, unit) else {
        return Ok(None);
    };
    if format != 0 {
        return Ok(None);
    }
    let Some(size) = width.checked_mul(height).and_then(|n| n.checked_mul(4)) else {
        return Ok(None);
    };
    let Some(plane) = mem.deferred_source(address, size as usize).and_then(|s| s.as_any().downcast_ref::<Plane>()) else {
        return Ok(None);
    };
    let info = plane.target.info;
    if (width, height) != (info.width, info.height)
        || plane.depth && std::env::var("USUM_VULKAN_RESIDENT_DEPTH_TEXTURE").as_deref() == Ok("0")
    {
        return Ok(None);
    }
    let Some(source) = pica
        .vulkan_live_target_cache
        .iter()
        .find(|t| t.info == info && t.pending.as_ref().is_some_and(|p| Arc::ptr_eq(p, &plane.target)))
    else {
        return Ok(None);
    };
    let reg = [0x81, 0x91, 0x99][unit];
    let sampler = SamplerState::from_pica(pica.regs[reg + 2]);
    let border = pica.regs[reg].to_le_bytes();
    let mut views = plane.target.views.lock().map_err(|_| "frozen texture cache poisoned")?;
    let gpu = match views.iter().find(|v| v.depth == plane.depth && v.sampler == sampler && v.border == border) {
        Some(view) => Arc::clone(&view.gpu),
        None => {
            let renderer = live_renderer()?;
            let gpu = Arc::new(if plane.depth {
                renderer.create_d24s8_feedback_texture(&source.gpu, sampler, border)?
            } else {
                renderer.create_color_feedback_texture(&source.gpu, sampler, border)?
            });
            views.push(FrozenView { depth: plane.depth, sampler, border, gpu: Arc::clone(&gpu) });
            gpu
        }
    };
    Ok(Some(TextureSnapshot {
        address,
        source_size: size,
        format,
        width,
        height,
        pixels: Arc::from([]),
        sampler,
        border,
        frozen: Some(Arc::new(FrozenTexture {
            source: Arc::clone(&plane.target),
            depth: plane.depth,
            gpu,
            pixels: OnceLock::new(),
        })),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use usum_guest::Perm;

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn shared_depth_planes_stay_resident_and_cpu_writes_break_the_proof() {
        use usum_render::{ColorMask, CullMode, DepthCompare, DepthState, StencilOperation, StencilState, TevState};
        #[derive(Debug, PartialEq, Eq)]
        struct Output {
            native: Vec<Vec<u8>>,
            scaled: Vec<Vec<u8>>,
        }
        fn run(defer: bool, scale: u32, cpu_plane: u32) -> Output {
            let renderer = live_renderer().unwrap();
            let mut pica = Pica::default();
            pica.enable_vulkan_acceleration();
            pica.set_vulkan_resolution_scale(scale).unwrap();
            if defer {
                pica.enable_vulkan_deferred_readback();
            }
            pica.regs[0x41] = 0x430000;
            pica.regs[0x43] = 0x430000;
            pica.regs[0x4d] = 0xbf0000;
            pica.regs[0x6d] = 1;
            let first = CaptureTarget {
                color_address: 0x2000_0000,
                depth_address: 0x2000_2000,
                width: 32,
                height: 32,
                color_format: 0,
                depth_format: 3,
            };
            let second = CaptureTarget { color_address: 0x2000_1000, ..first };
            let mut mem = Memory::new();
            mem.map(pica.linear_base, vec![0; 0x3000], Perm::RW).unwrap();
            mem.device_write(pica.linear_base + 0x2000, &[0xff; 4096]).unwrap();
            // Independent full-resolution reference: two color allocations and
            // one logical shared depth plane, without guest readback/re-upload.
            let side = 32 * scale;
            let count = (side * side) as usize;
            let reference = [0, 1].map(|_| {
                let gpu = renderer.create_color_target_from_snapshot(side, side, &vec![0; count * 4], &vec![1.0; count]).unwrap();
                let mut batch = renderer.begin_batch(&gpu);
                batch.seed_stencil(&[0xff; 1024], scale).unwrap();
                batch.submit().unwrap();
                gpu
            });
            for step in 0..8 {
                let info = if step % 2 == 0 { first } else { second };
                if step == 4 && (cpu_plane == 1 || cpu_plane == 2) {
                    let address = pica.linear_base + if cpu_plane == 1 { 4 } else { 0x2004 };
                    mem.write_u8(address, 0x91).unwrap();
                }
                if step == 1 && cpu_plane == 3 {
                    mem.device_write(pica.linear_base + 0x2000, &[0, 0, 0x40, 0x49].repeat(1024)).unwrap();
                }
                let before = pica.vulkan_live_resources.deferred.get().1;
                let color = [step as f32 / 8.0, 0.75, 0.5, 1.0];
                let vertices = [
                    Vertex { position: [-0.83, -0.81, -0.25 + step as f32 * 0.015, 1.0], color, ..Default::default() },
                    Vertex { position: [0.87, -0.79, -0.72, 1.0], color, ..Default::default() },
                    Vertex { position: [0.03, 0.89, -0.50, 1.0], color, ..Default::default() },
                ];
                let (target, textures) = capture_buffers(&mut pica, &mem, info).unwrap();
                let mut snapshot =
                    DrawSnapshot::from_shaded_vertices_shared(&pica, &vertices, &[[0, 1, 2]], info, target, textures).unwrap();
                snapshot.draw_number = step + 1;
                snapshot.state.lighting = None;
                snapshot.state.raster.tev = TevState::default();
                snapshot.state.raster.blend = None;
                snapshot.state.raster.cull = CullMode::None;
                snapshot.state.raster.color_mask = ColorMask::ALL;
                snapshot.state.raster.depth_max = 0xff_ffff;
                snapshot.state.raster.depth = DepthState { compare: DepthCompare::LessEqual, write: true };
                snapshot.state.raster.stencil = StencilState {
                    enabled: true,
                    write_mask: 0xff,
                    compare: if step == 0 { DepthCompare::Always } else { DepthCompare::Equal },
                    reference: if step == 0 { 1 } else { step as u8 },
                    pass: if step == 0 { StencilOperation::Replace } else { StencilOperation::IncrementClamp },
                    ..Default::default()
                };
                if cpu_plane == 3 && step >= 1 {
                    snapshot.state.raster.depth = DepthState { compare: DepthCompare::Always, write: false };
                    snapshot.state.raster.stencil = StencilState::default();
                }
                if cpu_plane == 0 || cpu_plane == 3 {
                    let index = step as usize % 2;
                    let gpu = &reference[index];
                    let depth = if step != 0 && !(cpu_plane == 3 && step == 1) {
                        Some(renderer.snapshot_depth(&reference[1 - index]).unwrap())
                    } else {
                        None
                    };
                    let mut batch = if cpu_plane == 3 && step == 1 {
                        // A full CPU clear replaces shared depth and stencil.
                        let mut batch = renderer
                            .begin_batch_from_snapshot(
                                gpu,
                                &vec![0; count * 4],
                                &vec![0x40_0000 as f32 / 0xff_ffff as f32; count],
                            )
                            .unwrap();
                        batch.seed_stencil(&[0x49; 1024], scale).unwrap();
                        batch
                    } else {
                        renderer.begin_batch(gpu)
                    };
                    if let Some(depth) = depth {
                        batch.restore_depth(&depth).unwrap();
                    }
                    let mut viewport = snapshot.state.viewport;
                    viewport.x *= scale as f32;
                    viewport.y *= scale as f32;
                    viewport.width *= scale as f32;
                    viewport.height *= scale as f32;
                    batch.draw(&snapshot.vertices, viewport, snapshot.state.raster).unwrap();
                    batch.submit().unwrap();
                }
                pica.vulkan_snapshots.push(snapshot);
                replay_live(&mut pica, &mut mem).unwrap();
                if defer && step >= 2 && !(step == 4 && (cpu_plane == 1 || cpu_plane == 2)) {
                    assert_eq!(
                        pica.vulkan_live_resources.deferred.get().1,
                        before,
                        "warm shared-depth switch must not map a ticket: scale {scale}, CPU plane {cpu_plane}, step {step}"
                    );
                }
                if cpu_plane == 3 && step >= 1 {
                    assert!(
                        !pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap().depth_detail,
                        "a copied uniform depth plane must clear stale subpixel-detail metadata"
                    );
                }
            }
            if defer && (cpu_plane == 0 || cpu_plane == 3) {
                for (info, expected) in [first, second].into_iter().zip(&reference) {
                    let actual = renderer
                        .read_color_target(&pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap().gpu)
                        .unwrap();
                    let expected = renderer.read_color_target(expected).unwrap();
                    assert_eq!(
                        actual.iter().zip(&expected).filter(|(a, b)| a != b).count(),
                        0,
                        "independent full-resolution reference: scale {scale}, CPU plane {cpu_plane}, target {info:?}"
                    );
                }
            }
            Output {
                native: [0, 0x1000, 0x2000].map(|offset| mem.read_range(pica.linear_base + offset, 4096).unwrap()).into(),
                scaled: [first, second]
                    .map(|info| {
                        renderer
                            .read_color_target(&pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap().gpu)
                            .unwrap()
                    })
                    .into(),
            }
        }
        for scale in [1, 6] {
            for cpu_plane in 0..4 {
                let actual = run(true, scale, cpu_plane);
                let expected = run(false, scale, cpu_plane);
                assert!(actual.scaled.iter().all(|color| color.iter().any(|&c| c != 0)));
                assert_eq!(actual.native, expected.native, "native samples: scale {scale}, CPU plane {cpu_plane}");
                // Synchronous import may re-expand native color when depth
                // changes. Only 1× is its full-resolution color reference.
                for (actual, expected) in actual.scaled.iter().zip(&expected.scaled).filter(|_| scale == 1) {
                    assert_eq!(
                        actual.iter().zip(expected).filter(|(a, b)| a != b).count(),
                        0,
                        "full-resolution color: scale {scale}, CPU plane {cpu_plane}"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn frozen_texture_capture_is_gpu_only_immutable_and_keeps_native_reference() {
        use usum_render::{DepthCompare, DepthState, RasterState, TevStage, Viewport};
        let renderer = live_renderer().unwrap();
        for scale in [1, 6] {
            for depth_plane in [false, true] {
                let mut pica = Pica::default();
                pica.enable_vulkan_acceleration();
                pica.enable_vulkan_deferred_readback();
                pica.set_vulkan_resolution_scale(scale).unwrap();
                let info = CaptureTarget {
                    color_address: 0x2000_0000,
                    depth_address: 0x2000_1000,
                    width: 16,
                    height: 16,
                    color_format: 0,
                    depth_format: 3,
                };
                let mut mem = Memory::new();
                mem.map(pica.linear_base, vec![0; 0x2000], Perm::RW).unwrap();
                let side = 16 * scale;
                let count = (side * side) as usize;
                let color: Vec<u8> = (0..count * 4).map(|n| (n * 37 + n / 19) as u8).collect();
                let depths = vec![0.5; count];
                let stencil: Vec<u8> = (0..256).map(|n| (n * 7) as u8).collect();
                let gpu = Arc::new(renderer.create_color_target_from_snapshot(side, side, &color, &depths).unwrap());
                let mut batch = renderer.begin_batch(&gpu);
                batch.seed_stencil(&stencil, scale).unwrap();
                let ticket = Arc::new(batch.submit_resolved_readback(scale).unwrap());
                pica.vulkan_live_target_cache.push(LiveTarget {
                    info,
                    gpu: Arc::clone(&gpu),
                    contents: None,
                    pending: None,
                    scaled_color: None,
                    last_draw: 1,
                    depth_detail: true,
                });
                install(&mut pica, &mut mem, info, Arc::clone(&ticket)).unwrap();
                pica.regs[0x80] = 1;
                pica.regs[0x82] = 16 << 16 | 16;
                let physical = if depth_plane { info.depth_address } else { info.color_address };
                pica.regs[0x85] = physical >> 3;
                assert!(texture_snapshot(&pica, &mem, 0, false).unwrap().is_none());
                let snapshot = texture_snapshot(&pica, &mem, 0, true).unwrap().unwrap();
                let again = texture_snapshot(&pica, &mem, 0, true).unwrap().unwrap();
                let frozen = snapshot.frozen.as_ref().unwrap();
                assert!(Arc::ptr_eq(&frozen.gpu, &again.frozen.as_ref().unwrap().gpu));
                assert!(ticket.ready().is_none(), "capturing a GPU texture must not map its source");
                assert_eq!(pica.vulkan_live_resources.deferred.get().1, 0);
                let mut expected = Vec::with_capacity(count * 4);
                for y in (0..side).rev() {
                    for x in 0..side {
                        if depth_plane {
                            expected.extend_from_slice(&[stencil[((y / scale) * 16 + x / scale) as usize], 0x7f, 0xff, 0xff]);
                        } else {
                            let at = ((y * side + x) * 4) as usize;
                            expected.extend_from_slice(&color[at..at + 4]);
                        }
                    }
                }
                let reference =
                    renderer.create_rgba8_texture_with_border(side, side, &expected, snapshot.sampler, snapshot.border).unwrap();
                // Reuse the source allocation after freezing it. Its old view and
                // immutable native ticket must both keep the previous generation.
                let _overwrite =
                    renderer.begin_batch_from_snapshot(&gpu, &vec![0; count * 4], &depths).unwrap().submit_readback().unwrap();
                let v = |x, y, u, v| RasterVertex::new([x, y, 0.0, 1.0], [1.0; 4]).with_texcoord0([u, v]);
                let vertices = [
                    v(-1.0, -1.0, 0.0, 0.0),
                    v(1.0, -1.0, 1.0, 0.0),
                    v(1.0, 1.0, 1.0, 1.0),
                    v(-1.0, -1.0, 0.0, 0.0),
                    v(1.0, 1.0, 1.0, 1.0),
                    v(-1.0, 1.0, 0.0, 1.0),
                ];
                let render = |texture: &SampledTexture| {
                    let target = renderer.create_color_target(side, side).unwrap();
                    let mut state =
                        RasterState { depth: DepthState { compare: DepthCompare::Always, write: false }, ..Default::default() };
                    state.tev.stages[0] = TevStage::replace(3);
                    renderer
                        .draw_vertices_with_inputs(
                            &target,
                            &vertices,
                            Viewport::full(&target),
                            state,
                            FragmentInputs { texture0: Some(texture), ..Default::default() },
                        )
                        .unwrap();
                    renderer.read_color_target(&target).unwrap()
                };
                let actual = render(&frozen.gpu);
                assert_eq!(actual, render(&reference));
                assert!(actual.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]));
                assert!(ticket.ready().is_none(), "sampling the frozen GPU view must not map its source");
                let native = super::super::super::texture::decoded_snapshot(&mut pica, &mem, 0).unwrap();
                assert_eq!(snapshot.resolved_pixels().unwrap().as_ref(), native.pixels.as_ref());
                let address = pica.pa_to_va(physical).unwrap();
                mem.write_u8(address, 0x91).unwrap();
                assert!(
                    texture_snapshot(&pica, &mem, 0, true).unwrap().is_none(),
                    "partial CPU writes invalidate the logical source"
                );
                pica.vulkan_live_target_cache.clear();
                drop(gpu);
                assert_eq!(actual, render(&frozen.gpu), "snapshot survives source destruction");
            }
        }
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn deferred_replay_matches_synchronous_lists_and_cpu_partial_writes() {
        fn run(defer: bool, scale: u32) -> (Vec<u8>, Vec<u8>, u64, Vec<u8>) {
            let mut pica = Pica::default();
            pica.enable_vulkan_acceleration();
            pica.set_vulkan_resolution_scale(scale).unwrap();
            if defer {
                pica.enable_vulkan_deferred_readback();
            }
            pica.regs[0x41] = 0x430000;
            pica.regs[0x43] = 0x430000;
            pica.regs[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
            pica.regs[0x113] = 1;
            pica.regs[0x115] = 2;
            let info = CaptureTarget {
                color_address: 0x2000_0000,
                depth_address: 0x2000_1000,
                width: 32,
                height: 32,
                color_format: 0,
                depth_format: 3,
            };
            let mut mem = Memory::new();
            mem.map(pica.linear_base, vec![0; 0x2000], Perm::RW).unwrap();
            mem.device_write(pica.linear_base + 0x1000, &[0xFF; 4096]).unwrap();
            for step in 0..5 {
                if step == 3 {
                    mem.write_u32(pica.linear_base + 4, 0x12345678).unwrap();
                }
                let z = -0.2 - step as f32 * 0.1;
                let color = [step as f32 / 5.0, 0.75, 0.5, 1.0];
                let vertices = [
                    Vertex { position: [-0.75, -0.75, z, 1.0], color, ..Vertex::default() },
                    Vertex { position: [0.75, -0.75, z, 1.0], color, ..Vertex::default() },
                    Vertex { position: [0.0, 0.75, z, 1.0], color, ..Vertex::default() },
                ];
                let (target, textures) = capture_buffers(&mut pica, &mem, info).unwrap();
                let snapshot =
                    DrawSnapshot::from_shaded_vertices_shared(&pica, &vertices, &[[0, 1, 2]], info, target, textures).unwrap();
                pica.vulkan_snapshots.push(snapshot);
                replay_live(&mut pica, &mut mem).unwrap();
                assert_eq!(pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap().pending.is_some(), defer);
            }
            (
                mem.read_range(pica.linear_base, 4096).unwrap(),
                mem.read_range(pica.linear_base + 0x1000, 4096).unwrap(),
                pica.vulkan_live_resources.target_reuses,
                live_renderer()
                    .unwrap()
                    .read_color_target(&pica.vulkan_live_target_cache.iter().find(|t| t.info == info).unwrap().gpu)
                    .unwrap(),
            )
        }
        for scale in [1, 6] {
            let sync = run(false, scale);
            let lazy = run(true, scale);
            assert_eq!(sync.0, lazy.0);
            assert_eq!(sync.1, lazy.1);
            assert_eq!(sync.3, lazy.3);
            assert!(lazy.2 >= 3, "expected resident reuse around the partial CPU write at scale {scale}");
        }
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn frozen_guest_planes_keep_rendered_stencil_and_residency_requires_both_planes() {
        for (split, depth_first) in [(false, false), (true, false), (true, true)] {
            let renderer = Renderer::new().unwrap();
            let mut pica = Pica::default();
            pica.enable_vulkan_acceleration();
            pica.enable_vulkan_deferred_readback();
            pica.vulkan_split_encode = split;
            let info = CaptureTarget {
                color_address: 0x2000_0000,
                depth_address: 0x2000_1000,
                width: 8,
                height: 8,
                color_format: 0,
                depth_format: 3,
            };
            let mut mem = Memory::new();
            mem.map(pica.linear_base, vec![0xA5; 0x2000], Perm::RW).unwrap();
            let gpu = Arc::new(renderer.create_color_target(8, 8).unwrap());
            let color: Vec<_> = (0..256).map(|n| (n * 73) as u8).collect();
            let depth: Vec<_> = (0..64).map(|n| n as f32 / 63.0).collect();
            renderer.upload_target(&gpu, &color, &depth).unwrap();
            let mut batch = renderer.begin_batch(&gpu);
            batch.seed_stencil(&[0x91; 64], 1).unwrap();
            let ticket = Arc::new(batch.submit_readback().unwrap());
            pica.vulkan_live_target_cache.push(LiveTarget {
                last_draw: 0,
                depth_detail: false,
                info,
                gpu: Arc::clone(&gpu),
                contents: None,
                pending: None,
                scaled_color: None,
            });
            install(&mut pica, &mut mem, info, ticket).unwrap();
            let pending = Arc::clone(pica.vulkan_live_target_cache[0].pending.as_ref().unwrap());
            let seed = resident_seed(&pica, &mem, info).unwrap();
            assert!(matches(pica.vulkan_live_target_cache[0].pending.as_ref(), &seed));
            let snapshot = mem.clone();
            // A newer GPU target must not leak into the frozen guest snapshot.
            renderer.upload_target(&gpu, &[9; 256], &[0.75; 64]).unwrap();
            let mut batch = renderer.begin_batch(&gpu);
            batch.seed_stencil(&[0xfe; 64], 1).unwrap();
            batch.submit().unwrap();
            let mut expected_color = vec![0; 256];
            let mut expected_depth = vec![0x91; 256];
            encode_rgba8_target(&color, &mut expected_color, 8);
            encode_d24s8_target(&depth, None, &mut expected_depth, 8);
            if depth_first {
                assert_eq!(mem.read_range(pica.linear_base + 0x1000, 256).unwrap(), expected_depth);
                assert!(pending.color_encoded.get().is_none(), "depth-only reads must leave color unencoded");
                assert!(pending.depth_encoded.get().is_some());
            }
            assert_eq!(mem.read_range(pica.linear_base, 256).unwrap(), expected_color);
            assert_eq!(pending.color_encoded.get().is_some(), split);
            assert_eq!(pending.depth_encoded.get().is_some(), depth_first, "encode only the planes actually read");
            assert_eq!(pending.encoded.get().is_some(), !split);
            assert_eq!(pica.vulkan_deferred_stats().1, 1);
            assert_eq!(mem.read_range(pica.linear_base + 0x1000, 256).unwrap(), expected_depth);
            assert_eq!(pending.depth_encoded.get().is_some(), split);
            assert_eq!(pica.vulkan_deferred_stats().1, 1, "two planes are still one materialized target");
            // Readback does not invalidate residency, but even a one-byte CPU write
            // removes that plane's identity and requires importing the guest bytes.
            assert!(resident_seed(&pica, &mem, info).is_some());
            mem.write_u8(pica.linear_base + 1, 7).unwrap();
            assert!(resident_seed(&pica, &mem, info).is_none());
            assert_eq!(snapshot.read_range(pica.linear_base, 256).unwrap(), expected_color);
            let mut expected = expected_color;
            expected[1] = 7;
            assert_eq!(mem.read_range(pica.linear_base, 256).unwrap(), expected);
        }
    }
}
