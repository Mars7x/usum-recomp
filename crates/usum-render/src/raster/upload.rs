//! Reuse draw allocations only after their command batch has been submitted.
//! Queue writes then follow the preceding batch on the same ordered queue.
use super::*;
use std::collections::VecDeque;
use std::num::NonZeroU64;

const MAX_ENTRIES: usize = 512;
const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 8 * 1024 * 1024;

struct Entry {
    buffer: wgpu::Buffer,
    capacity: u64,
    usage: wgpu::BufferUsages,
    order: u64,
}

fn usage_class(usage: wgpu::BufferUsages) -> Option<usize> {
    if usage == wgpu::BufferUsages::STORAGE {
        Some(0)
    } else if usage == wgpu::BufferUsages::INDEX {
        Some(1)
    } else if usage == wgpu::BufferUsages::VERTEX {
        Some(2)
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct Pool {
    // Retained capacities are powers of two through 8 MiB. Searching these
    // 24 size classes avoids scanning hundreds of individual idle buffers.
    available: [[VecDeque<Entry>; 24]; 3],
    entries: usize,
    bytes: u64,
    order: u64,
    created: u64,
    reused: u64,
}

impl Pool {
    fn take(&mut self, size: u64, usage: wgpu::BufferUsages) -> Option<Entry> {
        if let Some(kind) = usage_class(usage) {
            let first = size.max(4).checked_next_power_of_two().map(|n| n.trailing_zeros() as usize);
            if let Some(first) = first.filter(|&first| first < 24) {
                for bin in &mut self.available[kind][first..] {
                    if let Some(entry) = bin.pop_back() {
                        self.entries -= 1;
                        self.bytes -= entry.capacity;
                        self.reused += 1;
                        return Some(entry);
                    }
                }
            }
        }
        self.created += 1;
        None
    }

    fn retain(&mut self, mut entry: Entry, kind: usize) -> Vec<Entry> {
        let mut evicted = Vec::new();
        while self.entries >= MAX_ENTRIES || self.bytes + entry.capacity > MAX_BYTES {
            let mut oldest: Option<(u64, usize, usize)> = None;
            for (kind, bins) in self.available.iter().enumerate() {
                for (size, bin) in bins.iter().enumerate() {
                    if let Some(entry) = bin.front() {
                        if oldest.is_none_or(|(order, _, _)| entry.order < order) {
                            oldest = Some((entry.order, kind, size));
                        }
                    }
                }
            }
            let (_, kind, size) = oldest.expect("retained upload accounting");
            let old = self.available[kind][size].pop_front().expect("oldest upload");
            self.entries -= 1;
            self.bytes -= old.capacity;
            evicted.push(old);
        }
        self.order = self.order.wrapping_add(1);
        entry.order = self.order;
        self.entries += 1;
        self.bytes += entry.capacity;
        self.available[kind][entry.capacity.trailing_zeros() as usize].push_back(entry);
        evicted
    }

    pub(super) fn stats(&self) -> (u64, u64, usize, u64) {
        (self.created, self.reused, self.entries, self.bytes)
    }
}

pub(super) struct Lease {
    pool: Arc<Mutex<Pool>>,
    entry: Option<Entry>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(entry) = self.entry.take() else { return };
        if entry.capacity > MAX_ENTRY_BYTES || !entry.capacity.is_power_of_two() {
            return;
        }
        let Some(kind) = usage_class(entry.usage) else { return };
        // Destroy evicted GPU handles outside the pool lock.
        let evicted = {
            let mut pool = self.pool.lock().unwrap_or_else(|p| p.into_inner());
            pool.retain(entry, kind)
        };
        drop(evicted);
    }
}

pub(super) struct Uploaded {
    buffer: wgpu::Buffer,
    len: u64,
}

impl Uploaded {
    pub(super) fn binding(&self) -> wgpu::BindingResource<'_> {
        // Capacity must not change shader arrayLength or make old trailing
        // vertices/uniforms visible to a shorter subsequent draw.
        wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.buffer, offset: 0, size: NonZeroU64::new(self.len) })
    }

    pub(super) fn slice(&self) -> wgpu::BufferSlice<'_> {
        self.buffer.slice(..self.len)
    }
}

impl Renderer {
    /// Allocation/reuse counts and retained pool size, for controlled diagnostics.
    pub fn draw_upload_stats(&self) -> (u64, u64, usize, u64) {
        self.raster_pipelines.uploads.lock().unwrap_or_else(|p| p.into_inner()).stats()
    }

    pub(super) fn upload_draw_buffer(
        &self,
        leases: &mut Vec<Lease>,
        label: &str,
        bytes: &[u8],
        usage: wgpu::BufferUsages,
    ) -> Result<Uploaded, String> {
        let len = bytes.len() as u64;
        if !self.raster_pipelines.pooled_uploads {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: len,
                usage,
                mapped_at_creation: true,
            });
            buffer.get_mapped_range_mut(..).map_err(|e| e.to_string())?.copy_from_slice(bytes);
            buffer.unmap();
            return Ok(Uploaded { buffer, len });
        }
        let limit = self.device.limits().max_buffer_size;
        let capacity = len.checked_next_power_of_two().filter(|&n| n <= limit).unwrap_or(len).max(wgpu::COPY_BUFFER_ALIGNMENT);
        if len == 0 || !len.is_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT) || len > limit {
            return Err("draw upload must be nonempty, word-aligned and within the device buffer limit".into());
        }
        let pool = &self.raster_pipelines.uploads;
        let available = pool.lock().map_err(|_| "Vulkan draw upload pool lock was poisoned")?.take(len, usage);
        let entry = available.unwrap_or_else(|| Entry {
            buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity,
            usage,
            order: 0,
        });
        self.queue.write_buffer(&entry.buffer, 0, bytes);
        let uploaded = Uploaded { buffer: entry.buffer.clone(), len };
        // A lease survives every draw in this encoder, including overlapping
        // batches submitted in reverse order. Release follows submit or discard.
        leases.push(Lease { pool: Arc::clone(pool), entry: Some(entry) });
        Ok(uploaded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> Renderer {
        let mut renderer = Renderer::new().unwrap();
        renderer.raster_pipelines.pooled_uploads = true;
        renderer
    }

    fn triangle(color: [f32; 4]) -> [RasterVertex; 3] {
        [[-0.9, -0.9, 0.0, 1.0], [0.9, -0.9, 0.0, 1.0], [0.0, 0.9, 0.0, 1.0]].map(|p| RasterVertex::new(p, color))
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn upload_leases_preserve_reverse_submission_pending_snapshots_and_discarded_batches() {
        let renderer = renderer();
        let targets: Vec<_> = (0..5).map(|_| renderer.create_color_target(16, 16).unwrap()).collect();
        let viewport = Viewport { x: 0.0, y: 0.0, width: 16.0, height: 16.0 };
        let mut a = renderer.begin_batch(&targets[0]);
        a.draw(&triangle([1.0, 0.0, 0.0, 1.0]), viewport, RasterState::default()).unwrap();
        let mut b = renderer.begin_batch(&targets[1]);
        b.draw(&triangle([0.0, 0.0, 1.0, 1.0]), viewport, RasterState::default()).unwrap();
        assert_eq!(renderer.draw_upload_stats(), (2, 0, 0, 0));
        let blue = b.submit_readback().unwrap();
        let mut c = renderer.begin_batch(&targets[2]);
        c.draw(&triangle([0.0, 1.0, 0.0, 1.0]), viewport, RasterState::default()).unwrap();
        // Validation scopes are a stack: finish the newest batch before a.
        // c already reuses b's upload while a remains unsubmitted.
        let green = c.submit_readback().unwrap();
        let red = a.submit_readback().unwrap();
        let mut discarded = renderer.begin_batch(&targets[3]);
        discarded.draw(&triangle([1.0, 0.0, 1.0, 1.0]), viewport, RasterState::default()).unwrap();
        drop(discarded);
        let mut d = renderer.begin_batch(&targets[4]);
        d.draw(&triangle([1.0, 1.0, 0.0, 1.0]), viewport, RasterState::default()).unwrap();
        let yellow = d.submit_readback().unwrap();
        let center = (8 * 16 + 8) * 4;
        for (ticket, expected) in
            [(blue, [0, 0, 255, 255]), (red, [255, 0, 0, 255]), (green, [0, 255, 0, 255]), (yellow, [255, 255, 0, 255])]
        {
            assert_eq!(ticket.read().unwrap().color[center..center + 4], expected);
        }
        assert_eq!(renderer.draw_upload_stats().0, 2);
        assert_eq!(renderer.draw_upload_stats().1, 3);
        assert!(renderer.read_color_target(&targets[3]).unwrap().iter().all(|&b| b == 0));
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn shorter_uploads_bind_only_their_data_and_keep_usage_classes_separate() {
        let renderer = renderer();
        let mut first = Vec::new();
        let storage = renderer.upload_draw_buffer(&mut first, "long storage", &[7; 12], wgpu::BufferUsages::STORAGE).unwrap();
        assert_eq!(storage.buffer.size(), 16);
        let wgpu::BindingResource::Buffer(binding) = storage.binding() else { panic!("buffer binding") };
        assert_eq!(binding.size.unwrap().get(), 12);
        let mut indices = Vec::new();
        renderer.upload_draw_buffer(&mut indices, "indices", &[9; 12], wgpu::BufferUsages::INDEX).unwrap();
        assert_eq!(renderer.draw_upload_stats().0, 2, "a different usage cannot reuse the storage allocation");
        drop(first);
        let mut short = Vec::new();
        let storage = renderer.upload_draw_buffer(&mut short, "short storage", &[3; 4], wgpu::BufferUsages::STORAGE).unwrap();
        assert_eq!(storage.buffer.size(), 16);
        let wgpu::BindingResource::Buffer(binding) = storage.binding() else { panic!("buffer binding") };
        assert_eq!(binding.size.unwrap().get(), 4, "old trailing data is outside the shader-visible binding");
        assert_eq!(renderer.draw_upload_stats().1, 1);
        renderer.wait(renderer.submit([])).unwrap();
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn retained_draw_allocations_obey_entry_byte_and_oversized_limits() {
        let renderer = renderer();
        let mut leases = Vec::new();
        for _ in 0..MAX_ENTRIES + 3 {
            renderer.upload_draw_buffer(&mut leases, "tiny", &[0; 4], wgpu::BufferUsages::STORAGE).unwrap();
        }
        drop(leases);
        assert_eq!(renderer.draw_upload_stats().2, MAX_ENTRIES);
        let bytes = vec![0; MAX_ENTRY_BYTES as usize];
        let mut leases = Vec::new();
        for _ in 0..6 {
            renderer.upload_draw_buffer(&mut leases, "large", &bytes, wgpu::BufferUsages::STORAGE).unwrap();
        }
        drop(leases);
        let (_, _, retained, size) = renderer.draw_upload_stats();
        assert_eq!(retained, 4);
        assert_eq!(size, MAX_BYTES);
        let mut oversized = Vec::new();
        renderer
            .upload_draw_buffer(&mut oversized, "oversized", &vec![0; MAX_ENTRY_BYTES as usize + 4], wgpu::BufferUsages::STORAGE)
            .unwrap();
        drop(oversized);
        assert_eq!(renderer.draw_upload_stats().2, retained);
        assert_eq!(renderer.draw_upload_stats().3, size);
        renderer.wait(renderer.submit([])).unwrap();
    }
}
