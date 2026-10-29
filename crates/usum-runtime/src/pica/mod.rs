//! The PICA200 GPU's register state, driven by GX ProcessCommandList.
//!
//! Command lists (3dbrew "GPU/Internal Registers"): a parameter word,
//! then a header (register ID in bits 0-15, byte mask 16-19, extra
//! parameters 20-27, consecutive writes bit 31), the extra parameters,
//! padding to 8 bytes. A write to GPUREG_FINALIZE (0x0010) ends the list
//! and raises P3D.
//!
//! Draws are rendered in software (`draw`), the vertex shader is
//! interpreted (`shader`).

pub mod draw;
pub mod lighting;

/// `[f(0), f(1), f(2), f(3)]`: what `std::array::from_fn` builds, in the
/// same order, but always inlined. The standard helpers are not inlined
/// on the hot paths here (R-044).
#[inline(always)]
pub(crate) fn arr4<T>(mut f: impl FnMut(usize) -> T) -> [T; 4] {
    [f(0), f(1), f(2), f(3)]
}
pub mod shader;
pub mod texture;
#[cfg(feature = "vulkan-compare")]
pub mod vulkan_compare;

use usum_guest::{Fault, Memory};

pub const REGISTERS: usize = 0x300;
pub const FINALIZE: u16 = 0x0010;
pub const DRAW_ARRAYS: u16 = 0x022E;
pub const DRAW_ELEMENTS: u16 = 0x022F;

#[derive(Debug, Clone)]
pub struct Pica {
    pub regs: Vec<u32>,
    /// The vertex shader unit, and the geometry shader unit.
    pub vs: shader::Unit,
    pub gs: shader::Unit,
    /// Fixed vertex attributes, and the FIXEDATTRIB upload state.
    pub fixed: [shader::Vec4; 12],
    /// The fragment lighting lookup tables (3dbrew GPUREG_LIGHTING_LUT_*):
    /// 24 tables of 256 entries, each a 12-bit value and a 12-bit
    /// difference to the next.
    pub light_luts: std::sync::Arc<Vec<[u32; 256]>>,
    fixed_index: u32,
    fixed_pending: Vec<u32>,
    /// Where the process sees FCRAM physical 0x20000000 (its LINEAR base).
    pub linear_base: u32,
    /// Draw calls (DRAWARRAYS / DRAWELEMENTS writes).
    pub draws: u64,
    /// Command lists run, and register writes applied.
    pub lists: u64,
    pub writes: u64,
    pub triangles: u64,
    pub fragments: u64,
    /// Shaded vertices with w > 0, and the rest (a diagnostic).
    pub vertices_positive_w: u64,
    pub vertices_other_w: u64,
    /// Decoded textures kept across draws.
    pub texture_cache: texture::Cache,
    /// Command buffer jumps taken.
    pub jumps: u64,
    /// Host time spent drawing, and in rasterization (performance).
    pub draw_time: std::time::Duration,
    /// Time in command lists in all (draws included).
    pub list_time: std::time::Duration,
    pub raster_time: std::time::Duration,
    /// Host time spent copying/converting display-transfer framebuffers.
    pub display_transfer_time: std::time::Duration,
    pub display_transfers: u64,
    /// Time by phase: vertices (indices, loading, shading), buffers copied
    /// out of guest memory, textures prepared, bands rasterized, buffers
    /// copied back; then the vertex phase's parts: indices and
    /// deduplication, reserved (legacy separate loading), fused loading/shading.
    pub phases: [std::time::Duration; 8],
    /// Explicitly budgeted immutable draw captures for Vulkan comparison.
    /// Zero by default; capture never replaces or changes software rendering.
    #[cfg(feature = "vulkan-compare")]
    vulkan_capture_skip: usize,
    #[cfg(feature = "vulkan-compare")]
    vulkan_capture_remaining: usize,
    #[cfg(feature = "vulkan-compare")]
    vulkan_snapshots: Vec<vulkan_compare::DrawSnapshot>,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_depth: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_accelerate: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_cpu_depth_feedback: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_skip_software: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_vertex: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_defer_readback: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_split_encode: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_uniform_depth: bool,
    #[cfg(feature = "vulkan-compare")]
    vulkan_resolution_scale: u32,
    #[cfg(feature = "vulkan-compare")]
    scaled_displays: Vec<vulkan_compare::scale::Display>,
    #[cfg(feature = "vulkan-compare")]
    vulkan_vertex_counts: [u64; 4],
    #[cfg(feature = "vulkan-compare")]
    vulkan_vertex_error: Option<String>,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_lists: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_draws: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_targets: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_feedback_reads: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_feedback_kinds: [u64; 4],
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_feedback_depth_pairs: [u64; 56],
    #[cfg(feature = "vulkan-compare")]
    vulkan_accelerated_lists: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_acceleration_fallbacks: u64,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_time: std::time::Duration,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_target_cache: Vec<vulkan_compare::LiveTarget>,
    #[cfg(feature = "vulkan-compare")]
    vulkan_live_resources: vulkan_compare::LiveResources,
}

impl Default for Pica {
    fn default() -> Pica {
        Pica {
            regs: vec![0; REGISTERS],
            vs: shader::Unit::default(),
            gs: shader::Unit::default(),
            fixed: [[0.0, 0.0, 0.0, 1.0]; 12],
            light_luts: std::sync::Arc::new(vec![[0; 256]; 24]),
            fixed_index: 0,
            fixed_pending: Vec::new(),
            linear_base: 0x3000_0000,
            draws: 0,
            lists: 0,
            writes: 0,
            triangles: 0,
            fragments: 0,
            vertices_positive_w: 0,
            vertices_other_w: 0,
            texture_cache: texture::Cache::default(),
            jumps: 0,
            draw_time: std::time::Duration::ZERO,
            list_time: std::time::Duration::ZERO,
            raster_time: std::time::Duration::ZERO,
            display_transfer_time: std::time::Duration::ZERO,
            display_transfers: 0,
            phases: [std::time::Duration::ZERO; 8],
            #[cfg(feature = "vulkan-compare")]
            vulkan_capture_skip: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_capture_remaining: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_snapshots: Vec::new(),
            #[cfg(feature = "vulkan-compare")]
            vulkan_live: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_depth: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_accelerate: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_cpu_depth_feedback: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_skip_software: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_vertex: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_defer_readback: false,
            #[cfg(feature = "vulkan-compare")]
            vulkan_split_encode: true,
            #[cfg(feature = "vulkan-compare")]
            vulkan_uniform_depth: std::env::var("USUM_VULKAN_UNIFORM_DEPTH").as_deref() != Ok("0"),
            #[cfg(feature = "vulkan-compare")]
            vulkan_resolution_scale: 1,
            #[cfg(feature = "vulkan-compare")]
            scaled_displays: Vec::new(),
            #[cfg(feature = "vulkan-compare")]
            vulkan_vertex_counts: [0; 4],
            #[cfg(feature = "vulkan-compare")]
            vulkan_vertex_error: None,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_lists: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_draws: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_targets: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_feedback_reads: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_feedback_kinds: [0; 4],
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_feedback_depth_pairs: [0; 56],
            #[cfg(feature = "vulkan-compare")]
            vulkan_accelerated_lists: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_acceleration_fallbacks: 0,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_time: std::time::Duration::ZERO,
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_target_cache: Vec::new(),
            #[cfg(feature = "vulkan-compare")]
            vulkan_live_resources: vulkan_compare::LiveResources::default(),
        }
    }
}

/// Byte mask bits to a word mask: bit k keeps byte k writable.
fn word_mask(mask: u32) -> u32 {
    (0..4).filter(|k| mask & (1 << k) != 0).map(|k| 0xFF << (8 * k)).fold(0, |a, b| a | b)
}

impl Pica {
    /// A new guest process cannot inherit register/upload state or GPU views of
    /// its predecessor's RAM. Keep host policy and cumulative profiling counts.
    pub(crate) fn restart_process(&mut self) {
        self.regs.fill(0);
        self.vs = shader::Unit::default();
        self.gs = shader::Unit::default();
        self.fixed = [[0.0, 0.0, 0.0, 1.0]; 12];
        self.light_luts = std::sync::Arc::new(vec![[0; 256]; 24]);
        self.fixed_index = 0;
        self.fixed_pending.clear();
        self.texture_cache = texture::Cache::default();
        #[cfg(feature = "vulkan-compare")]
        {
            self.vulkan_snapshots.clear();
            self.vulkan_live_target_cache.clear();
            self.scaled_displays.clear();
            self.vulkan_live_resources.invalidate_process();
            self.vulkan_skip_software = false;
        }
    }

    /// Captures at most `draws` subsequent post-geometry triangle lists for
    /// Vulkan comparison while the software renderer continues normally.
    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_capture(&mut self, draws: usize) {
        self.enable_vulkan_capture_after(0, draws);
    }

    /// Skips `skip` non-empty draws, then captures at most `draws` draws.
    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_capture_after(&mut self, skip: usize, draws: usize) {
        self.vulkan_capture_skip = skip;
        self.vulkan_capture_remaining = draws;
        self.vulkan_snapshots.clear();
    }

    /// Removes and returns all completed immutable Vulkan draw snapshots.
    #[cfg(feature = "vulkan-compare")]
    pub fn take_vulkan_snapshots(&mut self) -> Vec<vulkan_compare::DrawSnapshot> {
        std::mem::take(&mut self.vulkan_snapshots)
    }

    /// Enables the experimental command-list Vulkan color path. Software
    /// rendering and depth remain active as the reference and fallback.
    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_live(&mut self) {
        self.vulkan_live = true;
        self.vulkan_live_target_cache.clear();
        self.scaled_displays.clear();
        self.vulkan_live_resources.clear();
    }

    /// Enables live Vulkan color and depth writeback. This is kept separate
    /// while depth comparison evidence is expanded beyond startup draws.
    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_live_depth(&mut self) {
        self.enable_vulkan_live();
        self.vulkan_live_depth = true;
    }

    /// Enables guarded Vulkan-only rasterization. Command-list state is
    /// checkpointed, and any dependency or GPU error reruns the list through
    /// the software renderer.
    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_acceleration(&mut self) {
        self.enable_vulkan_live_depth();
        self.vulkan_accelerate = true;
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_cpu_depth_feedback(&mut self) {
        self.vulkan_cpu_depth_feedback = true;
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn set_vulkan_resolution_scale(&mut self, scale: u32) -> Result<(), String> {
        if !(1..=8).contains(&scale) {
            return Err("resolution scale must be between 1 and 8".into());
        }
        if self.vulkan_resolution_scale != scale {
            self.vulkan_resolution_scale = scale;
            self.vulkan_live_target_cache.clear();
            self.scaled_displays.clear();
        }
        Ok(())
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_deferred_readback(&mut self) {
        self.vulkan_defer_readback = true;
        self.vulkan_split_encode = std::env::var("USUM_VULKAN_SPLIT_ENCODE").as_deref() != Ok("0");
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn enable_vulkan_vertex(&mut self) {
        self.vulkan_vertex = true;
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_vertex_stats(&self) -> ([u64; 4], Option<&str>) {
        (self.vulkan_vertex_counts, self.vulkan_vertex_error.as_deref())
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_live_stats(&self) -> (u64, u64, u64, std::time::Duration) {
        (self.vulkan_live_lists, self.vulkan_live_draws, self.vulkan_live_targets, self.vulkan_live_time)
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_target_stats(&self) -> (u64, u64) {
        self.vulkan_live_resources.target_stats()
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_capture_times(&self) -> [std::time::Duration; 3] {
        self.vulkan_live_resources.capture_times()
    }

    #[cfg(feature = "vulkan-compare")]
    pub(crate) fn capture_scaled_transfer(
        &mut self,
        addresses: (u32, u32),
        dims: ((u32, u32), (u32, u32)),
        flags: u32,
        input: &[u8],
        output: &[u8],
    ) -> Result<(), String> {
        vulkan_compare::scale::capture_transfer(self, addresses.0, addresses.1, dims, flags, input, output)
    }
    #[cfg(feature = "vulkan-compare")]
    pub(crate) fn scaled_frame(
        &self,
        p: &crate::kernel::Present,
        columns: u32,
        raw: &crate::probe::RawFrame,
    ) -> Option<crate::probe::ScaledFrame> {
        vulkan_compare::scale::presented(self, p, columns, raw)
    }
    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_frozen_texture_captures(&self) -> u64 {
        self.vulkan_live_resources.frozen_texture_captures
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_deferred_stats(&self) -> (u64, u64, std::time::Duration) {
        self.vulkan_live_resources.deferred_stats()
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_uniform_depth_imports(&self) -> u64 {
        self.vulkan_live_resources.uniform_depth_imports()
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_transfer_times(&self) -> [std::time::Duration; 2] {
        self.vulkan_live_resources.transfer_times()
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_live_resource_stats(&self) -> (u64, u64, u64, u64) {
        self.vulkan_live_resources.stats()
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_live_feedback_reads(&self) -> u64 {
        self.vulkan_live_feedback_reads
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_live_feedback_kinds(&self) -> [u64; 4] {
        self.vulkan_live_feedback_kinds
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_live_feedback_depth_pairs(&self) -> [u64; 56] {
        self.vulkan_live_feedback_depth_pairs
    }

    #[cfg(feature = "vulkan-compare")]
    fn record_vulkan_feedback(&mut self, feedback: vulkan_compare::FeedbackStats) {
        self.vulkan_live_feedback_reads += feedback.reads as u64;
        self.vulkan_live_feedback_kinds[0] += feedback.compatible_color as u64;
        self.vulkan_live_feedback_kinds[1] += feedback.other_color as u64;
        self.vulkan_live_feedback_kinds[2] += feedback.depth as u64;
        self.vulkan_live_feedback_kinds[3] += feedback.partial as u64;
        for (total, count) in self.vulkan_live_feedback_depth_pairs.iter_mut().zip(feedback.depth_pairs) {
            *total += count as u64;
        }
    }

    #[cfg(feature = "vulkan-compare")]
    pub fn vulkan_acceleration_stats(&self) -> (u64, u64) {
        (self.vulkan_accelerated_lists, self.vulkan_acceleration_fallbacks)
    }

    pub fn with_linear_base(linear_base: u32) -> Pica {
        Pica { linear_base, ..Pica::default() }
    }

    /// A physical address the GPU uses, as the process's virtual address:
    /// VRAM (0x18000000) at 0x1F000000, FCRAM (0x20000000) at the LINEAR
    /// base (3dbrew "Memory layout").
    pub fn pa_to_va(&self, pa: u32) -> Result<u32, String> {
        match pa {
            0x1800_0000..=0x185F_FFFF => Ok(pa - 0x1800_0000 + 0x1F00_0000),
            0x2000_0000..=0x27FF_FFFF => Ok(pa - 0x2000_0000 + self.linear_base),
            _ => Err(format!("GPU physical address {pa:#x} is neither VRAM nor FCRAM")),
        }
    }

    fn write(&mut self, mem: &mut Memory, reg: u16, value: u32, mask: u32) -> Result<bool, String> {
        let r = self.regs.get_mut(usize::from(reg)).ok_or_else(|| format!("GPU register {reg:#x} out of range"))?;
        let m = word_mask(mask);
        *r = (*r & !m) | (value & m);
        self.writes += 1;
        match reg {
            0x280..=0x2AF => self.gs.write(reg - 0x280, value),
            0x2B0..=0x2DF => {
                self.vs.write(reg - 0x2B0, value);
                // VSH_COM_MODE 0: the vertex shader configuration reaches
                // the geometry unit too (3dbrew GPUREG_VSH_COM_MODE)
                if self.regs[0x244] & 1 == 0 {
                    self.gs.write(reg - 0x2B0, value);
                }
            }
            // GPUREG_LIGHTING_LUT_DATA0-7: an entry at LUT_INDEX's table and
            // index, which then advances
            0x1C8..=0x1CF => {
                let at = self.regs[0x1C5];
                let (table, index) = (((at >> 8) & 0x1F) as usize, (at & 0xFF) as usize);
                if let Some(lut) = std::sync::Arc::make_mut(&mut self.light_luts).get_mut(table) {
                    lut[index] = value & 0xFF_FFFF;
                }
                self.regs[0x1C5] = (at & !0xFF) | ((at + 1) & 0xFF);
            }
            0x232 => {
                self.fixed_index = value & 0xF;
                self.fixed_pending.clear();
            }
            0x233..=0x235 => {
                self.fixed_pending.push(value);
                if self.fixed_pending.len() == 3 {
                    let v = shader::unpack_f24x4([self.fixed_pending[0], self.fixed_pending[1], self.fixed_pending[2]]);
                    self.fixed_pending.clear();
                    match self.fixed.get_mut(self.fixed_index as usize) {
                        Some(slot) => *slot = v,
                        None => return Err("immediate-mode vertex submission is not implemented".into()),
                    }
                    self.fixed_index += 1;
                }
            }
            DRAW_ARRAYS | DRAW_ELEMENTS if value != 0 => {
                self.draws += 1;
                draw::draw(self, mem, reg == DRAW_ELEMENTS)?;
            }
            _ => {}
        }
        Ok(reg == FINALIZE)
    }

    /// Runs the command list of `size` bytes at `address`. Returns whether
    /// it finalized (and so raises P3D).
    pub fn run(&mut self, mem: &mut Memory, address: u32, size: u32) -> Result<bool, String> {
        #[cfg(feature = "vulkan-compare")]
        if self.vulkan_live && self.vulkan_accelerate {
            return self.run_accelerated(mem, address, size);
        }
        #[cfg(feature = "vulkan-compare")]
        if self.vulkan_live {
            self.enable_vulkan_capture(usize::MAX);
        }
        let started = std::time::Instant::now();
        let result = self.run_list(mem, address, size);
        #[cfg(feature = "vulkan-compare")]
        if self.vulkan_live && result.is_ok() {
            let live_started = std::time::Instant::now();
            match vulkan_compare::replay_live(self, mem) {
                Ok((draws, targets, feedback)) => {
                    self.vulkan_live_lists += 1;
                    self.vulkan_live_draws += draws as u64;
                    self.vulkan_live_targets += targets as u64;
                    self.record_vulkan_feedback(feedback);
                    self.vulkan_live_time += live_started.elapsed();
                }
                Err(error) => {
                    eprintln!("Vulkan live mode disabled; software fallback remains active: {error}");
                    self.vulkan_live = false;
                    self.vulkan_snapshots.clear();
                    self.vulkan_live_target_cache.clear();
                    self.scaled_displays.clear();
                    self.vulkan_live_resources.clear();
                }
            }
        }
        self.list_time += started.elapsed();
        result
    }

    #[cfg(feature = "vulkan-compare")]
    fn acceleration_checkpoint(&mut self) -> Self {
        // These caches are host resources, not guest register state. Cloning
        // hundreds of texture/lighting entries on every command list adds
        // allocations and Arc traffic even for lists with no draws. Keep them
        // outside rollback; each live target is validated against guest memory
        // on its first use in a list, including after a software fallback.
        let texture_cache = std::mem::take(&mut self.texture_cache);
        let live_targets = std::mem::take(&mut self.vulkan_live_target_cache);
        let live_resources = std::mem::take(&mut self.vulkan_live_resources);
        let checkpoint = self.clone();
        self.texture_cache = texture_cache;
        self.vulkan_live_target_cache = live_targets;
        self.vulkan_live_resources = live_resources;
        checkpoint
    }

    #[cfg(feature = "vulkan-compare")]
    fn run_accelerated(&mut self, mem: &mut Memory, address: u32, size: u32) -> Result<bool, String> {
        self.enable_vulkan_capture(usize::MAX);
        let started = std::time::Instant::now();
        let checkpoint = self.acceleration_checkpoint();
        self.vulkan_skip_software = true;
        let result = self.run_list(mem, address, size);
        self.vulkan_skip_software = false;
        if result.is_err() {
            self.list_time += started.elapsed();
            return result;
        }

        let feedback = match vulkan_compare::pending_feedback_reads(self) {
            Ok(reads) => reads,
            Err(error) => {
                eprintln!("Vulkan acceleration disabled; software fallback remains active: {error}");
                return self.rerun_accelerated_list(
                    mem,
                    (address, size),
                    checkpoint,
                    started,
                    (true, vulkan_compare::FeedbackStats::default()),
                );
            }
        };
        if feedback.reads != 0 && !(self.vulkan_cpu_depth_feedback && vulkan_compare::pending_feedback_supported(self)?) {
            return self.rerun_accelerated_list(mem, (address, size), checkpoint, started, (false, feedback));
        }

        let live_started = std::time::Instant::now();
        match vulkan_compare::replay_live(self, mem) {
            Ok((draws, targets, feedback)) => {
                self.vulkan_live_lists += 1;
                self.vulkan_live_draws += draws as u64;
                self.vulkan_live_targets += targets as u64;
                self.record_vulkan_feedback(feedback);
                self.vulkan_live_time += live_started.elapsed();
                self.vulkan_accelerated_lists += 1;
                self.list_time += started.elapsed();
                result
            }
            Err(error) => {
                eprintln!("Vulkan acceleration disabled; rerunning the list in software: {error}");
                self.rerun_accelerated_list(
                    mem,
                    (address, size),
                    checkpoint,
                    started,
                    (true, vulkan_compare::FeedbackStats::default()),
                )
            }
        }
    }

    #[cfg(feature = "vulkan-compare")]
    fn rerun_accelerated_list(
        &mut self,
        mem: &mut Memory,
        command: (u32, u32),
        mut checkpoint: Pica,
        started: std::time::Instant,
        fallback: (bool, vulkan_compare::FeedbackStats),
    ) -> Result<bool, String> {
        let ((address, size), (disable, feedback)) = (command, fallback);
        checkpoint.texture_cache = std::mem::take(&mut self.texture_cache);
        checkpoint.vulkan_live_target_cache = std::mem::take(&mut self.vulkan_live_target_cache);
        checkpoint.vulkan_live_resources = std::mem::take(&mut self.vulkan_live_resources);
        *self = checkpoint;
        self.vulkan_skip_software = false;
        self.vulkan_capture_remaining = 0;
        self.vulkan_snapshots.clear();
        self.vulkan_acceleration_fallbacks += 1;
        self.record_vulkan_feedback(feedback);
        if disable {
            self.vulkan_live = false;
            self.vulkan_accelerate = false;
            self.vulkan_live_target_cache.clear();
            self.scaled_displays.clear();
            self.vulkan_live_resources.clear();
        }
        let result = self.run_list(mem, address, size);
        self.list_time += started.elapsed();
        result
    }

    fn run_list(&mut self, mem: &mut Memory, address: u32, size: u32) -> Result<bool, String> {
        let fault = |f: Fault| format!("GPU command list: {f}");
        let mut end = address.checked_add(size).ok_or("GPU command list wraps")?;
        let mut at = address;
        let mut hops = 0u32;
        self.lists += 1;
        'commands: while at + 8 <= end {
            let first = mem.read_u32(at).map_err(fault)?;
            let header = mem.read_u32(at + 4).map_err(fault)?;
            let (mut reg, mask) = ((header & 0xFFFF) as u16, (header >> 16) & 0xF);
            let extra = (header >> 20) & 0xFF;
            let consecutive = header & (1 << 31) != 0;
            // GPUREG_CMDBUF_JUMP0/1 (3dbrew): continue in command buffer 0/1
            // (address and size from GPUREG_CMDBUF_ADDR/SIZE, >> 3)
            let jump = |p: &Pica, reg: u16, value: u32| -> Result<Option<(u32, u32)>, String> {
                if !matches!(reg, 0x23C | 0x23D) || value == 0 {
                    return Ok(None);
                }
                let k = usize::from(reg - 0x23C);
                let to = p.pa_to_va((p.regs[0x23A + k] & 0x1FFF_FFFF) << 3)?;
                Ok(Some((to, (p.regs[0x238 + k] & 0x1F_FFFF) << 3)))
            };
            if self.write(mem, reg, first, mask)? {
                return Ok(true);
            }
            if let Some((to, len)) = jump(self, reg, first)? {
                hops += 1;
                if hops > 4096 {
                    return Err("GPU command buffers jump in a loop".into());
                }
                self.jumps += 1;
                (at, end) = (to, to + len);
                continue 'commands;
            }
            for k in 0..extra {
                let value = mem.read_u32(at + 8 + 4 * k).map_err(fault)?;
                if consecutive {
                    reg += 1;
                }
                if self.write(mem, reg, value, mask)? {
                    return Ok(true);
                }
                if let Some((to, len)) = jump(self, reg, value)? {
                    hops += 1;
                    if hops > 4096 {
                        return Err("GPU command buffers jump in a loop".into());
                    }
                    self.jumps += 1;
                    (at, end) = (to, to + len);
                    continue 'commands;
                }
            }
            // the parameter, the header, the extras, padded to 8 bytes
            at += 8 + (extra + (extra & 1)) * 4;
        }
        Ok(false)
    }
}

/// Integer internal rendering multiplier; native is the default.
pub fn resolution_scale_env() -> Result<u32, String> {
    match std::env::var("USUM_RESOLUTION_SCALE") {
        Ok(value) => value
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=8).contains(n))
            .ok_or_else(|| "USUM_RESOLUTION_SCALE must be an integer from 1 to 8".into()),
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(_) => Err("USUM_RESOLUTION_SCALE is not valid text".into()),
    }
}

/// Pixel formats of the transfer engine and framebuffers (3dbrew
/// "GPU/External Registers": 0 RGBA8, 1 RGB8, 2 RGB565, 3 RGB5A1,
/// 4 RGBA4; components in reverse byte order).
pub fn bytes_per_pixel(format: u32) -> Option<usize> {
    match format {
        0 => Some(4),
        1 => Some(3),
        2..=4 => Some(2),
        _ => None,
    }
}

/// One pixel in `format` from its bytes, as (r, g, b, a).
#[inline]
pub fn decode_pixel(format: u32, b: &[u8]) -> [u8; 4] {
    let x5 = |v: u16| ((v << 3) | (v >> 2)) as u8;
    let x6 = |v: u16| ((v << 2) | (v >> 4)) as u8;
    let x4 = |v: u16| (v * 0x11) as u8;
    let h = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    match format {
        0 => [b[3], b[2], b[1], b[0]],
        1 => [b[2], b[1], b[0], 0xFF],
        2 => {
            let v = h(b);
            [x5(v >> 11), x6((v >> 5) & 0x3F), x5(v & 0x1F), 0xFF]
        }
        3 => {
            let v = h(b);
            [x5(v >> 11), x5((v >> 6) & 0x1F), x5((v >> 1) & 0x1F), if v & 1 != 0 { 0xFF } else { 0 }]
        }
        _ => {
            let v = h(b);
            [x4(v >> 12), x4((v >> 8) & 0xF), x4((v >> 4) & 0xF), x4(v & 0xF)]
        }
    }
}

/// One encoded pixel: its bytes, as many as the format uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoded {
    bytes: [u8; 4],
    len: usize,
}

impl std::ops::Deref for Encoded {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// One pixel (r, g, b, a) in `format` (no allocation: this runs per
/// fragment).
#[inline]
pub fn encode_pixel(format: u32, [r, g, b, a]: [u8; 4]) -> Encoded {
    let (r, g, b, a) = (u16::from(r), u16::from(g), u16::from(b), u16::from(a));
    let half = |v: u16| {
        let [lo, hi] = v.to_le_bytes();
        Encoded { bytes: [lo, hi, 0, 0], len: 2 }
    };
    match format {
        0 => Encoded { bytes: [a as u8, b as u8, g as u8, r as u8], len: 4 },
        1 => Encoded { bytes: [b as u8, g as u8, r as u8, 0], len: 3 },
        2 => half((r >> 3) << 11 | (g >> 2) << 5 | b >> 3),
        3 => half((r >> 3) << 11 | (g >> 3) << 6 | (b >> 3) << 1 | a >> 7),
        _ => half((r >> 4) << 12 | (g >> 4) << 8 | (b >> 4) << 4 | a >> 4),
    }
}

/// The offset in pixels of (x, y) in a buffer of 8x8 tiles, `width`
/// pixels wide: tiles row by row, pixels inside a tile in Z order
/// (x and y bits interleaved, x lowest).
pub fn tiled_offset(x: u32, y: u32, width: u32) -> u32 {
    let (tx, ty) = (x / 8, y / 8);
    let (px, py) = (x % 8, y % 8);
    let z = (px & 1) | (py & 1) << 1 | (px & 2) << 1 | (py & 2) << 2 | (px & 4) << 2 | (py & 4) << 3;
    (ty * (width / 8) + tx) * 64 + z
}

/// Required input prefix for the common unfiltered tiled RGBA8 LCD copy.
/// Reading the advertised padded height can cross a framebuffer boundary and
/// materialize unrelated GPU results. Other transfer modes retain their full
/// input extent. `display_transfer` still validates flags and sampled bytes.
pub fn display_transfer_input_len((in_w, in_h): (u32, u32), (out_w, out_h): (u32, u32), flags: u32) -> Result<u32, String> {
    let bytes = bytes_per_pixel((flags >> 8) & 7).ok_or("DisplayTransfer input format")? as u32;
    let full = in_w.checked_mul(in_h).and_then(|n| n.checked_mul(bytes)).ok_or("DisplayTransfer input size overflow")?;
    if flags & !5 != 0x1000 || in_w == 0 || !in_w.is_multiple_of(8) {
        return Ok(full);
    }
    let (width, height) = (out_w.min(in_w), out_h.min(in_h));
    if width == 0 || height == 0 {
        return Ok(0);
    }
    // Morton offsets increase in each coordinate for an aligned tile stride.
    // An incomplete advertised final tile must retain the original short-input
    // error rather than reading additional memory beyond the old extent.
    let end = tiled_offset(width - 1, height - 1, in_w).checked_add(1).and_then(|n| n.checked_mul(4));
    Ok(end.unwrap_or(full).min(full))
}

/// A DisplayTransfer (3dbrew "GPU/External Registers", transfer engine).
/// The input is `in_w` × `in_h` pixels of 8x8 tiles. The output is linear
/// rows (the default mode) or 8x8 tiles again (flag bit 5), cropped from
/// the input's top-left when narrower (flag bit 2). Flag bit 0 flips
/// vertically; bits 24-25 shrink the input with a box filter, 2x1 or 2x2.
///
/// With a downscale the output holds `in / scale` pixels per axis, at
/// most the output dimensions given (3dbrew: for tiled-to-linear the two
/// sizes "should be the same"; for tiled-to-tiled only 2x2 is used
/// officially). Box averages round to nearest (a choice; the hardware's
/// rounding is not documented). Returns the output bytes.
pub fn display_transfer(
    input: &[u8],
    (in_w, in_h): (u32, u32),
    (out_w, out_h): (u32, u32),
    flags: u32,
) -> Result<Vec<u8>, String> {
    if flags & 0b1010 != 0 || flags & (1 << 16) != 0 {
        return Err(format!("DisplayTransfer flags {flags:#x}: linear-to-tiled and 32x32 tiles are not implemented"));
    }
    let tiled = flags & (1 << 5) != 0;
    let (sx, sy) = match (flags >> 24) & 3 {
        0 => (1, 1),
        1 => (2, 1),
        2 => (2, 2),
        _ => return Err(format!("DisplayTransfer flags {flags:#x}: invalid scaling")),
    };
    let (out_w, out_h) = (out_w.min(in_w / sx), out_h.min(in_h / sy));
    if out_w < in_w / sx && flags & 4 == 0 {
        return Err(format!("DisplayTransfer {in_w}x{in_h} -> {out_w}x{out_h} with flags {flags:#x}"));
    }
    if tiled && (out_w % 8 != 0 || out_h % 8 != 0) {
        return Err(format!("DisplayTransfer to tiles of {out_w}x{out_h}"));
    }
    let (fin, fout) = ((flags >> 8) & 7, (flags >> 12) & 7);
    let (bin, bout) =
        bytes_per_pixel(fin).zip(bytes_per_pixel(fout)).ok_or_else(|| format!("DisplayTransfer formats {fin}, {fout}"))?;
    // Common LCD presentation: tiled ABGR8 -> linear BGR8, without a
    // downscale. Copy the three stored color bytes directly; the general
    // path below still handles filtering, tiled output and other formats.
    if !tiled && sx == 1 && sy == 1 && fin == 0 && fout == 1 {
        let mut out = vec![0; (out_w * out_h) as usize * 3];
        if out_w == 0 {
            return Ok(out);
        }
        for (y, destination) in out.chunks_exact_mut(out_w as usize * 3).enumerate() {
            let row = if flags & 1 != 0 { out_h - 1 - y as u32 } else { y as u32 };
            for (x, pixel) in destination.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let at = tiled_offset(x as u32, row, in_w) as usize * 4;
                pixel.copy_from_slice(input.get(at + 1..at + 4).ok_or("DisplayTransfer reads past its input")?);
            }
        }
        return Ok(out);
    }
    let read = |x: u32, y: u32| -> Result<[u8; 4], String> {
        let at = tiled_offset(x, y, in_w) as usize * bin;
        Ok(decode_pixel(fin, input.get(at..at + bin).ok_or("DisplayTransfer reads past its input")?))
    };
    let n = sx * sy;
    let mut out = vec![0; (out_w * out_h) as usize * bout];
    for y in 0..out_h {
        let row = if flags & 1 != 0 { out_h - 1 - y } else { y };
        for x in 0..out_w {
            let mut sum = [0u32; 4];
            for (dx, dy) in (0..sy).flat_map(|dy| (0..sx).map(move |dx| (dx, dy))) {
                let px = read(x * sx + dx, row * sy + dy)?;
                sum.iter_mut().zip(px).for_each(|(s, c)| *s += u32::from(c));
            }
            let px = sum.map(|s| ((s + n / 2) / n) as u8);
            let at = if tiled { tiled_offset(x, y, out_w) } else { y * out_w + x } as usize * bout;
            out[at..at + bout].copy_from_slice(&encode_pixel(fout, px));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_restart_resets_gpu_state_and_keeps_host_policy_and_counters() {
        let mut pica = Pica::default();
        pica.regs[0x41] = 0xabcd;
        pica.fixed[0] = [3.0; 4];
        pica.fixed_index = 9;
        pica.fixed_pending.push(7);
        std::sync::Arc::make_mut(&mut pica.light_luts)[0][4] = 0x123;
        pica.draws = 58;
        #[cfg(feature = "vulkan-compare")]
        {
            pica.enable_vulkan_acceleration();
            pica.enable_vulkan_deferred_readback();
            pica.enable_vulkan_vertex();
            pica.vulkan_uniform_depth = false;
            pica.set_vulkan_resolution_scale(6).unwrap();
        }
        pica.restart_process();
        assert_eq!(pica.regs[0x41], 0);
        assert_eq!(pica.fixed[0], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(pica.fixed_index, 0);
        assert!(pica.fixed_pending.is_empty());
        assert_eq!(pica.light_luts[0][4], 0);
        assert_eq!(pica.draws, 58);
        #[cfg(feature = "vulkan-compare")]
        {
            assert!(pica.vulkan_accelerate && pica.vulkan_vertex && pica.vulkan_defer_readback);
            assert!(!pica.vulkan_uniform_depth);
            assert_eq!(pica.vulkan_resolution_scale, 6);
        }
    }

    use usum_guest::memory::Perm;

    #[test]
    fn direct_display_conversion_matches_generic_cropping_and_flip() {
        for (width, height) in [(8, 8), (16, 24), (256, 512)] {
            let input: Vec<u8> = (0..width * height * 4).map(|i| (i * 73 + i / 19) as u8).collect();
            for output in [(width, height), (width - 3, height - 2), (0, height), (width, 0)] {
                for flip in [0, 1] {
                    let flags = flip | 4;
                    // RGBA8 output uses the generic implementation. Convert its
                    // pixels to RGB8 as an independent oracle for the fast path.
                    let generic = display_transfer(&input, (width, height), output, flags).unwrap();
                    let expected: Vec<u8> = generic
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .flat_map(|pixel| encode_pixel(1, decode_pixel(0, pixel)).to_vec())
                        .collect();
                    let direct = display_transfer(&input, (width, height), output, flags | 1 << 12).unwrap();
                    assert_eq!(direct, expected);
                    let length = display_transfer_input_len((width, height), output, flags | 1 << 12).unwrap() as usize;
                    assert_eq!(display_transfer(&input[..length], (width, height), output, flags | 1 << 12).unwrap(), expected);
                    if length > 0 {
                        assert!(display_transfer(&input[..length - 1], (width, height), output, flags | 1 << 12).is_err());
                    }
                }
            }
            let short = &input[..input.len() - 1];
            assert!(display_transfer(short, (width, height), (width, height), 1 << 12).is_err());
        }
    }

    #[test]
    fn lcd_reads_omit_unused_tiles_and_other_modes_keep_the_full_extent() {
        assert_eq!(display_transfer_input_len((256, 512), (240, 400), 0x1004).unwrap(), 409088);
        assert_eq!(display_transfer_input_len((256, 512), (240, 320), 0x1005).unwrap(), 327168);
        for flags in [0, 0x0100_1004, 0x1000 | 32] {
            assert_eq!(display_transfer_input_len((256, 512), (240, 320), flags).unwrap(), 256 * 512 * 4);
        }
        assert_eq!(display_transfer_input_len((7, 8), (3, 4), 0x1004).unwrap(), 7 * 8 * 4);
        assert!(display_transfer_input_len((u32::MAX, 2), (1, 1), 0x1004).is_err());
        assert!(display_transfer_input_len((8, 8), (8, 8), 0x1704).is_err());
    }

    #[test]
    fn pixel_formats_round_trip_where_exact() {
        for f in 0..5 {
            let px = [0xFF, 0x00, 0x88, 0xFF];
            let back = decode_pixel(f, &encode_pixel(f, px));
            assert_eq!((back[0], back[1]), (0xFF, 0x00), "format {f}");
        }
        assert_eq!(*encode_pixel(1, [1, 2, 3, 4]), [3, 2, 1], "RGB8 is stored B, G, R");
    }

    #[test]
    fn tiles_are_z_ordered_and_transfers_crop() {
        assert_eq!((tiled_offset(1, 0, 16), tiled_offset(0, 1, 16), tiled_offset(2, 0, 16)), (1, 2, 4));
        assert_eq!(tiled_offset(8, 0, 16), 64, "next tile");
        assert_eq!(tiled_offset(0, 8, 16), 128, "next tile row");
        // 16x8 RGB8 tiles whose pixels hold their own linear index; crop to 8x8
        let mut input = vec![0u8; 16 * 8 * 3];
        for y in 0..8 {
            for x in 0..16 {
                let at = tiled_offset(x, y, 16) as usize * 3;
                input[at] = (y * 16 + x) as u8;
            }
        }
        let out = display_transfer(&input, (16, 8), (8, 8), 0x1104).unwrap();
        assert_eq!(out.len(), 8 * 8 * 3);
        assert_eq!((out[0], out[3 * 7], out[3 * 8]), (0, 7, 16));
        assert!(display_transfer(&input, (16, 8), (8, 8), 0x1100).is_err(), "cropping needs bit 2");
    }

    #[test]
    fn transfers_downscale_into_tiles() {
        // 16x16 RGBA8 tiles, red = x, green = y; 2x2 box into 8x8 tiles
        let mut input = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let at = tiled_offset(x, y, 16) as usize * 4;
                input[at..at + 4].copy_from_slice(&encode_pixel(0, [x as u8 * 10, y as u8 * 10, 0, 255]));
            }
        }
        let out = display_transfer(&input, (16, 16), (16, 16), 0x0200_0020).unwrap();
        assert_eq!(out.len(), 8 * 8 * 4, "the output is half of the input per axis");
        let at = tiled_offset(3, 5, 8) as usize * 4;
        // x 6 and 7 average 65, y 10 and 11 average 105
        assert_eq!(decode_pixel(0, &out[at..at + 4]), [65, 105, 0, 255]);
        assert!(display_transfer(&input, (16, 16), (16, 16), 0x0300_0020).is_err());
    }

    #[test]
    fn consecutive_masked_writes_and_finalize() {
        let words = [
            // 3dbrew's example: writes 0x11C, 0x11D, 0x11E
            0xAAAA_AAAA,
            0x802F_011C,
            0xBBBB_BBBB,
            0xCCCC_CCCC,
            // byte mask 0b0011 on 0x117: only the low half changes
            0x1234_5678,
            0x0003_0117,
            // a draw, then finalize, then something never run
            0x0000_0001,
            0x000F_022E,
            0x1234_5678,
            0x000F_0010,
            0xFFFF_FFFF,
            0x000F_0117,
        ];
        let mut mem = Memory::new();
        mem.map(0x1000, words.iter().flat_map(|w: &u32| w.to_le_bytes()).collect(), Perm::R).unwrap();
        let mut p = Pica::default();
        p.regs[0x117] = 0xFFFF_0000;
        assert_eq!(p.run(&mut mem, 0x1000, 4 * words.len() as u32), Ok(true));
        assert_eq!((p.regs[0x11C], p.regs[0x11D], p.regs[0x11E]), (0xAAAA_AAAA, 0xBBBB_BBBB, 0xCCCC_CCCC));
        assert_eq!(p.regs[0x117], 0xFFFF_5678);
        assert_eq!(p.draws, 1);
    }
}
