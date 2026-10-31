//! Restart a title from pristine boot memory while retaining the host session.
use super::*;

pub(crate) struct BootImage {
    cpu: Cpu,
    memory: Memory,
    process: Process,
    blocks: Vec<Block>,
    services: Vec<String>,
}

impl BootImage {
    /// Called before the first translated instruction, not at jump preparation
    /// after the game has torn down parts of its process.
    pub(crate) fn capture(cpu: &Cpu, memory: &Memory, kernel: &Kernel) -> Option<Self> {
        let process = kernel.process?;
        Some(Self {
            cpu: cpu.clone(),
            memory: memory.clone(),
            process,
            blocks: kernel.blocks.clone(),
            services: kernel.services.clone(),
        })
    }

    pub(crate) fn restart(&self, cpu: &mut Cpu, memory: &mut Memory, kernel: &mut Kernel, args: apt::DeliverArg) {
        let mut next = Kernel::new(self.process, self.blocks.clone(), self.services.clone());
        // OS/session time and input recording are monotonic across process
        // lifetimes. Replaying earlier input on every reboot would loop forever.
        macro_rules! transfer {
            ($($field:ident),* $(,)?) => { $(next.$field = std::mem::take(&mut kernel.$field);)* };
        }
        transfer!(
            started,
            virtual_clock,
            native_timing,
            next_vblank,
            next_audio_frame,
            vblanks,
            audio_frames,
            slept,
            input,
            live_pad,
            live_touch,
            recorded,
            pad,
            frame_out,
            save_dir,
            romfs,
            exefs,
            console,
            program_id,
            sleep_limit,
            pace,
            quit,
            snapshot_next,
            calls,
            requests,
            presented,
            gx_log,
            gsp_list_groups,
            missing_titles,
            save_bytes_read,
            save_bytes_written,
            save_commits,
            romfs_read,
            y2r_conversions,
            y2r_time,
            random,
            pica
        );
        next.pica.restart_process();
        next.audio.sink = kernel.audio.sink.take();
        next.audio.record = kernel.audio.record.take();
        next.audio.output = std::mem::take(&mut kernel.audio.output);
        next.audio.frames = kernel.audio.frames;
        next.restarts = kernel.restarts + 1;
        next.deliver_arg = Some(args);
        *cpu = self.cpu.clone();
        *memory = self.memory.clone();
        *kernel = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicU64, Arc};

    #[test]
    fn self_jump_reboots_pristine_memory_and_retains_session_and_delivery() {
        let process = Process {
            linear_base: 0x3000_0000,
            app_region: 0x0500_0000,
            main_priority: 0x30,
            executable_end: 0x700000,
            save_data_size: 0,
        };
        let mut kernel = Kernel::new(process, Vec::new(), vec!["APT:A".into()]);
        kernel.program_id = 0x0004_0000_001b_5000;
        let mut cpu = Cpu { tls: 0x1000, ..Default::default() };
        cpu.r[15] = 0x100000;
        let mut mem = Memory::new();
        mem.map(0x1000, vec![0; 0x1000], Perm::RW).unwrap();
        let boot = BootImage::capture(&cpu, &mem, &kernel).unwrap();
        let pristine = cpu.clone();
        let pad = Arc::new(AtomicU64::new(1));
        kernel.live_pad = Some(Arc::clone(&pad));
        kernel.virtual_clock = Some(5_000_000_000);
        kernel.native_timing = true;
        kernel.vblanks = 299;
        kernel.save_dir = Some("/private/saves".into());
        let metadata_path = std::env::temp_dir().join(format!("usum-restart-exefs-{}", std::process::id()));
        std::fs::write(&metadata_path, [1, 2, 3, 4]).unwrap();
        let metadata = fs::Image::open(&metadata_path, usum_import::romfs::Image { offset: 0, size: 4 }).unwrap();
        std::fs::remove_file(metadata_path).unwrap();
        kernel.exefs.insert("icon".into(), metadata);
        kernel.self_ncch_archives.insert(1);
        kernel.pica.regs[0x41] = 0xffff;
        kernel.pica.draws = 47;
        #[cfg(feature = "vulkan-compare")]
        {
            kernel.pica.enable_vulkan_acceleration();
            kernel.pica.enable_vulkan_deferred_readback();
            kernel.pica.enable_vulkan_vertex();
            kernel.pica.set_vulkan_resolution_scale(6).unwrap();
        }
        mem.map(0x4000, vec![0x83; 0x1000], Perm::RW).unwrap();
        let ipc = cpu.tls + services::COMMAND_BUFFER;
        let send = |kernel: &mut Kernel, cpu: &mut Cpu, mem: &mut Memory, words: &[u32]| {
            Message::reply(mem, ipc, words).unwrap();
            kernel.request(cpu, mem, "APT:A".into())
        };
        assert!(matches!(send(&mut kernel, &mut cpu, &mut mem, &[0x0031_0100, 2, u32::MAX, u32::MAX, 0]), Outcome::Continue));
        mem.write_range(0x1500, &[9, 8, 7]).unwrap();
        mem.write_range(0x1520, &[0x91; 32]).unwrap();
        let Outcome::Restart(args) =
            send(&mut kernel, &mut cpu, &mut mem, &[0x0032_0084, 3, 32, (3 << 14) | 2, 0x1500, (32 << 14) | 0x802, 0x1520])
        else {
            panic!("expected process restart")
        };
        assert_eq!(args.parameter, [9, 8, 7]);
        assert_eq!(args.hmac, [0x91; 32]);
        let old_handle = kernel.open(Object::Event { signaled: true, reset: 1 });
        boot.restart(&mut cpu, &mut mem, &mut kernel, args);
        assert_eq!(cpu, pristine);
        assert_eq!(mem.read_u8(0x1500).unwrap(), 0);
        assert!(mem.read_u8(0x4000).is_err());
        assert!(kernel.object(old_handle).is_none());
        assert_eq!(kernel.threads.len(), 1);
        assert_eq!(kernel.virtual_clock, Some(5_000_000_000));
        assert!(kernel.native_timing);
        assert_eq!(kernel.vblanks, 299);
        assert_eq!(kernel.restarts, 1);
        assert_eq!(kernel.save_dir.as_deref(), Some(std::path::Path::new("/private/saves")));
        assert_eq!(kernel.exefs["icon"].read(0, 4).unwrap(), [1, 2, 3, 4]);
        assert!(kernel.self_ncch_archives.is_empty(), "archive handles belong to the exited process");
        assert!(Arc::ptr_eq(kernel.live_pad.as_ref().unwrap(), &pad));
        assert_eq!(kernel.pica.regs[0x41], 0);
        assert_eq!(kernel.pica.draws, 47, "profiling remains cumulative");
        // Delivery belongs to the new process and is consumed exactly once.
        mem.write_u32(ipc + 0x100, (3 << 14) | 2).unwrap();
        mem.write_u32(ipc + 0x104, 0x1500).unwrap();
        mem.write_u32(ipc + 0x108, (32 << 14) | 0x802).unwrap();
        mem.write_u32(ipc + 0x10c, 0x1520).unwrap();
        assert!(matches!(send(&mut kernel, &mut cpu, &mut mem, &[0x0035_0080, 3, 32]), Outcome::Continue));
        assert_eq!(mem.read_range(0x1500, 3).unwrap(), [9, 8, 7]);
        assert_eq!(mem.read_range(0x1520, 32).unwrap(), [0x91; 32]);
        assert_eq!(mem.read_u32(ipc + 8).unwrap(), kernel.program_id as u32);
        assert_eq!(mem.read_u32(ipc + 12).unwrap(), (kernel.program_id >> 32) as u32);
        assert_eq!(mem.read_u32(ipc + 16).unwrap(), 1);
        assert!(matches!(send(&mut kernel, &mut cpu, &mut mem, &[0x0035_0080, 3, 32]), Outcome::Continue));
        assert_eq!(mem.read_u32(ipc + 16).unwrap(), 0);
    }

    #[test]
    fn jumping_requires_self_preparation_valid_descriptors_and_readable_buffers() {
        for (prepare, words) in [
            (false, vec![0x0032_0084, 0, 0, 2, 0, 0x802, 0]),
            (true, vec![0x0032_0084, 0x301, 0, (0x301 << 14) | 2, 0, 0x802, 0]),
            (true, vec![0x0032_0084, 1, 0, 2, 0x1500, 0x802, 0]),
            (true, vec![0x0032_0084, 1, 0, (1 << 14) | 2, 0x9000, 0x802, 0]),
        ] {
            let mut kernel = Kernel { apt_jump_prepared: prepare, ..Default::default() };
            let mut cpu = Cpu { tls: 0x1000, ..Default::default() };
            let mut mem = Memory::new();
            mem.map(0x1000, vec![0; 0x1000], Perm::RW).unwrap();
            Message::reply(&mut mem, 0x1080, &words).unwrap();
            assert!(matches!(kernel.request(&mut cpu, &mut mem, "APT:A".into()), Outcome::Unimplemented(_)));
        }
    }
}
