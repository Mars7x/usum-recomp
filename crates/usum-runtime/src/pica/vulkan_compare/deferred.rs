//! Logical guest device writes backed by immutable GPU readback tickets.
use super::*;
use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Default)]
pub(super) struct Stats {
    queued: AtomicU64,
    materialized: AtomicU64,
    nanos: AtomicU64,
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
        let (color, depth) = self.target.encoded.get_or_init(|| self.target.encode()).as_ref().map_err(|f| *f)?;
        Ok(if self.depth { depth } else { color })
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DeferredTarget {
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
        && pica.vulkan_resolution_scale == 1
        && pica.vulkan_accelerate
        && pica.vulkan_live_depth
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

pub(super) fn resident_seed(pica: &Pica, mem: &Memory, info: CaptureTarget) -> Option<Arc<TargetSnapshot>> {
    if !eligible(pica, info) {
        return None;
    }
    let pending = pica.vulkan_live_target_cache.iter().find(|t| t.info == info)?.pending.as_ref()?;
    let len = info.width as usize * info.height as usize * 4;
    for (physical, depth) in [(info.color_address, false), (info.depth_address, true)] {
        let address = pica.pa_to_va(physical).ok()?;
        let plane = mem.deferred_source(address, len)?.as_any().downcast_ref::<Plane>()?;
        if plane.depth != depth || !Arc::ptr_eq(&plane.target, pending) {
            return None;
        }
    }
    Some(Arc::clone(&pending.seed))
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

#[cfg(test)]
mod tests {
    use super::*;
    use usum_guest::Perm;

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn deferred_replay_matches_synchronous_lists_and_cpu_partial_writes() {
        fn run(defer: bool) -> (Vec<u8>, Vec<u8>, u64) {
            let mut pica = Pica::default();
            pica.enable_vulkan_acceleration();
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
            }
            (
                mem.read_range(pica.linear_base, 4096).unwrap(),
                mem.read_range(pica.linear_base + 0x1000, 4096).unwrap(),
                pica.vulkan_live_resources.target_reuses,
            )
        }
        let sync = run(false);
        let lazy = run(true);
        assert_eq!(sync.0, lazy.0);
        assert_eq!(sync.1, lazy.1);
        assert!(lazy.2 >= 3, "expected resident reuse around the partial CPU write");
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn frozen_guest_planes_keep_rendered_stencil_and_residency_requires_both_planes() {
        let renderer = Renderer::new().unwrap();
        let mut pica = Pica::default();
        pica.enable_vulkan_acceleration();
        pica.enable_vulkan_deferred_readback();
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
        assert_eq!(mem.read_range(pica.linear_base, 256).unwrap(), expected_color);
        assert_eq!(mem.read_range(pica.linear_base + 0x1000, 256).unwrap(), expected_depth);
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
