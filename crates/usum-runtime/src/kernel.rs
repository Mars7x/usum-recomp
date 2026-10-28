//! HLE of the 3DS kernel's system calls, implemented one at a time in the
//! order the game reaches them (milestone 5).
//!
//! Register conventions follow the game's own SVC wrappers (checked by
//! disassembly, RESEARCH.md R-025) and libctru's `svc.s` (read for
//! conventions only): arguments in r0-r4 or so, the result code in r0,
//! outputs in r1 upward. Semantics follow 3dbrew's SVC pages. Where neither
//! settles a detail, the code says so. An SVC not implemented here stops
//! execution with a report; nothing is guessed.

use std::collections::BTreeMap;

use usum_guest::{Cpu, Memory, Perm};

use crate::boot::{Block, Process, STATE_CODE, STATE_FREE, STATE_PRIVATE, TLS, TLS_SLOT, TLS_SLOTS};
use crate::services::{self, apt, cfg, dsp, frd, fs, gsp, hid, ro, srv, uds, y2r, Message};

/// 3DS result codes. 0 is success; failures have bit 31 set, which is
/// what the game's wrappers test.
pub const RESULT_SUCCESS: u32 = 0;
/// Result for a handle that names no object (3dbrew "Error codes":
/// summary invalid argument, module kernel, description invalid handle).
pub const RESULT_INVALID_HANDLE: u32 = 0xD8E0_07F7;
/// WaitSynchronization* timed out (3dbrew "Error codes": "Timeout occurred
/// with svcWaitSynchronization*, when timeout is not ~0").
pub const RESULT_TIMEOUT: u32 = 0x0940_1BFE;

/// Pseudo-handles every thread can use without opening anything.
pub const CURRENT_THREAD: u32 = 0xFFFF_8000;
pub const CURRENT_PROCESS: u32 = 0xFFFF_8001;

/// Kernel objects the game has created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Object {
    AddressArbiter,
    /// The client end of a session to a service (or to the srv: port).
    Session {
        service: String,
    },
    Semaphore {
        count: i32,
        max: i32,
    },
    /// The process's resource limit (3dbrew KResourceLimit).
    ResourceLimit,
    Thread {
        id: u32,
    },
    /// Owner thread (None: free) and recursion count.
    Mutex {
        owner: Option<u32>,
        count: u32,
    },
    /// `reset`: 0 one-shot (clears when it wakes a thread), 1 sticky.
    Event {
        signaled: bool,
        reset: u32,
    },
    /// A shared memory block; `mapped` where the process mapped it.
    SharedMemory {
        size: u32,
        mapped: Option<u32>,
    },
    /// A file session onto a file in a save archive (host path), opened
    /// with these FS open flags.
    SaveFile {
        path: std::path::PathBuf,
        flags: u32,
    },
    /// A directory session: its entries, and how many were read.
    Directory {
        entries: Vec<fs::Entry>,
        read: usize,
    },
    /// A file session onto `size` bytes of the RomFS image from `start`.
    RomFsFile {
        start: u64,
        size: u64,
    },
}

/// The ARM11 system tick rate (libctru `SYSCLOCK_ARM11`: 16,756,991 Hz x 16).
pub const TICKS_PER_SECOND: u64 = 268_111_856;

/// The main thread's ID. A choice: the real kernel's numbering of a
/// game's main thread is not established.
pub const MAIN_THREAD_ID: u32 = 1;

/// The game's process ID. A choice like the thread IDs: the value the
/// real kernel gives an application is not established.
pub const PROCESS_ID: u32 = 0x30;

/// One SVC as the game made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub number: u32,
    /// The instruction after the SVC.
    pub resume: u32,
    pub args: [u32; 5],
    /// r0 afterwards: the result code. None if not implemented, or if the
    /// thread blocked (then `blocked`; the result comes at the wake-up).
    pub result: Option<u32>,
    pub blocked: bool,
}

pub enum Outcome {
    Continue,
    /// The calling thread blocked, exited or was preempted: the run loop
    /// must call [`Kernel::switch`].
    Switch,
    /// Not implemented; the string says what exactly.
    Unimplemented(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadState {
    Ready,
    Waiting(Wait),
    Exited,
}

/// What a waiting thread waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wait {
    /// Objects (empty for SleepThread).
    pub handles: Vec<u32>,
    /// All of them (WaitSynchronizationN with waitAll), or any one.
    pub all: bool,
    /// When it times out, in nanoseconds of the kernel clock; None: never.
    pub deadline: Option<u64>,
    /// WaitSynchronizationN reports the index of the object in r1.
    pub index_in_r1: bool,
    /// An address arbiter wait: woken by ArbitrateAddress SIGNAL on it.
    pub address: Option<u32>,
    /// r0 when the deadline passes.
    pub timeout_result: u32,
}

impl Wait {
    fn objects(handles: Vec<u32>, all: bool, deadline: Option<u64>, index_in_r1: bool) -> Wait {
        Wait { handles, all, deadline, index_in_r1, address: None, timeout_result: RESULT_TIMEOUT }
    }
}

/// A guest thread. Translated code always returns to the host at SVCs and
/// budget exits, so `cpu` is the thread's complete state: threads are
/// multiplexed on one host thread by swapping it (ADR-0006).
#[derive(Debug, Clone)]
pub struct Thread {
    pub id: u32,
    pub priority: u32,
    /// 0 or 1 (the core it runs on).
    pub core: u32,
    /// Saved context; stale while the thread is running.
    pub cpu: Cpu,
    pub state: ThreadState,
}

/// One IPC request the game sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub service: String,
    pub header: u32,
    /// The parameters after the header, as many as the header says.
    pub words: Vec<u32>,
    pub handled: bool,
    /// The VBlank count when it was made.
    pub vblank: u64,
}

/// The LCDs' refresh period: about 59.83 Hz. The exact rate is not
/// established here (R-029); this is 268,111,856 ticks / 4,481,136.
pub const VBLANK_NS: u64 = 16_713_680;
/// An audio frame: 160 samples at 32,728 Hz (3dbrew "DSP Memory Region":
/// 160 samples per output buffer; the rate is an assumption, R-029).
pub const AUDIO_FRAME_NS: u64 = 4_888_780;

/// A framebuffer GSP took for a screen (0 top, 1 bottom) at VBlank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Present {
    pub screen: u32,
    pub active: u32,
    pub left: u32,
    pub right: u32,
    pub stride: u32,
    pub format: u32,
}

/// Where the kernel hands the screens to a live display (`play`).
pub struct FrameOut {
    /// The newest framebuffers, top and bottom, as stored.
    pub frames: std::sync::Mutex<[Option<crate::probe::RawFrame>; 2]>,
    /// Called after each VBlank's frames are in place.
    pub notify: Box<dyn Fn() + Send + Sync>,
}

/// The game's registration with GSP.
#[derive(Debug, Clone, Copy)]
pub struct Gsp {
    pub flags: u32,
    /// Signaled when GSP queues an interrupt for the game.
    pub event: u32,
    /// The shared memory block.
    pub memory: u32,
}

/// What APT Initialize set up.
#[derive(Debug, Clone)]
pub struct Apt {
    pub app_id: u32,
    pub attributes: u32,
    pub notification: u32,
    pub resume: u32,
    /// The parameter waiting to be received.
    pub parameter: Option<apt::Parameter>,
}

#[derive(Default)]
pub struct Kernel {
    /// When the kernel started; system ticks count from here.
    started: Option<std::time::Instant>,
    process: Option<Process>,
    /// The address space's blocks, sorted by base, for QueryMemory.
    blocks: Vec<Block>,
    handles: BTreeMap<u32, Object>,
    next_handle: u32,
    /// Services the exheader permits (srv refuses the others).
    services: Vec<String>,
    pub calls: Vec<Call>,
    pub requests: Vec<Request>,
    pub threads: Vec<Thread>,
    /// The APT lock (a mutex, created on first GetLockHandle).
    apt_lock: Option<u32>,
    /// The HLE DSP's pipes.
    pub dsp_pipes: dsp::Pipes,
    /// DSP interrupt events: (interrupt, channel) -> event handle.
    pub dsp_events: BTreeMap<(u32, u32), u32>,
    /// The event DSP GetSemaphoreEventHandle gave out.
    pub dsp_semaphore_event: Option<u32>,
    /// What the game wrote to DSP pipes: channel, bytes.
    pub dsp_pipe_writes: Vec<(u32, Vec<u8>)>,
    /// The DSP firmware the game loaded: size and SHA-256 (not run).
    pub dsp_firmware: Option<(usize, String)>,
    /// The CRS RO was initialized with: where it is mapped, and parsed.
    pub crs: Option<(u32, usum_import::cro::Cro)>,
    /// Modules RO has loaded, the CRS first.
    pub modules: Vec<crate::ro::Loaded>,
    /// The CRR RO holds: address and size.
    pub crr: Option<(u32, u32)>,
    /// The GPU's register state.
    pub pica: crate::pica::Pica,
    /// GX commands run so far: ID and parameters.
    pub gx_log: Vec<(u32, Vec<u32>)>,
    /// The BOSS storage entry: extdata ID, size, entry ID, media type.
    pub boss_storage: Option<(u64, u32, u32, u32)>,
    /// The input script (buttons by VBlank) and the buttons held last.
    pub input: Vec<hid::Step>,
    /// Live input, when a frontend supplies it: the PAD bits (as in the
    /// input script) held now; it takes the script's place.
    pub live_pad: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    /// Live touch, packed by `hid::pack_touch`; takes the script's place.
    pub live_touch: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    /// Live input as HID read it, one step per change: replayed as an
    /// input script, it repeats the session exactly (the clock is
    /// virtual).
    pub recorded: Vec<hid::Step>,
    pad: u32,
    /// HID's shared memory and its five events.
    pub hid: Option<(u32, [u32; 5])>,
    /// The DSP's per-frame source processing (timing only, no mixing yet).
    pub audio: crate::audio::Dsp,
    /// Local wireless, once initialized.
    pub uds: Option<uds::Uds>,
    /// The YUV-to-RGB converter's settings (video).
    pub y2r: y2r::Y2r,
    /// The state of the random numbers services hand out (SSLC
    /// GenerateRandomData): a fixed seed, so runs repeat exactly; a real
    /// console's numbers differ every time (a choice, R-047).
    random: u64,
    /// Hardware timeline (kernel clock, ns): the next VBlank and audio frame.
    next_vblank: u64,
    next_audio_frame: u64,
    pub vblanks: u64,
    pub audio_frames: u64,
    /// Framebuffers GSP took at VBlank.
    pub presented: Vec<Present>,
    /// Live display: at every VBlank each screen's newest framebuffer is
    /// copied here and the consumer woken, as the LCDs scan out.
    pub frame_out: Option<std::sync::Arc<FrameOut>>,
    /// The first error a hardware event ran into.
    pub hardware_error: Option<String>,
    /// The GSP client (one: the game).
    pub gsp: Option<Gsp>,
    /// The emulated console's settings (config blocks).
    pub console: cfg::Console,
    /// The RomFS level 3 image (SelfNCCH), if the dump has one.
    pub romfs: Option<fs::Image>,
    /// Where the game's save data (archive 4) lives on the host. Never
    /// next to the dump; the probe uses a scratch directory of its own.
    pub save_dir: Option<std::path::PathBuf>,
    /// The longest the host may sleep for the next timeout when every
    /// thread waits; beyond it `switch` gives up (the probe sets one).
    pub sleep_limit: Option<std::time::Duration>,
    /// Live play: the wall-clock moment virtual time 0 corresponds to.
    /// When set, jumps of the virtual clock over idle time wait for the
    /// wall clock to catch up, so the game never runs ahead of real time
    /// (and its sound comes out evenly, not in bursts).
    pub pace: Option<std::time::Instant>,
    /// Set by a live frontend to stop: a paced wait checks it between
    /// steps of at most 50 ms, so quitting never waits on the game.
    pub quit: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The VBlank count at which the probe next snapshots the screens.
    pub snapshot_next: u64,
    /// The virtual clock in nanoseconds, if one is used.
    pub virtual_clock: Option<u64>,
    /// Nanoseconds the host has slept with every thread waiting.
    pub slept: u64,
    /// Set when `switch` gave up: nanoseconds until the next timeout.
    pub idle_wait: Option<u64>,
    /// Open FS archives by handle.
    pub archives: BTreeMap<u64, fs::Archive>,
    next_archive: u64,
    /// Titles whose contents the game asked for but that are not here
    /// (system data such as the NGWord list).
    pub missing_titles: Vec<u64>,
    /// Bytes the game has read from and written to its save files.
    pub save_bytes_read: u64,
    pub save_bytes_written: u64,
    pub save_commits: u64,
    /// Bytes the game has read from the RomFS.
    pub romfs_read: u64,
    /// Settings the game handed to services (services::settings), by
    /// service and command: the last parameters.
    pub settings: BTreeMap<(String, u32), Vec<u32>>,
    /// The application's registration with APT (Initialize).
    pub apt: Option<Apt>,
    current: usize,
    next_thread_id: u32,
}

/// SVC names from 3dbrew's SVC table, for reports.
pub fn name(number: u32) -> &'static str {
    match number {
        0x01 => "ControlMemory",
        0x02 => "QueryMemory",
        0x03 => "ExitProcess",
        0x08 => "CreateThread",
        0x09 => "ExitThread",
        0x0A => "SleepThread",
        0x0B => "GetThreadPriority",
        0x0C => "SetThreadPriority",
        0x13 => "CreateMutex",
        0x14 => "ReleaseMutex",
        0x15 => "CreateSemaphore",
        0x16 => "ReleaseSemaphore",
        0x17 => "CreateEvent",
        0x18 => "SignalEvent",
        0x19 => "ClearEvent",
        0x1A => "CreateTimer",
        0x1B => "SetTimer",
        0x1E => "CreateMemoryBlock",
        0x1F => "MapMemoryBlock",
        0x21 => "CreateAddressArbiter",
        0x22 => "ArbitrateAddress",
        0x23 => "CloseHandle",
        0x24 => "WaitSynchronization1",
        0x25 => "WaitSynchronizationN",
        0x27 => "DuplicateHandle",
        0x28 => "GetSystemTick",
        0x2A => "GetSystemInfo",
        0x2B => "GetProcessInfo",
        0x2D => "ConnectToPort",
        0x32 => "SendSyncRequest",
        0x35 => "GetProcessId",
        0x37 => "GetThreadId",
        0x38 => "GetResourceLimit",
        0x39 => "GetResourceLimitLimitValues",
        0x3A => "GetResourceLimitCurrentValues",
        0x3C => "Break",
        0x3D => "OutputDebugString",
        _ => "?",
    }
}

impl Kernel {
    pub fn new(process: Process, mut blocks: Vec<Block>, services: Vec<String>) -> Kernel {
        blocks.sort_by_key(|b| b.base);
        // the real kernel's handle encoding is not reproduced; any value
        // that is not a pseudo-handle works for the game
        let main = Thread {
            id: MAIN_THREAD_ID,
            priority: process.main_priority,
            core: 0,
            cpu: Cpu::default(),
            state: ThreadState::Ready,
        };
        Kernel {
            started: Some(std::time::Instant::now()),
            process: Some(process),
            blocks,
            services,
            next_handle: 0x0000_8000,
            pica: crate::pica::Pica::with_linear_base(process.linear_base),
            threads: vec![main],
            current: 0,
            next_thread_id: MAIN_THREAD_ID + 1,
            ..Kernel::default()
        }
    }

    /// A NUL-terminated name of at most `max` bytes at `address`.
    fn read_name(mem: &Memory, address: u32, max: u32) -> Option<String> {
        let mut bytes = Vec::new();
        for i in 0..max {
            match mem.read_u8(address.wrapping_add(i)).ok()? {
                0 => return Some(String::from_utf8_lossy(&bytes).into_owned()),
                b => bytes.push(b),
            }
        }
        None
    }

    /// ControlMemory, for what the game has used: MEMOP_ALLOC (3) of the
    /// regular heap at a given address, and MEMOP_ALLOC with
    /// MEMOP_LINEAR_FLAG (0x10003) at an address the kernel chooses; both
    /// page-aligned and within the commit limit. Everything else is
    /// reported, not guessed.
    ///
    /// LINEAR placement is a choice: the real kernel's allocator is not
    /// reproduced. LINEAR virtual addresses map 1:1 onto FCRAM offsets, so
    /// allocations are placed after the memory already committed, keeping
    /// each physical page used once.
    fn control_memory(&mut self, cpu: &mut Cpu, mem: &mut Memory) -> Outcome {
        const MEMOP_ALLOC: u32 = 3;
        const MEMOP_ALLOC_LINEAR: u32 = 0x1_0003;
        const HEAP: std::ops::Range<u32> = 0x0800_0000..0x1000_0000;
        const STATE_CONTINUOUS: u32 = 7;
        let (op, addr, size, perm) = (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]);
        let describe = || format!("ControlMemory(op {op:#x}, addr {addr:#010x}, size {size:#x}, perm {perm})");
        if perm & !3 != 0 || size % 0x1000 != 0 || size == 0 {
            return Outcome::Unimplemented(describe());
        }
        let (Some(process), committed) = (self.process, self.committed()) else { return Outcome::Unimplemented(describe()) };
        if committed + u64::from(size) > u64::from(process.app_region) {
            return Outcome::Unimplemented(format!("{}: over the commit limit", describe()));
        }
        let (base, state) = match op {
            MEMOP_ALLOC
                if addr % 0x1000 == 0 && HEAP.contains(&addr) && u64::from(addr) + u64::from(size) <= u64::from(HEAP.end) =>
            {
                (addr, STATE_PRIVATE)
            }
            MEMOP_ALLOC_LINEAR if addr == 0 => (process.linear_base + committed as u32, STATE_CONTINUOUS),
            _ => return Outcome::Unimplemented(describe()),
        };
        let access = if perm & 2 != 0 { Perm::RW } else { Perm::R };
        if mem.map(base, vec![0; size as usize], access).is_err() {
            return Outcome::Unimplemented(format!("{}: already mapped", describe()));
        }
        self.blocks.push(Block { base, size, perm, state });
        self.blocks.sort_by_key(|b| b.base);
        cpu.r[1] = base;
        cpu.r[0] = RESULT_SUCCESS;
        Outcome::Continue
    }

    /// Maps `len` bytes at `from` at `to` instead (memory now reached
    /// only through `to`), and updates the blocks QueryMemory reports:
    /// the source becomes Aliased without access, the target AliasCode
    /// (3dbrew MemoryState 8 and 10).
    fn alias(&mut self, mem: &mut Memory, from: u32, len: u32, to: u32, perm: Perm) -> Result<(), String> {
        const STATE_ALIASED: u32 = 8;
        const STATE_ALIAS_CODE: u32 = 10;
        let i = self
            .blocks
            .iter()
            .position(|b| from >= b.base && u64::from(from) + u64::from(len) <= u64::from(b.base) + u64::from(b.size));
        let Some(i) = i.filter(|_| from.is_multiple_of(0x1000) && len.is_multiple_of(0x1000) && len > 0) else {
            return Err(format!("alias of {from:#x}+{len:#x}: not page-aligned inside one block"));
        };
        mem.remap(from, len, to, perm).map_err(|_| format!("alias of {from:#x}+{len:#x} to {to:#x}: overlaps"))?;
        let b = self.blocks.remove(i);
        if from > b.base {
            self.blocks.push(Block { size: from - b.base, ..b });
        }
        if from + len < b.base + b.size {
            self.blocks.push(Block { base: from + len, size: b.base + b.size - from - len, ..b });
        }
        self.blocks.push(Block { base: from, size: len, perm: 0, state: STATE_ALIASED });
        let p = if perm.write { 3 } else { 1 };
        self.blocks.push(Block { base: to, size: len, perm: p, state: STATE_ALIAS_CODE });
        self.blocks.sort_by_key(|b| b.base);
        Ok(())
    }

    /// The GSP shared memory's address, once mapped.
    fn gsp_memory(&self) -> Option<u32> {
        let g = self.gsp?;
        match self.handles.get(&g.memory) {
            Some(&Object::SharedMemory { mapped, .. }) => mapped,
            _ => None,
        }
    }

    /// Runs the GX commands queued in shared memory (GSP thread 0), then
    /// advances the queue. Returns whether an interrupt was queued (and
    /// the game's event signaled).
    fn gx_commands(&mut self, mem: &mut Memory) -> Result<bool, String> {
        let base = self.gsp_memory().ok_or("TriggerCmdReqQueue before the shared memory is mapped")?;
        let queue = base + gsp::command_queue(0);
        let fault = |f: usum_guest::Fault| format!("GX queue: {f}");
        let mut raised = false;
        loop {
            let header = mem.read_u32(queue).map_err(fault)?;
            let (index, count) = (header & 0xFF, (header >> 8) & 0xFF);
            if count == 0 || index >= gsp::QUEUE_ENTRIES {
                return Ok(raised);
            }
            let at = queue + 0x20 + 0x20 * index;
            let c: Vec<u32> = (0..8).map(|k| mem.read_u32(at + 4 * k)).collect::<Result<_, _>>().map_err(fault)?;
            let id = c[0] & 0xFF;
            let interrupt = match id {
                // cache maintenance: nothing to do on coherent host memory
                gsp::FLUSH_CACHE_REGIONS => None,
                // RequestDMA(source, destination, size): a copy; DMA when
                // done (3dbrew "GSP Shared Memory")
                gsp::REQUEST_DMA => {
                    let bytes = mem.read_range(c[1], c[3].min(0x0100_0000)).map_err(|f| format!("DMA source: {f}"))?;
                    mem.device_write(c[2], &bytes).map_err(|f| format!("DMA destination: {f}"))?;
                    Some(gsp::INTERRUPT_DMA)
                }
                // TextureCopy(input, output, size, input line width | gap << 16,
                // output width | gap, flags): a raw copy in lines, widths and
                // gaps in 16-byte units (3dbrew "GPU/External Registers",
                // TextureCopy); PPF when done
                gsp::TEXTURE_COPY => {
                    let unit = |w: u32| ((w & 0xFFFF) * 16, (w >> 16) * 16);
                    let ((in_w, in_gap), (out_w, out_gap)) = (unit(c[4]), unit(c[5]));
                    let size = c[3].min(0x0100_0000);
                    let bytes = gsp::gather(mem, c[1], size, in_w, in_gap).map_err(|f| format!("TextureCopy input: {f}"))?;
                    gsp::scatter(mem, c[2], &bytes, out_w, out_gap).map_err(|f| format!("TextureCopy output: {f}"))?;
                    Some(gsp::INTERRUPT_PPF)
                }
                // DisplayTransfer(input, output, input dims, output dims,
                // flags): PPF when done
                gsp::DISPLAY_TRANSFER => {
                    let dims = |w: u32| (w & 0xFFFF, w >> 16);
                    let (input_dims, output_dims, flags) = (dims(c[3]), dims(c[4]), c[5]);
                    let fin = crate::pica::bytes_per_pixel((flags >> 8) & 7).ok_or("DisplayTransfer input format")?;
                    let input = mem
                        .read_range(c[1], input_dims.0 * input_dims.1 * fin as u32)
                        .map_err(|f| format!("DisplayTransfer input: {f}"))?;
                    let out = crate::pica::display_transfer(&input, input_dims, output_dims, flags)?;
                    mem.device_write(c[2], &out).map_err(|f| format!("DisplayTransfer output: {f}"))?;
                    Some(gsp::INTERRUPT_PPF)
                }
                // MemoryFill: two buffers (start, value, end), control 0 |
                // control 1 << 16; PSC0 / PSC1 when each finishes
                gsp::MEMORY_FILL => {
                    for (k, (start, value, end)) in [(c[1], c[2], c[3]), (c[4], c[5], c[6])].into_iter().enumerate() {
                        let control = (c[7] >> (16 * k)) & 0xFFFF;
                        if start == 0 || control & 1 == 0 {
                            continue;
                        }
                        let bytes = gsp::fill(start, value, end, control)?;
                        mem.device_write(start, &bytes).map_err(|f| format!("MemoryFill: {f}"))?;
                        self.gsp_interrupt(mem, gsp::INTERRUPT_PSC0 + k as u8)?;
                        raised = true;
                    }
                    None
                }
                // ProcessCommandList(address, size, ...): P3D when it
                // finalizes (3dbrew: a list that does not hangs the GPU)
                gsp::PROCESS_COMMAND_LIST => {
                    if self.pica.run(mem, c[1], c[2])? {
                        Some(gsp::INTERRUPT_P3D)
                    } else {
                        return Err(format!(
                            "GPU command list at {:#x} ({:#x} bytes) does not finalize: the GPU would hang",
                            c[1], c[2]
                        ));
                    }
                }
                _ => return Err(format!("GX command {id} {:08x?}", &c[1..])),
            };
            self.gx_log.push((id, c[1..].to_vec()));
            // consumed: next index, one fewer
            let next = (index + 1) % gsp::QUEUE_ENTRIES;
            mem.write_u32(queue, (header & !0xFFFF) | ((count - 1) << 8) | next).map_err(fault)?;
            if let Some(i) = interrupt {
                self.gsp_interrupt(mem, i)?;
                raised = true;
            }
        }
    }

    /// Queues interrupt `id` for the game (3dbrew "GSP Shared Memory":
    /// byte 0 the start, byte 1 the count, the list from 0xC) and signals
    /// its event.
    fn gsp_interrupt(&mut self, mem: &mut Memory, id: u8) -> Result<(), String> {
        let (Some(base), Some(g)) = (self.gsp_memory(), self.gsp) else { return Ok(()) };
        let q = base + gsp::interrupt_queue(0);
        let fault = |f: usum_guest::Fault| format!("GSP interrupt queue: {f}");
        let (start, count) = (mem.read_u8(q).map_err(fault)?, mem.read_u8(q + 1).map_err(fault)?);
        if u32::from(count) >= gsp::INTERRUPT_ENTRIES {
            return Err("GSP interrupt queue full".into());
        }
        let slot = (u32::from(start) + u32::from(count)) % gsp::INTERRUPT_ENTRIES;
        mem.write_u8(q + 0xC + slot, id).map_err(fault)?;
        mem.write_u8(q + 1, count + 1).map_err(fault)?;
        if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&g.event) {
            *signaled = true;
        }
        Ok(())
    }

    /// Hardware events due, then the SVC. If an event woke a thread more
    /// urgent than the caller, the call ends in a switch (preemption).
    pub fn tick_then_svc(&mut self, cpu: &mut Cpu, mem: &mut Memory, number: u32) -> Outcome {
        let preempt = self.wakes_more_urgent_after(mem);
        match self.svc(cpu, mem, number) {
            Outcome::Continue if preempt => Outcome::Switch,
            other => other,
        }
    }

    fn wakes_more_urgent_after(&mut self, mem: &mut Memory) -> bool {
        self.hardware(mem) && self.wakes_more_urgent()
    }

    /// Wakes the threads whose waits are now satisfied; true if one is
    /// more urgent than the running thread.
    pub fn wakes_more_urgent(&mut self) -> bool {
        matches!(self.wake(), Outcome::Switch)
    }

    /// When the next hardware event is due, if any is scheduled.
    fn next_hardware_event(&self) -> Option<u64> {
        let vblank = self.gsp.map(|_| self.next_vblank);
        let audio = self.dsp_events.contains_key(&(dsp::PIPE_INTERRUPT, dsp::AUDIO_PIPE)).then_some(self.next_audio_frame);
        vblank.into_iter().chain(audio).min()
    }

    /// Runs the hardware events that are due by the kernel clock: VBlank
    /// for GSP, audio frames for the DSP. Returns whether an event was
    /// signaled (waiters may wake). Errors stop nothing here: they are
    /// recorded in `hardware_error` for the report.
    pub fn hardware(&mut self, mem: &mut Memory) -> bool {
        let now = self.now();
        let mut signaled = false;
        if self.gsp.is_some() {
            while self.next_vblank <= now {
                self.next_vblank += VBLANK_NS;
                if let Err(e) = self.vblank(mem) {
                    self.hardware_error.get_or_insert(e);
                }
                signaled = true;
            }
        }
        if let Some(&event) = self.dsp_events.get(&(dsp::PIPE_INTERRUPT, dsp::AUDIO_PIPE)) {
            while self.next_audio_frame <= now {
                self.next_audio_frame += AUDIO_FRAME_NS;
                self.audio_frames += 1;
                if let Err(e) = self.audio.frame(mem, self.pica.linear_base) {
                    self.hardware_error.get_or_insert(e);
                }
                if let Some(Object::Event { signaled: s, .. }) = self.handles.get_mut(&event) {
                    *s = true;
                }
                signaled = true;
            }
        }
        signaled
    }

    /// One VBlank: GSP takes new framebuffer settings the game left in
    /// shared memory (3dbrew "GSP Shared Memory": header byte 0 the entry
    /// index, byte 1 bit 0 "new data"; entries of 0x1C bytes from +4),
    /// then queues PDC0 and PDC1 unless the game asked to skip them.
    fn vblank(&mut self, mem: &mut Memory) -> Result<(), String> {
        let Some(base) = self.gsp_memory() else { return Ok(()) };
        let fault = |f: usum_guest::Fault| format!("GSP framebuffer info: {f}");
        for screen in 0..2u32 {
            let info = base + 0x200 + screen * 0x40;
            let (index, flags) = (mem.read_u8(info).map_err(fault)?, mem.read_u8(info + 1).map_err(fault)?);
            if flags & 1 != 0 {
                let entry = info + 4 + u32::from(index & 1) * 0x1C;
                let w: Vec<u32> = (0..7).map(|k| mem.read_u32(entry + 4 * k)).collect::<Result<_, _>>().map_err(fault)?;
                self.presented.push(Present { screen, active: w[0], left: w[1], right: w[2], stride: w[3], format: w[4] });
                mem.write_u8(info + 1, flags & !1).map_err(fault)?;
            }
        }
        self.vblanks += 1;
        if let Some(out) = &self.frame_out {
            let latest = |screen| self.presented.iter().rev().find(|p| p.screen == screen);
            let raw = [(0u32, 400u32), (1, 320)]
                .map(|(screen, columns)| latest(screen).and_then(|p| crate::probe::framebuffer_raw(mem, p, columns).ok()));
            if let Ok(mut frames) = out.frames.lock() {
                for (slot, frame) in frames.iter_mut().zip(raw) {
                    if frame.is_some() {
                        *slot = frame;
                    }
                }
            }
            (out.notify)();
        }
        if let Err(e) = self.hid_update(mem) {
            self.hardware_error.get_or_insert(e);
        }
        let q = base + gsp::interrupt_queue(0);
        let skip_pdc = mem.read_u8(q + 3).map_err(|f| format!("GSP interrupt queue: {f}"))? & 1 != 0;
        if !skip_pdc {
            for id in [gsp::INTERRUPT_PDC0, gsp::INTERRUPT_PDC0 + 1] {
                // a full queue drops the PDC interrupt (3dbrew counts missed ones)
                let count = mem.read_u8(q + 1).map_err(|f| format!("GSP interrupt queue: {f}"))?;
                if u32::from(count) < 0x20 {
                    self.gsp_interrupt(mem, id)?;
                }
            }
        }
        Ok(())
    }

    /// The host path of a file the game names in an open save archive.
    fn archive_file(&self, mem: &Memory, archive: u64, path: fs::Path3) -> Result<std::path::PathBuf, String> {
        let (Some(named), Some(root)) = (self.archives.get(&archive), &self.save_dir) else {
            return Err(format!("archive handle {archive:#x} is not open"));
        };
        let bytes = mem.read_range(path.address, path.size.min(0x400)).map_err(|f| format!("FS path: {f}"))?;
        let parts = fs::archive_path(path.kind, &bytes)?;
        Ok(fs::host_path(&named.dir(root), &parts))
    }

    /// RO LoadCRO: checks, places, maps, relocates and links one module.
    fn load_cro(&mut self, mem: &mut Memory, load: ro::LoadCro) -> Result<(), String> {
        let ro::LoadCro { buffer: at, address, size, data, bss, fix_level, .. } = load;
        let end = self.process.map_or(0, |p| p.executable_end);
        if address != ro::mapped_address(at, end) {
            return Err(format!("CRO mapped at {address:#x}, not where 3dbrew's rule puts it"));
        }
        let bytes = mem.read_range(at, size).map_err(|f| format!("CRO at {at:#x}: {f}"))?;
        let c = usum_import::cro::parse(&bytes).map_err(|e| format!("CRO at {at:#x}: {e}"))?;
        if c.is_crs || self.modules.iter().any(|m| m.cro.name == c.name) {
            return Err(format!("CRO {} loaded twice, or a CRS", c.name));
        }
        // .data from the file into its buffer, .bss zeroed
        for s in &c.segments {
            match s.kind {
                usum_import::cro::SegmentKind::Data if s.size > 0 => {
                    if s.size > data.1 {
                        return Err(format!("{}: .data {:#x} bytes, buffer {:#x}", c.name, s.size, data.1));
                    }
                    let from = &bytes[s.offset as usize..(s.offset + s.size) as usize];
                    mem.write_range(data.0, from).map_err(|f| format!("{} .data: {f}", c.name))?;
                }
                usum_import::cro::SegmentKind::Bss if s.size > 0 => {
                    if s.size > bss.1 {
                        return Err(format!("{}: .bss {:#x} bytes, buffer {:#x}", c.name, s.size, bss.1));
                    }
                    mem.write_range(bss.0, &vec![0; s.size as usize]).map_err(|f| format!("{} .bss: {f}", c.name))?;
                }
                _ => {}
            }
        }
        // the image is mapped read-only and executable; RO writes it as a device
        self.alias(mem, at, size.next_multiple_of(0x1000), address, Perm::R)?;
        let segments = crate::ro::place(&c, address, data.0, bss.0);
        let unresolved = (0..c.imports.len()).collect();
        let sha256 = usum_import::sha256_hex(&bytes);
        self.modules.push(crate::ro::Loaded { cro: c, segments, base: address, image: address, unresolved, sha256 });
        let new = self.modules.len() - 1;
        crate::ro::relocate_internal(mem, &self.modules[new])?;
        // its imports from everyone, then everyone's from it
        for i in (0..self.modules.len()).rev() {
            crate::ro::link(mem, &mut self.modules, i)?;
        }
        crate::ro::bind_unresolved(mem, &self.modules[new])?;
        crate::ro::rebase_header(mem, &self.modules[new], fix_level)?;
        Ok(())
    }

    /// One HID update (at VBlank; the real rate is not established): the
    /// PAD and touch sections of HID shared memory as 3dbrew "HID Shared
    /// Memory" lays them out, from the input script, then the PAD event.
    fn hid_update(&mut self, mem: &mut Memory) -> Result<(), String> {
        let Some((memory, events)) = self.hid else { return Ok(()) };
        let Some(&Object::SharedMemory { mapped: Some(base), .. }) = self.handles.get(&memory) else { return Ok(()) };
        let fault = |f: usum_guest::Fault| format!("HID shared memory: {f}");
        // HID writes as a device: the process maps this memory read-only
        let put = |mem: &mut Memory, at: u32, v: u32| mem.device_write(at, &v.to_le_bytes()).map_err(fault);
        let ticks = self.now() as u128 * u128::from(TICKS_PER_SECOND) / 1_000_000_000;
        let pad = match &self.live_pad {
            Some(live) => live.load(std::sync::atomic::Ordering::Relaxed),
            None => hid::held(&self.input, self.vblanks),
        };
        let previous = self.pad;
        self.pad = pad;
        for section in [0u32, 0xA8] {
            let at = base + section;
            // 3dbrew: the ticks are those of the updates of entry 0, the
            // previous one kept at +8; readers infer the update rate from
            // them, so they change only when the index wraps to 0 (R-038)
            let index = (mem.read_u32(at + 0x10).map_err(fault)? + 1) % 8;
            if index == 0 {
                let old_tick = [mem.read_u32(at).map_err(fault)?, mem.read_u32(at + 4).map_err(fault)?];
                put(mem, at + 8, old_tick[0])?;
                put(mem, at + 12, old_tick[1])?;
                put(mem, at, ticks as u32)?;
                put(mem, at + 4, (ticks >> 32) as u32)?;
            }
            put(mem, at + 0x10, index)?;
            if section == 0 {
                // the circle pad as two s16 (x, y): raw at +0x20, and in
                // each entry after current, pressed, released
                let (x, y) = hid::circle_pad(pad);
                let stick = u32::from(x as u16) | u32::from(y as u16) << 16;
                put(mem, at + 0x1C, pad)?;
                put(mem, at + 0x20, stick)?;
                let entry = at + 0x28 + 0x10 * index;
                for (k, v) in [pad, pad & !previous, previous & !pad, stick].into_iter().enumerate() {
                    put(mem, entry + 4 * k as u32, v)?;
                }
            } else {
                // 3dbrew: x, y (pixels), then a u32 whose low byte is 1
                // when the entry holds a touch; all zero when untouched.
                // The raw entry at +0x18 gets the same pixel values (a
                // choice: 3dbrew does not give the raw units)
                let touch = match &self.live_touch {
                    Some(live) => hid::unpack_touch(live.load(std::sync::atomic::Ordering::Relaxed)),
                    None => hid::touched(&self.input, self.vblanks),
                };
                if self.live_pad.is_some() {
                    let step = hid::Step { at: self.vblanks, pad, touch };
                    if self.recorded.last().is_none_or(|last| (last.pad, last.touch) != (pad, touch)) {
                        self.recorded.push(step);
                    }
                }
                let mut entry = [0u8; 8];
                if let Some((x, y)) = touch {
                    entry[..2].copy_from_slice(&x.to_le_bytes());
                    entry[2..4].copy_from_slice(&y.to_le_bytes());
                    entry[4] = 1;
                }
                mem.device_write(at + 0x18, &entry).map_err(fault)?;
                mem.device_write(at + 0x20 + 8 * index, &entry).map_err(fault)?;
            }
        }
        for event in &events[..2] {
            if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(event) {
                *signaled = true;
            }
        }
        Ok(())
    }

    /// Merges touching blocks with the same permission and state.
    fn merge_blocks(&mut self) {
        self.blocks.sort_by_key(|b| b.base);
        let mut merged: Vec<Block> = Vec::with_capacity(self.blocks.len());
        for b in std::mem::take(&mut self.blocks) {
            match merged.last_mut() {
                Some(last)
                    if last.perm == b.perm
                        && last.state == b.state
                        && u64::from(last.base) + u64::from(last.size) == u64::from(b.base) =>
                {
                    last.size += b.size;
                }
                _ => merged.push(b),
            }
        }
        self.blocks = merged;
    }

    /// Undoes `alias`: the memory at `to` goes back to `from`, readable and
    /// writable, and the blocks return to Private.
    fn unalias(&mut self, mem: &mut Memory, to: u32, len: u32, from: u32) -> Result<(), String> {
        mem.remap(to, len, from, Perm::RW).map_err(|_| format!("unmapping {to:#x}+{len:#x} back to {from:#x} failed"))?;
        self.blocks.retain(|b| !(b.base == to && b.size == len));
        for b in &mut self.blocks {
            if b.base == from && b.size == len {
                *b = Block { base: from, size: len, perm: 3, state: STATE_PRIVATE };
            }
        }
        self.merge_blocks();
        Ok(())
    }

    /// Changes the permission of `len` bytes at `address`, which must lie
    /// page-aligned inside one block.
    fn protect(&mut self, mem: &mut Memory, address: u32, len: u32, perm: Perm) -> Result<(), String> {
        let i = self
            .blocks
            .iter()
            .position(|b| address >= b.base && u64::from(address) + u64::from(len) <= u64::from(b.base) + u64::from(b.size));
        let Some(i) = i.filter(|_| address.is_multiple_of(0x1000) && len.is_multiple_of(0x1000) && len > 0) else {
            return Err(format!("protect {address:#x}+{len:#x}: not page-aligned inside one block"));
        };
        mem.remap(address, len, address, perm).map_err(|_| format!("protect {address:#x}+{len:#x} failed"))?;
        let b = self.blocks.remove(i);
        if address > b.base {
            self.blocks.push(Block { size: address - b.base, ..b });
        }
        if address + len < b.base + b.size {
            self.blocks.push(Block { base: address + len, size: b.base + b.size - address - len, ..b });
        }
        self.blocks.push(Block { base: address, size: len, perm: if perm.write { 3 } else { 1 }, ..b });
        self.blocks.sort_by_key(|b| b.base);
        Ok(())
    }

    /// Memory committed from the APPLICATION region so far: code, data,
    /// bss and the stack (TLS comes from the BASE region). This is the
    /// runtime's reading of the kernel's accounting, not a measured value.
    fn committed(&self) -> u64 {
        // code, private (data, bss, stack, heap) and continuous (LINEAR) memory
        self.blocks
            .iter()
            // aliased memory is still committed once, at its source
            .filter(|b| matches!(b.state, STATE_CODE | STATE_PRIVATE | 7 | 8) && b.base != TLS)
            .map(|b| u64::from(b.size))
            .sum()
    }

    fn resource_values(&mut self, cpu: &mut Cpu, mem: &mut Memory, limits: bool) -> Outcome {
        const COMMIT: u32 = 1;
        let (out, types, count) = (cpu.r[0], cpu.r[2], cpu.r[3]);
        for i in 0..count {
            let kind = match mem.read_u32(types.wrapping_add(4 * i)) {
                Ok(k) => k,
                Err(f) => return Outcome::Unimplemented(format!("resource limit types unreadable: {f}")),
            };
            let value: u64 = match (kind, limits) {
                (COMMIT, true) => u64::from(self.process.map_or(0, |p| p.app_region)),
                (COMMIT, false) => self.committed(),
                _ => {
                    return Outcome::Unimplemented(format!(
                        "resource limit type {kind} ({})",
                        if limits { "limit" } else { "current" }
                    ))
                }
            };
            let at = out.wrapping_add(8 * i);
            if mem.write_u32(at, value as u32).and_then(|()| mem.write_u32(at.wrapping_add(4), (value >> 32) as u32)).is_err() {
                return Outcome::Unimplemented("resource limit values unwritable".into());
            }
        }
        cpu.r[0] = RESULT_SUCCESS;
        Outcome::Continue
    }

    /// SendSyncRequest on a session: the request is in the calling
    /// thread's command buffer; the reply is written over it.
    fn request(&mut self, cpu: &mut Cpu, mem: &mut Memory, service: String) -> Outcome {
        let buffer = cpu.tls + services::COMMAND_BUFFER;
        let message = match Message::read(mem, buffer) {
            Ok(m) => m,
            Err(f) => return Outcome::Unimplemented(format!("command buffer unreadable: {f}")),
        };
        let header = message.words[0];
        let mut wake = false;
        let reply = match service.as_str() {
            "srv:" => match srv::parse(&message) {
                srv::Request::RegisterClient => Some(srv::register_client_reply()),
                srv::Request::EnableNotification => {
                    // 3dbrew does not give the semaphore's maximum; any positive value works here
                    let h = self.open(Object::Semaphore { count: 0, max: 0x7FFF_FFFF });
                    Some(srv::handle_reply(0x2, srv::COPY_HANDLE, h))
                }
                srv::Request::GetServiceHandle(name) if self.services.contains(&name) => {
                    let h = self.open(Object::Session { service: name });
                    Some(srv::handle_reply(0x5, srv::MOVE_HANDLE, h))
                }
                srv::Request::GetServiceHandle(_) => Some(srv::error_reply(0x5, srv::RESULT_NOT_AUTHORIZED)),
                srv::Request::Other(_) => None,
            },
            "APT:A" | "APT:U" | "APT:S" => match apt::parse(&message) {
                apt::Request::GetLockHandle { flags } => {
                    let lock = match self.apt_lock {
                        Some(h) => h,
                        None => {
                            let h = self.open(Object::Mutex { owner: None, count: 0 });
                            self.apt_lock = Some(h);
                            h
                        }
                    };
                    Some(apt::lock_handle_reply(flags, lock))
                }
                apt::Request::Initialize { app_id, attributes } => {
                    // 3dbrew: the resume event is "triggered at start of
                    // execution". It does not give either event's reset
                    // type; one-shot is a choice.
                    let notification = self.open(Object::Event { signaled: false, reset: 0 });
                    let resume = self.open(Object::Event { signaled: true, reset: 0 });
                    self.apt = Some(Apt { app_id, attributes, notification, resume, parameter: Some(apt::Parameter::startup()) });
                    Some(apt::initialize_reply(notification, resume))
                }
                apt::Request::Parameter { glance, app_id, size, buffer } => {
                    let pending = self.apt.as_ref().filter(|a| a.app_id == app_id).and_then(|a| a.parameter.clone());
                    match pending {
                        Some(p) if p.data.len() as u32 <= size => {
                            for (i, &b) in p.data.iter().enumerate() {
                                if let Err(f) = mem.write_u8(buffer.wrapping_add(i as u32), b) {
                                    return Outcome::Unimplemented(format!("APT parameter buffer: {f}"));
                                }
                            }
                            // Glance clears it too for commands 8 and 9 (3dbrew)
                            if !glance || matches!(p.command, 8 | 9) {
                                if let Some(a) = self.apt.as_mut() {
                                    a.parameter = None;
                                }
                            }
                            Some(apt::parameter_reply(glance, &p, buffer))
                        }
                        // what NS replies with nothing pending is not known
                        _ => None,
                    }
                }
                // UnlockTransition(u32 transition): void, so nothing is
                // written to the output buffer. Nothing has locked one yet
                // (LockTransition is not implemented), so it is recorded only.
                apt::Request::AppletUtility { utility: apt::UNLOCK_TRANSITION, input_size: 4, input, .. } => {
                    match mem.read_u32(input) {
                        Ok(transition) => {
                            self.settings.insert((service.clone(), 0x004B_0007), vec![transition]);
                            Some(apt::applet_utility_reply(0))
                        }
                        Err(f) => return Outcome::Unimplemented(format!("AppletUtility input: {f}")),
                    }
                }
                // SleepIfShellClosed(): the emulated console's shell is open
                apt::Request::AppletUtility { utility: apt::SLEEP_IF_SHELL_CLOSED, .. } => Some(apt::applet_utility_reply(0)),
                apt::Request::AppletUtility { utility, .. } => {
                    return Outcome::Unimplemented(format!("{service} AppletUtility {utility}"));
                }
                apt::Request::Other(_) => None,
            },
            // SetThisSaveDataSecureValue(slot, u64 value): a value kept beside
            // the save, which the game compares on loading to notice a save
            // that was rolled back (3dbrew "FS:SetThisSaveDataSecureValue").
            // Kept in the save directory, as the save's own (R-047)
            "fs:USER" if header == 0x086E_00C0 => {
                let w = &message.words;
                let value = u64::from(w[3]) << 32 | u64::from(w[2]);
                let Some(root) = &self.save_dir else {
                    return Outcome::Unimplemented("SetThisSaveDataSecureValue without a save directory".into());
                };
                let file = root.join(format!("secure-value-{}", w[1]));
                if let Err(e) = std::fs::create_dir_all(root).and_then(|()| std::fs::write(&file, value.to_le_bytes())) {
                    return Outcome::Unimplemented(format!("{}: {e}", file.display()));
                }
                Some(services::result_reply(0x86E))
            }
            // GetThisSaveDataSecureValue(slot): result, u8 exists, u8 is a
            // game card, u64 value (3dbrew "FS:GetThisSaveDataSecureValue"),
            // read back from where Set… kept it. The dump is a cartridge
            "y2r:u" => match self.y2r.request(&message.words) {
                Some(y2r::Action::Reply(reply)) => Some(reply),
                // GetTransferEndEvent: one event, made on first request
                Some(y2r::Action::Event) => {
                    let event = match self.y2r.event {
                        Some(e) => e,
                        None => {
                            let e = self.open(Object::Event { signaled: false, reset: 0 });
                            self.y2r.event = Some(e);
                            e
                        }
                    };
                    Some(vec![services::header(0xF, 1, 2), 0, srv::COPY_HANDLE, event])
                }
                // StartConversion: done at once, then the end event fires
                Some(y2r::Action::Convert) => {
                    if let Err(what) = self.y2r.convert(mem) {
                        return Outcome::Unimplemented(what);
                    }
                    if let Some(event) = self.y2r.event {
                        if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&event) {
                            *signaled = true;
                            wake = true;
                        }
                    }
                    Some(services::result_reply(0x26))
                }
                None => None,
            },
            "fs:USER" if header == 0x086F_0040 => {
                let Some(root) = &self.save_dir else {
                    return Outcome::Unimplemented("GetThisSaveDataSecureValue without a save directory".into());
                };
                let file = root.join(format!("secure-value-{}", message.words[1]));
                let value = std::fs::read(&file).ok().and_then(|b| b.try_into().ok()).map(u64::from_le_bytes);
                let (exists, value) = (u32::from(value.is_some()), value.unwrap_or(0));
                Some(vec![services::header(0x86F, 5, 0), 0, exists, 1, value as u32, (value >> 32) as u32])
            }
            "fs:USER" => match fs::parse(&message) {
                fs::Request::InitializeWithSdkVersion { sdk_version } => {
                    self.settings.insert((service.clone(), header), vec![sdk_version]);
                    Some(services::result_reply(0x861))
                }
                fs::Request::OpenFileDirectly { archive, archive_path, file_path, flags } => {
                    let path = match mem.read_range(file_path.address, file_path.size.min(0x400)) {
                        Ok(p) => p,
                        Err(f) => return Outcome::Unimplemented(format!("OpenFileDirectly path: {f}")),
                    };
                    let mut return_reply: Option<Vec<u32>> = None;
                    let main_romfs = archive == fs::ARCHIVE_SELF_NCCH
                        && archive_path.kind == fs::PATH_EMPTY
                        && file_path.kind == fs::PATH_BINARY
                        && path.len() == 12
                        && path.iter().all(|&b| b == 0)
                        && flags == fs::OPEN_READ;
                    // another title's contents (archive 0x2345678A, "title
                    // access"): the console's system data, e.g. the NGWord
                    // list. Not part of the game dump and never shipped: not
                    // found (R-034)
                    if archive == 0x2345_678A {
                        let archive_bytes = mem.read_range(archive_path.address, archive_path.size.min(0x10)).unwrap_or_default();
                        let title = archive_bytes.get(..8).map_or(0, |b| u64::from_le_bytes(b.try_into().expect("8")));
                        self.missing_titles.push(title);
                        return_reply = Some(vec![services::header(0x803, 1, 0), fs::RESULT_NOT_FOUND]);
                    }
                    match &self.romfs {
                        _ if return_reply.is_some() => return_reply.take(),
                        Some(image) if main_romfs => {
                            let size = image.size();
                            let h = self.open(Object::RomFsFile { start: 0, size });
                            Some(fs::open_file_reply(h))
                        }
                        _ => {
                            let archive_bytes =
                                mem.read_range(archive_path.address, archive_path.size.min(0x40)).unwrap_or_default();
                            return Outcome::Unimplemented(format!(
                                "OpenFileDirectly(archive {archive:#x}, archive path type {} {:02x?}, file path type {} {:02x?}, flags {flags:#x})",
                                archive_path.kind, archive_bytes, file_path.kind, path
                            ));
                        }
                    }
                }
                // GetPriority: what SetPriority (0x0862) last set; 3dbrew
                // does not give the default, so it is not invented
                fs::Request::Other(0x0863_0000) => {
                    self.settings.get(&(service.clone(), 0x0862_0040)).map(|w| vec![services::header(0x863, 2, 0), 0, w[0]])
                }
                // IsSdmcDetected: result, bool. The emulated console has an
                // SD card: Ultra Sun treats "not detected" as fatal (it
                // throws 0xC88044AA from 0x00168AD4; R-028)
                fs::Request::Other(0x0817_0000) => Some(vec![services::header(0x817, 2, 0), 0, 1]),
                // GetFormatInfo of save data or extra data: what formatting
                // or creation recorded; none yet is "not formatted" (save
                // data) or "not found" (extra data), R-029
                fs::Request::GetFormatInfo { archive, path } => {
                    let bytes = mem.read_range(path.address, path.size.min(0x100)).unwrap_or_default();
                    let (Some(named), Some(root)) = (fs::Archive::named(archive, path.kind, &bytes), &self.save_dir) else {
                        return Outcome::Unimplemented(format!("GetFormatInfo(archive {archive:#x}, path type {})", path.kind));
                    };
                    match fs::Format::load(&named.dir(root)) {
                        Some(f) => {
                            Some(vec![services::header(0x845, 5, 0), 0, f.size, f.directories, f.files, u32::from(f.duplicate)])
                        }
                        None if named == fs::Archive::SaveData => {
                            Some(vec![services::header(0x845, 1, 0), fs::RESULT_NOT_FORMATTED])
                        }
                        None => Some(vec![services::header(0x845, 1, 0), fs::RESULT_NOT_FOUND]),
                    }
                }
                // OpenArchive: result, u64 handle. Missing save data is "not
                // formatted", missing extra data "not found" (R-029)
                fs::Request::OpenArchive { archive, path } => {
                    let bytes = mem.read_range(path.address, path.size.min(0x100)).unwrap_or_default();
                    let (Some(named), Some(root)) = (fs::Archive::named(archive, path.kind, &bytes), &self.save_dir) else {
                        return Outcome::Unimplemented(format!(
                            "OpenArchive(archive {archive:#x}, path type {} {bytes:02x?})",
                            path.kind
                        ));
                    };
                    let dir = named.dir(root);
                    let exists = match named {
                        fs::Archive::SaveData => fs::Format::load(&dir).is_some(),
                        fs::Archive::ExtSaveData { .. } => dir.is_dir(),
                    };
                    if exists {
                        self.next_archive += 1;
                        let h = self.next_archive;
                        self.archives.insert(h, named);
                        Some(vec![services::header(0x80C, 3, 0), 0, h as u32, (h >> 32) as u32])
                    } else {
                        let result = if named == fs::Archive::SaveData { fs::RESULT_NOT_FORMATTED } else { fs::RESULT_NOT_FOUND };
                        Some(vec![services::header(0x80C, 1, 0), result])
                    }
                }
                // FormatSaveData (archive 4): the directory is emptied and the
                // format recorded; the size is the exheader's save data size
                // (a hypothesis, R-031)
                fs::Request::FormatSaveData { archive: fs::ARCHIVE_SAVE_DATA, path, directories, files, duplicate }
                    if path.kind == fs::PATH_EMPTY =>
                {
                    let (Some(root), Some(p)) = (&self.save_dir, self.process) else {
                        return Outcome::Unimplemented("save data without a save directory".into());
                    };
                    let dir = fs::Archive::SaveData.dir(root);
                    let _ = std::fs::remove_dir_all(&dir);
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        return Outcome::Unimplemented(format!("FormatSaveData: {e}"));
                    }
                    let format = fs::Format { size: p.save_data_size as u32, directories, files, duplicate };
                    if let Err(e) = format.store(&dir) {
                        return Outcome::Unimplemented(format!("FormatSaveData: {e}"));
                    }
                    Some(services::result_reply(0x84C))
                }
                fs::Request::FormatSaveData { archive, .. } => {
                    return Outcome::Unimplemented(format!("FormatSaveData(archive {archive:#x})"))
                }
                fs::Request::DeleteFile { archive, path } => match self.archive_file(mem, archive, path) {
                    Ok(host) => {
                        let result = if std::fs::remove_file(&host).is_ok() { RESULT_SUCCESS } else { fs::RESULT_NOT_FOUND };
                        Some(vec![services::header(0x804, 1, 0), result])
                    }
                    Err(what) => return Outcome::Unimplemented(what),
                },
                // CreateFile: zero-filled, of the size asked
                fs::Request::CreateFile { archive, path, size } => match self.archive_file(mem, archive, path) {
                    Ok(host) => {
                        let made =
                            std::fs::OpenOptions::new().write(true).create_new(true).open(&host).and_then(|f| f.set_len(size));
                        let result = match made {
                            Ok(()) => RESULT_SUCCESS,
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => fs::RESULT_ALREADY_EXISTS,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::RESULT_NOT_FOUND,
                            Err(e) => return Outcome::Unimplemented(format!("CreateFile {}: {e}", host.display())),
                        };
                        Some(vec![services::header(0x808, 1, 0), result])
                    }
                    Err(what) => return Outcome::Unimplemented(what),
                },
                // OpenFile: a session; a missing file without the create flag
                // is "not found"
                fs::Request::OpenFile { archive, path, flags } => match self.archive_file(mem, archive, path) {
                    Ok(host) => {
                        if !host.is_file() && flags & fs::OPEN_CREATE != 0 {
                            if let Err(e) = std::fs::File::create(&host) {
                                return Outcome::Unimplemented(format!("OpenFile {}: {e}", host.display()));
                            }
                        }
                        if host.is_file() {
                            let h = self.open(Object::SaveFile { path: host, flags });
                            Some(fs::open_file_reply_for(0x802, h))
                        } else {
                            Some(vec![services::header(0x802, 1, 0), fs::RESULT_NOT_FOUND])
                        }
                    }
                    Err(what) => return Outcome::Unimplemented(what),
                },
                // ControlArchive(u64 archive, action, ...): action 0 commits
                // save data. Writes already go to the host file, so there is
                // nothing to commit; a real save store will need it (R-031)
                fs::Request::Other(0x080D_0144) if message.words[3] == 0 => {
                    let archive = u64::from(message.words[2]) << 32 | u64::from(message.words[1]);
                    if !self.archives.contains_key(&archive) {
                        return Outcome::Unimplemented(format!("ControlArchive on {archive:#x}, which is not open"));
                    }
                    self.save_commits += 1;
                    Some(services::result_reply(0x80D))
                }
                // CreateExtSaveData(info: u8 media, ..., u64 ID; directories,
                // files, u64 size limit, icon size; the icon buffer): the
                // archive's directory, and its icon beside it (3dbrew
                // "FS:CreateExtSaveData")
                fs::Request::Other(0x0851_0242) if message.words[10] == (message.words[9] << 4) | 0xA => {
                    let w = &message.words;
                    let named = fs::Archive::ExtSaveData { media: w[1] & 0xFF, id: u64::from(w[3]) << 32 | u64::from(w[2]) };
                    let Some(root) = &self.save_dir else {
                        return Outcome::Unimplemented("extdata without a save directory".into());
                    };
                    let dir = named.dir(root);
                    let result = if dir.is_dir() {
                        fs::RESULT_ALREADY_EXISTS
                    } else {
                        let icon = match mem.read_range(w[11], w[9].min(0x10_0000)) {
                            Ok(b) => b,
                            Err(f) => return Outcome::Unimplemented(format!("CreateExtSaveData icon: {f}")),
                        };
                        let format = fs::Format { size: w[7], directories: w[5], files: w[6], duplicate: false };
                        let made = std::fs::create_dir_all(&dir)
                            .and_then(|()| std::fs::write(dir.with_extension("icon"), icon))
                            .and_then(|()| format.store(&dir));
                        if let Err(e) = made {
                            return Outcome::Unimplemented(format!("CreateExtSaveData {}: {e}", dir.display()));
                        }
                        RESULT_SUCCESS
                    };
                    Some(vec![services::header(0x851, 1, 0), result])
                }
                fs::Request::CreateDirectory { archive, path } => match self.archive_file(mem, archive, path) {
                    Ok(host) => {
                        let result = match std::fs::create_dir(&host) {
                            Ok(()) => RESULT_SUCCESS,
                            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => fs::RESULT_ALREADY_EXISTS,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::RESULT_NOT_FOUND,
                            Err(e) => return Outcome::Unimplemented(format!("CreateDirectory {}: {e}", host.display())),
                        };
                        Some(vec![services::header(0x809, 1, 0), result])
                    }
                    Err(what) => return Outcome::Unimplemented(what),
                },
                fs::Request::OpenDirectory { archive, path } => match self.archive_file(mem, archive, path) {
                    Ok(host) => match fs::list(&host) {
                        Ok(entries) => {
                            let h = self.open(Object::Directory { entries, read: 0 });
                            Some(fs::open_file_reply_for(0x80B, h))
                        }
                        Err(_) => Some(vec![services::header(0x80B, 1, 0), fs::RESULT_NOT_FOUND]),
                    },
                    Err(what) => return Outcome::Unimplemented(what),
                },
                fs::Request::CloseArchive(handle) => {
                    if self.archives.remove(&handle).is_none() {
                        return Outcome::Unimplemented(format!("CloseArchive of {handle:#x}, which is not open"));
                    }
                    Some(services::result_reply(0x80E))
                }
                fs::Request::Other(_) => None,
            },
            "cfg:u" | "cfg:s" | "cfg:i" => match cfg::parse(&message) {
                cfg::Request::GetConfigInfoBlk2 { size, id, buffer: out } => match self.console.block(id) {
                    Some(bytes) if bytes.len() as u32 == size => match mem.write_range(out, &bytes) {
                        Ok(()) => Some(cfg::block_reply(0x1, size, out)),
                        Err(f) => return Outcome::Unimplemented(format!("config block into {out:#x}: {f}")),
                    },
                    _ => return Outcome::Unimplemented(format!("config block {id:#010x} ({size} bytes)")),
                },
                cfg::Request::SecureInfoGetRegion => Some(vec![services::header(0x2, 2, 0), 0, u32::from(self.console.region)]),
                cfg::Request::GetRegionCanadaUsa => {
                    Some(vec![services::header(0x4, 2, 0), 0, u32::from(self.console.region_canada_usa())])
                }
                cfg::Request::GenHashConsoleUnique(salt) => {
                    let (low, high) = self.console.hash_console_unique(salt);
                    Some(vec![services::header(0x3, 3, 0), 0, low, high])
                }
                cfg::Request::Other(_) => None,
            },
            "dsp::DSP" => match dsp::parse(&message) {
                dsp::Request::LoadComponent { size, buffer: at } => match mem.read_range(at, size.min(0x10_0000)) {
                    Ok(firmware) => {
                        self.dsp_firmware = Some((firmware.len(), usum_import::sha256_hex(&firmware)));
                        Some(dsp::load_component_reply(size, at))
                    }
                    Err(f) => return Outcome::Unimplemented(format!("DSP component at {at:#x}: {f}")),
                },
                // recorded; nothing signals these until audio is emulated
                dsp::Request::RegisterInterruptEvents { interrupt, channel, event } => {
                    if event == 0 {
                        self.dsp_events.remove(&(interrupt, channel));
                    } else {
                        self.dsp_events.insert((interrupt, channel), event);
                        self.next_audio_frame = self.now() + AUDIO_FRAME_NS;
                    }
                    Some(services::result_reply(0x15))
                }
                // GetSemaphoreEventHandle: result, copy descriptor, event.
                // 3dbrew gives no layout or reset type; one event per
                // process, one-shot (choices)
                dsp::Request::Other(0x0016_0000) => {
                    let event = match self.dsp_semaphore_event {
                        Some(h) => h,
                        None => {
                            let h = self.open(Object::Event { signaled: false, reset: 0 });
                            self.dsp_semaphore_event = Some(h);
                            h
                        }
                    };
                    Some(srv::handle_reply(0x16, srv::COPY_HANDLE, event))
                }
                dsp::Request::WriteProcessPipe { channel, size, buffer: at } => match mem.read_range(at, size.min(0x1000)) {
                    Ok(bytes) => {
                        self.dsp_pipe_writes.push((channel, bytes.clone()));
                        match self.dsp_pipes.write(channel, &bytes) {
                            Ok(answered) => {
                                // data to read: the pipe's interrupt event fires
                                if let Some(&event) = self.dsp_events.get(&(dsp::PIPE_INTERRUPT, channel)).filter(|_| answered) {
                                    if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&event) {
                                        *signaled = true;
                                    }
                                    wake = true;
                                }
                                Some(services::result_reply(0xD))
                            }
                            Err(what) => return Outcome::Unimplemented(what),
                        }
                    }
                    Err(f) => return Outcome::Unimplemented(format!("DSP pipe data at {at:#x}: {f}")),
                },
                dsp::Request::ReadPipeIfPossible { channel, peer: 0, size, buffer: at } => {
                    let bytes = self.dsp_pipes.read(channel, size);
                    match mem.write_range(at, &bytes) {
                        Ok(()) => Some(vec![services::header(0x10, 2, 0), 0, bytes.len() as u32]),
                        Err(f) => return Outcome::Unimplemented(format!("DSP pipe read into {at:#x}: {f}")),
                    }
                }
                dsp::Request::ReadPipeIfPossible { channel, peer, .. } => {
                    return Outcome::Unimplemented(format!("DSP pipe {channel} read from peer {peer}"));
                }
                // FlushDataCache / InvalidateDCache(address, size, process):
                // cache maintenance, nothing to do on coherent host memory
                dsp::Request::Other(h @ (0x0013_0082 | 0x0014_0082))
                    if message.words[3] == srv::COPY_HANDLE && message.words[4] == CURRENT_PROCESS =>
                {
                    Some(services::result_reply((h >> 16) as u16))
                }
                dsp::Request::ConvertProcessAddressFromDspDram(word) => {
                    Some(vec![services::header(0xC, 2, 0), 0, dsp::arm_address(word)])
                }
                dsp::Request::Other(_) => None,
            },
            "ldr:ro" => match ro::parse(&message) {
                ro::Request::Initialize { buffer: at, size, address } => {
                    if self.crs.is_some() {
                        return Outcome::Unimplemented("ldr:ro Initialize twice".into());
                    }
                    let bytes = match mem.read_range(at, size) {
                        Ok(b) => b,
                        Err(f) => return Outcome::Unimplemented(format!("CRS at {at:#x}: {f}")),
                    };
                    let crs = match usum_import::cro::parse(&bytes) {
                        Ok(c) if c.is_crs => c,
                        Ok(_) => return Outcome::Unimplemented("ldr:ro Initialize with a CRO, not a CRS".into()),
                        Err(e) => return Outcome::Unimplemented(format!("CRS: {e}")),
                    };
                    let end = self.process.map_or(0, |p| p.executable_end);
                    if address != ro::mapped_address(at, end) {
                        return Outcome::Unimplemented(format!("CRS mapped at {address:#x}, not where 3dbrew's rule puts it"));
                    }
                    // the CRS is read by RO, not executed: mapped read-only
                    if let Err(what) = self.alias(mem, at, size, address, Perm::R) {
                        return Outcome::Unimplemented(what);
                    }
                    let segments = crate::ro::place(&crs, 0, 0, 0);
                    let unresolved = (0..crs.imports.len()).collect();
                    self.modules.push(crate::ro::Loaded {
                        cro: crs.clone(),
                        segments,
                        base: 0,
                        image: address,
                        unresolved,
                        sha256: String::new(),
                    });
                    self.crs = Some((address, crs));
                    Some(services::result_reply(0x1))
                }
                // 3dbrew: RO makes the buffer read-only, then verifies the
                // CRR (an RSA signature). Not verified here: the dump's
                // hashes were checked when it was imported.
                ro::Request::LoadCrr { buffer: at, size } => {
                    if self.crr.is_some() {
                        return Outcome::Unimplemented("a second CRR (RO holds one at a time)".into());
                    }
                    match self.protect(mem, at, size, Perm::R) {
                        Ok(()) => {
                            self.crr = Some((at, size));
                            Some(services::result_reply(0x2))
                        }
                        Err(what) => return Outcome::Unimplemented(what),
                    }
                }
                ro::Request::LoadCro(load) => {
                    match self.load_cro(mem, load) {
                        Ok(()) => {
                            self.settings.insert(
                                (service.clone(), 0x0009_0000 | self.modules.len() as u32),
                                vec![u32::from(load.auto_link), load.fix_level],
                            );
                            // the size after fixing: what the fix level frees is not
                            // modelled, so the whole module (R-030)
                            Some(vec![services::header(0x9, 2, 0), 0, load.size])
                        }
                        Err(what) => return Outcome::Unimplemented(what),
                    }
                }
                ro::Request::UnloadCro { address, buffer: back } => {
                    let Some(i) = self.modules.iter().position(|m| m.base == address && address != 0) else {
                        return Outcome::Unimplemented(format!("UnloadCRO of {address:#x}, which is not loaded"));
                    };
                    let gone = self.modules.remove(i);
                    if let Err(what) = crate::ro::unrebase_header(mem, &gone) {
                        return Outcome::Unimplemented(what);
                    }
                    if let Err(what) = crate::ro::forget(mem, &mut self.modules, &gone) {
                        return Outcome::Unimplemented(what);
                    }
                    let len = gone.cro.file_size.next_multiple_of(0x1000);
                    if let Err(what) = self.unalias(mem, address, len, back) {
                        return Outcome::Unimplemented(what);
                    }
                    Some(services::result_reply(0x5))
                }
                ro::Request::Other(_) => None,
            },
            "gsp::Gpu" => match gsp::parse(&message) {
                gsp::Request::AcquireRight { flags } => {
                    self.settings.insert((service.clone(), header), vec![flags]);
                    Some(services::result_reply(0x16))
                }
                gsp::Request::RegisterInterruptRelayQueue { flags, event } => {
                    if self.gsp.is_some() {
                        return Outcome::Unimplemented("a second GSP interrupt relay queue".into());
                    }
                    let memory = self.open(Object::SharedMemory { size: gsp::SHARED_MEMORY_SIZE, mapped: None });
                    self.gsp = Some(Gsp { flags, event, memory });
                    self.next_vblank = self.now() + VBLANK_NS;
                    // 3dbrew does not define the result; success (a choice)
                    Some(gsp::register_reply(0, memory))
                }
                // TriggerCmdReqQueue: GSP processes the GX command queue
                gsp::Request::Other(0x000C_0000) => match self.gx_commands(mem) {
                    Ok(raised) => {
                        wake |= raised;
                        Some(services::result_reply(0xC))
                    }
                    Err(what) => return Outcome::Unimplemented(what),
                },
                // FlushDataCache, InvalidateDataCache, StoreDataCache
                // (address, size, process): nothing to do on coherent host memory
                gsp::Request::Other(h @ (0x0008_0082 | 0x0009_0082 | 0x001F_0082))
                    if message.words[3] == srv::COPY_HANDLE && message.words[4] == CURRENT_PROCESS =>
                {
                    Some(services::result_reply((h >> 16) as u16))
                }
                gsp::Request::Other(_) => None,
            },
            // GetIPCHandles: one set per process, made on first request.
            // 3dbrew gives no reset types for the events; one-shot (a choice)
            "hid:USER" | "hid:SPVR" if header == 0x000A_0000 => {
                let (memory, events) = match self.hid {
                    Some(h) => h,
                    None => {
                        let memory = self.open(Object::SharedMemory { size: hid::SHARED_MEMORY_SIZE, mapped: None });
                        let events = [0; 5].map(|_| self.open(Object::Event { signaled: false, reset: 0 }));
                        self.hid = Some((memory, events));
                        (memory, events)
                    }
                };
                Some(hid::ipc_handles_reply(memory, events))
            }
            // InitializeWithVersion(shared memory size, NodeInfo, version,
            // shared memory): result, 0, a copy descriptor, the event
            // signalled when the connection status changes (3dbrew
            // "NWMUDS:InitializeWithVersion"). No other console is ever in
            // range, so the event is never signalled; one-shot (a choice)
            // GenerateRandomData(size, buffer): `size` random bytes into a
            // mapped buffer (3dbrew "SSLC:GenerateRandomData")
            "ssl:C" if header == 0x0011_0042 && message.words[2] & 0xF == 0xC => {
                let (size, buffer) = (message.words[1], message.words[3]);
                if size > 0x1000 || message.words[2] >> 4 < size {
                    return Outcome::Unimplemented(format!("GenerateRandomData of {size:#x} bytes"));
                }
                let mut bytes = Vec::with_capacity(size as usize);
                while bytes.len() < size as usize {
                    // xorshift64*, seeded on first use
                    if self.random == 0 {
                        self.random = 0x2545_F491_4F6C_DD1D;
                    }
                    self.random ^= self.random >> 12;
                    self.random ^= self.random << 25;
                    self.random ^= self.random >> 27;
                    bytes.extend(self.random.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
                }
                bytes.truncate(size as usize);
                if let Err(f) = mem.write_range(buffer, &bytes) {
                    return Outcome::Unimplemented(format!("GenerateRandomData buffer: {f}"));
                }
                Some(vec![services::header(0x11, 1, 2), 0, message.words[2], buffer])
            }
            "nwm::UDS" if header == 0x001B_0302 && message.words[13] == srv::COPY_HANDLE => {
                if self.uds.is_some() {
                    return Outcome::Unimplemented("nwm::UDS initialized twice".into());
                }
                let event = self.open(Object::Event { signaled: false, reset: 0 });
                self.uds = Some(uds::Uds { event, status: uds::Status::idle() });
                Some(vec![services::header(0x1B, 1, 2), 0, srv::COPY_HANDLE, event])
            }
            // CreateNetwork2(passphrase size, NetworkStruct, passphrase), both
            // static buffers: host alone. The status becomes "host" and the
            // status event fires, as for any status change (a choice: 3dbrew
            // does not say whether hosting signals it)
            "nwm::UDS" if header == 0x001D_0044 && self.uds.is_some() => {
                let w = &message.words;
                let size = w[2] >> 14;
                let max_nodes = match mem.read_range(w[3], size) {
                    Ok(b) if b.len() > uds::NETWORK_MAX_NODES => b[uds::NETWORK_MAX_NODES],
                    _ => return Outcome::Unimplemented(format!("CreateNetwork2 NetworkStruct at {:#x}, size {size:#x}", w[3])),
                };
                let u = self.uds.as_mut().expect("checked");
                u.status = uds::Status::host(max_nodes);
                let event = u.event;
                if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&event) {
                    *signaled = true;
                }
                wake = true;
                Some(services::result_reply(0x1D))
            }
            // DestroyNetwork: stop hosting; back to idle, and the status
            // event fires as for any status change (as on CreateNetwork2)
            "nwm::UDS" if header == 0x0008_0000 && self.uds.is_some_and(|u| u.status.status == uds::STATUS_HOST) => {
                let u = self.uds.as_mut().expect("checked");
                u.status = uds::Status::idle();
                let event = u.event;
                if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&event) {
                    *signaled = true;
                }
                wake = true;
                Some(services::result_reply(0x8))
            }
            // StartScan(max size, ScanInputStruct, wlancommID, id8, 0, event,
            // output buffer): no beacon is ever received, so the reply
            // (3dbrew "NWM Services" BeaconDataReply) is the 0xC-byte header
            // with no entries. Only when not connected (3dbrew). The event
            // is signalled as the scan is done at once (a choice)
            "nwm::UDS"
                if header == 0x000F_0404
                    && self.uds.is_some_and(|u| u.status.status == uds::STATUS_IDLE)
                    && message.words[17] == srv::COPY_HANDLE
                    && message.words[19] & 0xF == 0xC =>
            {
                let w = &message.words;
                let (max, event, size, buffer) = (w[1], w[18], w[19] >> 4, w[20]);
                if max.min(size) < 0xC {
                    return Outcome::Unimplemented(format!("StartScan into {size:#x} bytes, max {max:#x}"));
                }
                let reply: Vec<u8> = [max, 0xC, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
                if let Err(f) = mem.write_range(buffer, &reply) {
                    return Outcome::Unimplemented(format!("StartScan output: {f}"));
                }
                if let Some(Object::Event { signaled, .. }) = self.handles.get_mut(&event) {
                    *signaled = true;
                    wake = true;
                }
                Some(services::result_reply(0xF))
            }
            // Finalize: local wireless off; InitializeWithVersion may follow
            // again. The event handle stays the client's to close
            "nwm::UDS" if header == 0x0003_0000 && self.uds.is_some() => {
                self.uds = None;
                Some(services::result_reply(0x3))
            }
            // GetConnectionStatus: result, the 0x30-byte status. Reading it
            // clears changed_nodes (a choice: the bitmask names what changed
            // since the last event)
            "nwm::UDS" if header == 0x000B_0000 && self.uds.is_some() => {
                let u = self.uds.as_mut().expect("checked");
                let words = u.status.words();
                u.status.changed_nodes = 0;
                let mut reply = vec![services::header(0xB, 13, 0), 0];
                reply.extend(words);
                Some(reply)
            }
            // GetMyFriendKey: no account, all zero (a choice)
            "frd:u" | "frd:a" if header == 0x0005_0000 => Some(vec![services::header(0x5, 5, 0), 0, 0, 0, 0, 0]),
            // GetFriendKeyList(offset, count): no friends, 0 entries
            "frd:u" | "frd:a" if header == 0x0011_0080 => Some(vec![services::header(0x11, 2, 2), 0, 0, 2, message.statics[1]]),
            // boss RegisterStorageEntry(u64 extdata ID, size, u16 entry,
            // u8 media) and GetStorageEntryInfo (size, entry; none
            // registered: 0 and 0, a choice). Kept in memory only: BOSS
            // (SpotPass) itself is not emulated.
            "boss:U" if header == 0x002F_0140 => {
                let w = &message.words;
                self.boss_storage = Some((u64::from(w[2]) << 32 | u64::from(w[1]), w[3], w[4] & 0xFFFF, w[5] & 0xFF));
                Some(services::result_reply(0x2F))
            }
            "boss:U" if header == 0x0030_0000 => {
                let (size, entry) = self.boss_storage.map_or((0, 0), |s| (s.1, s.2));
                Some(vec![services::header(0x30, 3, 0), 0, size, entry])
            }
            // GetMyPresence: offline, all zero, into static buffer 0
            "frd:u" | "frd:a" if header == 0x0008_0000 => {
                let at = message.statics[1];
                match mem.write_range(at, &[0; frd::MY_PRESENCE_SIZE as usize]) {
                    Ok(()) => Some(frd::static_reply(0x8, frd::MY_PRESENCE_SIZE, at)),
                    Err(f) => return Outcome::Unimplemented(format!("GetMyPresence into {at:#x}: {f}")),
                }
            }
            // any request to err:f reports a fatal error: stop and show it
            "err:f" => {
                let words: Vec<String> = message.words[..32].iter().map(|w| format!("{w:08x}")).collect();
                return Outcome::Unimplemented(format!(
                    "the game threw a fatal error (err:f {header:#010x}): {}",
                    words.join(" ")
                ));
            }
            _ => None,
        };
        let length = ((header >> 6) & 0x3F) + (header & 0x3F);
        let words = message.words[1..=length.min(services::COMMAND_WORDS as u32 - 1) as usize].to_vec();
        let reply = reply.or_else(|| {
            services::settings::lookup(&service, &message.words)?;
            self.settings.insert((service.clone(), header), words.clone());
            Some(services::result_reply((header >> 16) as u16))
        });
        self.requests.push(Request { service: service.clone(), header, words, handled: reply.is_some(), vblank: self.vblanks });
        match reply {
            Some(words) => match Message::reply(mem, buffer, &words) {
                Ok(()) => {
                    cpu.r[0] = RESULT_SUCCESS;
                    // a service signaled an event: waiters may wake
                    if wake {
                        self.wake()
                    } else {
                        Outcome::Continue
                    }
                }
                Err(f) => Outcome::Unimplemented(format!("command buffer unwritable: {f}")),
            },
            None => Outcome::Unimplemented(format!("{service} command {header:#010x}")),
        }
    }

    /// A request on a directory session: Read (0x0801) and Close (0x0802).
    fn directory_request(&mut self, cpu: &mut Cpu, mem: &mut Memory, handle: u32) -> Outcome {
        let buffer = cpu.tls + services::COMMAND_BUFFER;
        let message = match Message::read(mem, buffer) {
            Ok(m) => m,
            Err(f) => return Outcome::Unimplemented(format!("command buffer unreadable: {f}")),
        };
        let w = message.words;
        let reply = match w[0] {
            0x0801_0042 if w[2] == ((w[1] * fs::DIRECTORY_ENTRY_SIZE as u32) << 4) | 0xC => {
                let Some(Object::Directory { entries, read }) = self.handles.get_mut(&handle) else {
                    unreachable!("dispatched on it")
                };
                let take: Vec<fs::Entry> = entries.iter().skip(*read).take(w[1] as usize).cloned().collect();
                *read += take.len();
                let bytes: Vec<u8> = take.iter().flat_map(fs::Entry::encode).collect();
                if let Err(f) = mem.write_range(w[3], &bytes) {
                    return Outcome::Unimplemented(format!("directory entries into {:#x}: {f}", w[3]));
                }
                Some(vec![services::header(0x801, 2, 0), 0, take.len() as u32])
            }
            0x0802_0000 => {
                self.handles.remove(&handle);
                Some(services::result_reply(0x802))
            }
            _ => None,
        };
        self.requests.push(Request {
            service: "directory".into(),
            header: w[0],
            words: w[1..4].to_vec(),
            handled: reply.is_some(),
            vblank: self.vblanks,
        });
        match reply {
            Some(words) => match Message::reply(mem, buffer, &words) {
                Ok(()) => {
                    cpu.r[0] = RESULT_SUCCESS;
                    Outcome::Continue
                }
                Err(f) => Outcome::Unimplemented(format!("command buffer unwritable: {f}")),
            },
            None => Outcome::Unimplemented(format!("directory command {:#010x}", w[0])),
        }
    }

    /// A request on a save file session: served from the host file.
    fn save_file_request(&mut self, cpu: &mut Cpu, mem: &mut Memory, handle: u32, path: &std::path::Path, flags: u32) -> Outcome {
        use std::os::unix::fs::FileExt;
        let buffer = cpu.tls + services::COMMAND_BUFFER;
        let message = match Message::read(mem, buffer) {
            Ok(m) => m,
            Err(f) => return Outcome::Unimplemented(format!("command buffer unreadable: {f}")),
        };
        let header = message.words[0];
        let io = |e: std::io::Error| Outcome::Unimplemented(format!("save file {}: {e}", path.display()));
        let open = |write: bool| std::fs::OpenOptions::new().read(true).write(write).open(path);
        let reply = match fs::parse_file(&message) {
            fs::FileRequest::Read { offset, size, buffer: out } => {
                let file = match open(false) {
                    Ok(f) => f,
                    Err(e) => return io(e),
                };
                let length = file.metadata().map(|m| m.len()).unwrap_or(0);
                let n = u64::from(size).min(length.saturating_sub(offset)) as usize;
                let mut bytes = vec![0; n];
                if let Err(e) = file.read_exact_at(&mut bytes, offset) {
                    return io(e);
                }
                if let Err(f) = mem.write_range(out, &bytes) {
                    return Outcome::Unimplemented(format!("save read into {out:#x}: {f}"));
                }
                self.save_bytes_read += n as u64;
                Some(fs::read_reply(n as u32))
            }
            fs::FileRequest::Write { offset, size, buffer: from } if flags & fs::OPEN_WRITE != 0 => {
                let bytes = match mem.read_range(from, size) {
                    Ok(b) => b,
                    Err(f) => return Outcome::Unimplemented(format!("save write from {from:#x}: {f}")),
                };
                if let Err(e) = open(true).and_then(|f| f.write_all_at(&bytes, offset)) {
                    return io(e);
                }
                self.save_bytes_written += u64::from(size);
                Some(vec![services::header(0x803, 2, 0), 0, size])
            }
            fs::FileRequest::GetSize => match std::fs::metadata(path) {
                Ok(m) => Some(fs::size_reply(m.len())),
                Err(e) => return io(e),
            },
            fs::FileRequest::SetSize(n) if flags & fs::OPEN_WRITE != 0 => match open(true).and_then(|f| f.set_len(n)) {
                Ok(()) => Some(services::result_reply(0x805)),
                Err(e) => return io(e),
            },
            fs::FileRequest::Close => {
                self.handles.remove(&handle);
                Some(services::result_reply(0x808))
            }
            fs::FileRequest::Flush => Some(services::result_reply(0x809)),
            fs::FileRequest::SetPriority(_) => Some(services::result_reply(0x80A)),
            fs::FileRequest::Write { .. } | fs::FileRequest::SetSize(_) => {
                return Outcome::Unimplemented(format!("a write to {} opened read-only", path.display()))
            }
            fs::FileRequest::Other(_) => None,
        };
        let length = (((header >> 6) & 0x3F) + (header & 0x3F)).min(services::COMMAND_WORDS as u32 - 1);
        let words = message.words[1..=length as usize].to_vec();
        self.requests.push(Request { service: "savefile".into(), header, words, handled: reply.is_some(), vblank: self.vblanks });
        match reply {
            Some(words) => match Message::reply(mem, buffer, &words) {
                Ok(()) => {
                    cpu.r[0] = RESULT_SUCCESS;
                    Outcome::Continue
                }
                Err(f) => Outcome::Unimplemented(format!("command buffer unwritable: {f}")),
            },
            None => Outcome::Unimplemented(format!("save file command {header:#010x}")),
        }
    }

    /// A request on a RomFS file session.
    fn file_request(&mut self, cpu: &mut Cpu, mem: &mut Memory, handle: u32, start: u64, size: u64) -> Outcome {
        let buffer = cpu.tls + services::COMMAND_BUFFER;
        let message = match Message::read(mem, buffer) {
            Ok(m) => m,
            Err(f) => return Outcome::Unimplemented(format!("command buffer unreadable: {f}")),
        };
        let header = message.words[0];
        let reply = match fs::parse_file(&message) {
            fs::FileRequest::Read { offset, size: len, buffer: out } => {
                let Some(image) = &self.romfs else { return Outcome::Unimplemented("RomFS read without an image".into()) };
                let len = u64::from(len).min(size.saturating_sub(offset)) as u32;
                match image.read(start + offset, len) {
                    Ok(bytes) => match mem.write_range(out, &bytes) {
                        Ok(()) => {
                            self.romfs_read += bytes.len() as u64;
                            Some(fs::read_reply(bytes.len() as u32))
                        }
                        Err(f) => return Outcome::Unimplemented(format!("RomFS read into {out:#x}: {f}")),
                    },
                    Err(e) => return Outcome::Unimplemented(format!("RomFS read from the dump failed: {e}")),
                }
            }
            fs::FileRequest::GetSize => Some(fs::size_reply(size)),
            fs::FileRequest::Close => {
                self.handles.remove(&handle);
                Some(services::result_reply(0x808))
            }
            fs::FileRequest::Flush => Some(services::result_reply(0x809)),
            fs::FileRequest::SetPriority(_) => Some(services::result_reply(0x80A)),
            fs::FileRequest::Write { .. } | fs::FileRequest::SetSize(_) => {
                return Outcome::Unimplemented("a write to the RomFS".into())
            }
            fs::FileRequest::Other(_) => None,
        };
        let length = (((header >> 6) & 0x3F) + (header & 0x3F)).min(services::COMMAND_WORDS as u32 - 1);
        let words = message.words[1..=length as usize].to_vec();
        self.requests.push(Request { service: "file".into(), header, words, handled: reply.is_some(), vblank: self.vblanks });
        match reply {
            Some(words) => match Message::reply(mem, buffer, &words) {
                Ok(()) => {
                    cpu.r[0] = RESULT_SUCCESS;
                    Outcome::Continue
                }
                Err(f) => Outcome::Unimplemented(format!("command buffer unwritable: {f}")),
            },
            None => Outcome::Unimplemented(format!("RomFS file command {header:#010x}")),
        }
    }

    /// The block containing `address`, or the free block around it.
    pub fn query(&self, address: u32) -> Block {
        if let Some(b) = self.blocks.iter().find(|b| address.wrapping_sub(b.base) < b.size) {
            return *b;
        }
        let below = self.blocks.iter().filter(|b| b.base < address).map(|b| b.base + b.size).max().unwrap_or(0);
        let above = self.blocks.iter().filter(|b| b.base > address).map(|b| u64::from(b.base)).min().unwrap_or(1 << 32);
        Block { base: below, size: (above - u64::from(below)) as u32, perm: 0, state: STATE_FREE }
    }

    fn open(&mut self, object: Object) -> u32 {
        let handle = self.next_handle;
        self.next_handle += 0x8000;
        self.handles.insert(handle, object);
        handle
    }

    pub fn object(&self, handle: u32) -> Option<&Object> {
        self.handles.get(&handle)
    }

    /// Open handles, in handle order.
    pub fn handles(&self) -> impl Iterator<Item = (u32, &Object)> {
        self.handles.iter().map(|(h, o)| (*h, o))
    }

    pub fn current_thread(&self) -> &Thread {
        &self.threads[self.current]
    }

    fn current_id(&self) -> u32 {
        self.threads.get(self.current).map_or(MAIN_THREAD_ID, |t| t.id)
    }

    /// Saves the running context and loads the highest-priority ready
    /// thread's (lowest number), rotating among equals so each gets a turn.
    /// Returns false when no thread can run.
    ///
    /// When every thread waits and one has a timeout, the host sleeps until
    /// it expires: nothing else could happen in between.
    pub fn switch(&mut self, cpu: &mut Cpu, mem: &mut Memory) -> bool {
        // saved first: a wake-up writes its result into the saved copy, also
        // when the thread that wakes is the one that blocked
        self.threads[self.current].cpu = cpu.clone();
        loop {
            self.expire(self.now());
            if self.hardware(mem) {
                self.wake();
            }
            if let Some(next) = self.pick(|_| true) {
                self.load(next, cpu);
                return true;
            }
            let deadlines = self.threads.iter().filter_map(|t| match &t.state {
                ThreadState::Waiting(w) => w.deadline,
                _ => None,
            });
            let Some(first) = deadlines.chain(self.next_hardware_event()).min() else { return false };
            let wait = first.saturating_sub(self.now());
            // the limit covers each sleep and all of them together (with a
            // virtual clock: the clock itself)
            let limit = self.sleep_limit.map(|l| l.as_nanos() as u64);
            let over = match self.virtual_clock {
                Some(ns) => limit.is_some_and(|l| ns + wait > l),
                None => limit.is_some_and(|l| self.slept + wait > l),
            };
            if over {
                self.idle_wait = Some(wait);
                return false;
            }
            match &mut self.virtual_clock {
                Some(ns) => {
                    *ns = first.max(*ns);
                    if let Some(start) = self.pace {
                        let due = std::time::Duration::from_nanos(*ns);
                        while let Some(ahead) = due.checked_sub(start.elapsed()) {
                            if self.quit.as_ref().is_some_and(|q| q.load(std::sync::atomic::Ordering::Relaxed)) {
                                return false;
                            }
                            std::thread::sleep(ahead.min(std::time::Duration::from_millis(50)));
                        }
                    }
                }
                None => std::thread::sleep(std::time::Duration::from_nanos(wait)),
            }
            self.slept += wait;
        }
    }

    /// Nanoseconds since the kernel started: the virtual clock if there
    /// is one, else the host's monotonic clock.
    fn now(&self) -> u64 {
        match self.virtual_clock {
            Some(ns) => ns,
            None => self.started.map_or(0, |t| t.elapsed().as_nanos() as u64),
        }
    }

    /// Uses a virtual clock from now on: time advances with the
    /// instructions run (`advance`) and jumps to the next event when every
    /// thread waits, instead of the host sleeping. Runs then repeat exactly
    /// whatever the host's speed.
    pub fn use_virtual_clock(&mut self) {
        self.virtual_clock = Some(0);
    }

    /// Charges `instructions` to the virtual clock: one per ARM11 cycle at
    /// 268 MHz (a simplification: the real cost per instruction varies).
    pub fn advance(&mut self, instructions: u64) {
        if let Some(ns) = &mut self.virtual_clock {
            *ns += instructions * 1_000_000_000 / TICKS_PER_SECOND;
        }
    }

    /// Wakes, with RESULT_TIMEOUT, every waiting thread whose deadline has
    /// passed (SleepThread returns success instead).
    fn expire(&mut self, now: u64) {
        for t in &mut self.threads {
            if let ThreadState::Waiting(w) = &t.state {
                if w.deadline.is_some_and(|d| d <= now) {
                    t.cpu.r[0] = w.timeout_result;
                    t.state = ThreadState::Ready;
                }
            }
        }
    }

    /// Whether `object` would satisfy a wait by thread `me` now.
    fn signaled(&self, handle: u32, me: u32) -> bool {
        match self.handles.get(&handle) {
            Some(Object::Semaphore { count, .. }) => *count > 0,
            Some(Object::Mutex { owner, .. }) => owner.is_none_or(|o| o == me),
            Some(Object::Event { signaled, .. }) => *signaled,
            Some(Object::Thread { id }) => self.threads.iter().any(|t| t.id == *id && t.state == ThreadState::Exited),
            _ => false,
        }
    }

    /// Takes what a satisfied wait takes: a semaphore count, mutex
    /// ownership, a one-shot event's signal.
    fn acquire(&mut self, handle: u32, me: u32) {
        match self.handles.get_mut(&handle) {
            Some(Object::Semaphore { count, .. }) => *count -= 1,
            Some(Object::Mutex { owner, count }) => {
                *owner = Some(me);
                *count += 1;
            }
            Some(Object::Event { signaled, reset: 0 }) => *signaled = false,
            _ => {}
        }
    }

    /// Satisfies `wait` for thread `me` if it can be now: the index of the
    /// object that did it (0 for wait-all).
    fn satisfy(&mut self, wait: &Wait, me: u32) -> Option<u32> {
        if wait.handles.is_empty() {
            return None;
        }
        if wait.all {
            if !wait.handles.iter().all(|&h| self.signaled(h, me)) {
                return None;
            }
            for &h in &wait.handles {
                self.acquire(h, me);
            }
            return Some(0);
        }
        let index = wait.handles.iter().position(|&h| self.signaled(h, me))?;
        self.acquire(wait.handles[index], me);
        Some(index as u32)
    }

    /// After an object was signaled: wakes the waiting threads it now
    /// satisfies, highest priority first. Switch if one outranks the caller.
    fn wake(&mut self) -> Outcome {
        let mut order: Vec<usize> = (0..self.threads.len()).collect();
        order.sort_by_key(|&i| self.threads[i].priority);
        let mut preempt = false;
        for i in order {
            let ThreadState::Waiting(wait) = self.threads[i].state.clone() else { continue };
            let id = self.threads[i].id;
            if let Some(index) = self.satisfy(&wait, id) {
                let t = &mut self.threads[i];
                t.cpu.r[0] = RESULT_SUCCESS;
                if wait.index_in_r1 {
                    t.cpu.r[1] = index;
                }
                t.state = ThreadState::Ready;
                preempt |= t.priority < self.threads[self.current].priority;
            }
        }
        if preempt {
            Outcome::Switch
        } else {
            Outcome::Continue
        }
    }

    /// The common part of WaitSynchronization1/N and SleepThread: satisfied
    /// now, timed out now (timeout 0), or the thread blocks.
    fn wait(&mut self, cpu: &mut Cpu, wait: Wait) -> Outcome {
        if let Some(h) = wait.handles.iter().find(|&&h| !self.handles.contains_key(&h)) {
            return Outcome::Unimplemented(format!("wait on {h:#x}, which is not an object here"));
        }
        let me = self.current_id();
        if let Some(index) = self.satisfy(&wait, me) {
            cpu.r[0] = RESULT_SUCCESS;
            if wait.index_in_r1 {
                cpu.r[1] = index;
            }
            return Outcome::Continue;
        }
        if wait.deadline.is_some_and(|d| d <= self.now()) {
            cpu.r[0] = wait.timeout_result;
            return Outcome::Continue;
        }
        self.threads[self.current].state = ThreadState::Waiting(wait);
        Outcome::Switch
    }

    /// A timeout in nanoseconds (negative: none) as a kernel-clock deadline.
    fn deadline(&self, low: u32, high: u32) -> Option<u64> {
        let ns = (u64::from(high) << 32 | u64::from(low)) as i64;
        (ns >= 0).then(|| self.now().saturating_add(ns as u64))
    }

    /// At the end of a time slice: if a thread on the other core is ready,
    /// run it. This stands in for the two cores running at once, so that a
    /// thread waiting on another core's progress cannot spin forever.
    pub fn time_slice(&mut self, cpu: &mut Cpu) {
        let here = self.threads[self.current].core;
        if let Some(next) = self.pick(|t| t.core != here) {
            self.threads[self.current].cpu = cpu.clone();
            self.load(next, cpu);
        }
    }

    /// The ready thread to run next: the highest priority (lowest number),
    /// the one after the current in table order among equals.
    fn pick(&self, allowed: impl Fn(&Thread) -> bool) -> Option<usize> {
        let n = self.threads.len();
        (1..=n)
            .map(|k| (self.current + k) % n)
            .filter(|&i| self.threads[i].state == ThreadState::Ready && allowed(&self.threads[i]))
            .min_by_key(|&i| self.threads[i].priority)
    }

    /// Makes thread `next` current. The budget belongs to the run loop's
    /// time slice, not to a thread, so it carries over.
    fn load(&mut self, next: usize, cpu: &mut Cpu) {
        let budget = cpu.budget;
        *cpu = self.threads[next].cpu.clone();
        cpu.budget = budget;
        self.current = next;
    }

    /// CreateThread(r0 = priority, r1 = entry, r2 = argument, r3 = stack
    /// top, r4 = processor): r1 = handle. The new thread's FPSCR is 0, an
    /// open question (RESEARCH.md R-017).
    fn create_thread(&mut self, cpu: &mut Cpu) -> Outcome {
        let (priority, entry, arg, stack, processor) = (cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4] as i32);
        if priority > 0x3F || self.threads.len() as u32 >= TLS_SLOTS || !(-2..=1).contains(&processor) {
            return Outcome::Unimplemented(format!("CreateThread(priority {priority:#x}, processor {processor})"));
        }
        let slot = self.threads.len() as u32;
        let mut context = Cpu::default();
        context.r[0] = arg;
        context.r[13] = stack & !7;
        context.r[15] = entry & !1;
        context.thumb = entry & 1 == 1;
        context.tls = TLS + slot * TLS_SLOT;
        // -2: the process's ideal processor (0 for applications), -1: any
        let core = u32::from(processor == 1);
        let id = self.next_thread_id;
        self.next_thread_id += 1;
        self.threads.push(Thread { id, priority, core, cpu: context, state: ThreadState::Ready });
        cpu.r[1] = self.open(Object::Thread { id });
        cpu.r[0] = RESULT_SUCCESS;
        // a higher-priority thread (lower number) preempts its creator
        if priority < self.threads[self.current].priority {
            Outcome::Switch
        } else {
            Outcome::Continue
        }
    }

    /// Handles SVC `number`; `cpu.r[15]` already points after it.
    pub fn svc(&mut self, cpu: &mut Cpu, _mem: &mut Memory, number: u32) -> Outcome {
        let mut call = Call {
            number,
            resume: cpu.r[15],
            args: [cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4]],
            result: None,
            blocked: false,
        };
        let outcome = match number {
            // CreateAddressArbiter(): r0 = result, r1 = handle
            0x21 => {
                cpu.r[1] = self.open(Object::AddressArbiter);
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // QueryMemory(addr = r2): r1 base, r2 size, r3 permission,
            // r4 state, r5 page flags (none set here)
            0x02 => {
                let b = self.query(cpu.r[2]);
                (cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4], cpu.r[5]) = (b.base, b.size, b.perm, b.state, 0);
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // GetProcessInfo(r1 = process, r2 = type): r1:r2 = s64 value.
            // Only what the game has asked for is implemented.
            0x2B if cpu.r[1] == CURRENT_PROCESS && cpu.r[2] == 20 => {
                // 3dbrew: 0x20000000 - the process's LINEAR virtual base
                let linear = self.process.map_or(0x3000_0000, |p| p.linear_base);
                (cpu.r[1], cpu.r[2]) = (0x2000_0000u32.wrapping_sub(linear), 0);
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // GetResourceLimit(r1 = process): r1 = resource limit handle
            0x38 if cpu.r[1] == CURRENT_PROCESS => {
                cpu.r[1] = self.open(Object::ResourceLimit);
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // GetResourceLimitLimitValues / CurrentValues(r0 = s64 values
            // out, r1 = limit, r2 = types, r3 = count)
            0x39 | 0x3A if matches!(self.handles.get(&cpu.r[1]), Some(Object::ResourceLimit)) => {
                self.resource_values(cpu, _mem, number == 0x39)
            }
            // ControlMemory(r0 = operation, r1 = addr0, r2 = addr1, r3 = size,
            // r4 = permission): r1 = address
            0x01 => self.control_memory(cpu, _mem),
            // DuplicateHandle(r1 = handle): r1 = a new handle to its object
            0x27 => {
                let object = match cpu.r[1] {
                    CURRENT_THREAD => Some(Object::Thread { id: self.current_id() }),
                    other => self.handles.get(&other).cloned(),
                };
                match object {
                    Some(object) => {
                        cpu.r[1] = self.open(object);
                        cpu.r[0] = RESULT_SUCCESS;
                        Outcome::Continue
                    }
                    None => Outcome::Unimplemented(format!("DuplicateHandle({:#010x})", cpu.r[1])),
                }
            }
            // GetSystemTick(): r0:r1 = ticks since boot at the ARM11 tick
            // rate, from the kernel clock (virtual or the host's)
            0x28 => {
                let elapsed = u128::from(self.now());
                let ticks = (elapsed * u128::from(TICKS_PER_SECOND) / 1_000_000_000) as u64;
                (cpu.r[0], cpu.r[1]) = (ticks as u32, (ticks >> 32) as u32);
                Outcome::Continue
            }
            // GetProcessId(r1 = process): r1 = ID
            0x35 if cpu.r[1] == CURRENT_PROCESS => {
                cpu.r[1] = PROCESS_ID;
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // CreateMemoryBlock(r0 = other permission, r1 = address, r2 =
            // size, r3 = my permission): r1 = a block over the caller's own
            // memory, already mapped there (3dbrew "SVC"). Address 0 (the
            // kernel allocates) is not supported yet
            0x1E => {
                let (other, address, size, mine) = (cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3]);
                if address == 0 || address % 0x1000 != 0 || size % 0x1000 != 0 || _mem.read_range(address, size).is_err() {
                    return Outcome::Unimplemented(format!(
                        "CreateMemoryBlock({address:#x}, size {size:#x}, perm {mine}/{other})"
                    ));
                }
                cpu.r[1] = self.open(Object::SharedMemory { size, mapped: Some(address) });
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // MapMemoryBlock(r0 = block, r1 = address, r2 = my permission,
            // r3 = other permission): shared memory, at the address given
            0x1F => {
                let (handle, address, perm) = (cpu.r[0], cpu.r[1], cpu.r[2]);
                match self.handles.get(&handle) {
                    Some(&Object::SharedMemory { size, mapped: None })
                        if address != 0 && address % 0x1000 == 0 && perm & !3 == 0 =>
                    {
                        let access = if perm & 2 != 0 { Perm::RW } else { Perm::R };
                        if _mem.map(address, vec![0; size as usize], access).is_err() {
                            return Outcome::Unimplemented(format!("MapMemoryBlock at {address:#x}: already mapped"));
                        }
                        self.handles.insert(handle, Object::SharedMemory { size, mapped: Some(address) });
                        const STATE_SHARED: u32 = 6;
                        self.blocks.push(Block { base: address, size, perm, state: STATE_SHARED });
                        self.blocks.sort_by_key(|b| b.base);
                        cpu.r[0] = RESULT_SUCCESS;
                        Outcome::Continue
                    }
                    other => {
                        Outcome::Unimplemented(format!("MapMemoryBlock({handle:#x} = {other:?}, {address:#x}, perm {perm})"))
                    }
                }
            }
            0x08 => self.create_thread(cpu),
            // ExitThread()
            0x09 => {
                self.threads[self.current].state = ThreadState::Exited;
                self.wake();
                Outcome::Switch
            }
            // SleepThread(r0:r1 = nanoseconds). 0 yields to threads of the
            // same priority; there is no time slicing among them otherwise.
            0x0A => {
                let deadline = self.deadline(cpu.r[0], cpu.r[1]).unwrap_or(0);
                if deadline <= self.now() {
                    cpu.r[0] = RESULT_SUCCESS;
                    Outcome::Switch
                } else {
                    let outcome = self.wait(
                        cpu,
                        Wait {
                            deadline: Some(deadline),
                            timeout_result: RESULT_SUCCESS,
                            ..Wait::objects(Vec::new(), false, None, false)
                        },
                    );
                    cpu.r[0] = RESULT_SUCCESS;
                    outcome
                }
            }
            // ArbitrateAddress(r0 = arbiter, r1 = address, r2 = type,
            // r3 = value, r4:r5 = timeout), 3dbrew "Multi-threading"
            0x22 if matches!(self.handles.get(&cpu.r[0]), Some(Object::AddressArbiter)) => {
                let (address, kind, value) = (cpu.r[1], cpu.r[2], cpu.r[3] as i32);
                match kind {
                    // SIGNAL: wake up to `value` waiters (negative: all),
                    // highest priority first
                    0 => {
                        let mut waiters: Vec<usize> = (0..self.threads.len())
                            .filter(|&i| matches!(&self.threads[i].state, ThreadState::Waiting(w) if w.address == Some(address)))
                            .collect();
                        waiters.sort_by_key(|&i| self.threads[i].priority);
                        let n = if value < 0 { waiters.len() } else { (value as usize).min(waiters.len()) };
                        let mut preempt = false;
                        for &i in &waiters[..n] {
                            self.threads[i].cpu.r[0] = RESULT_SUCCESS;
                            self.threads[i].state = ThreadState::Ready;
                            preempt |= self.threads[i].priority < self.threads[self.current].priority;
                        }
                        cpu.r[0] = RESULT_SUCCESS;
                        if preempt {
                            Outcome::Switch
                        } else {
                            Outcome::Continue
                        }
                    }
                    // WAIT_IF_LESS_THAN, and with a timeout
                    1 | 3 => match _mem.read_u32(address) {
                        Ok(word) if (word as i32) < value => {
                            let deadline = if kind == 3 { self.deadline(cpu.r[4], cpu.r[5]) } else { None };
                            // the result after a timeout is not documented: a choice
                            let wait = Wait { address: Some(address), ..Wait::objects(Vec::new(), false, deadline, false) };
                            self.threads[self.current].state = ThreadState::Waiting(wait);
                            Outcome::Switch
                        }
                        Ok(_) => {
                            cpu.r[0] = RESULT_SUCCESS;
                            Outcome::Continue
                        }
                        Err(f) => Outcome::Unimplemented(format!("ArbitrateAddress on unreadable {address:#x}: {f}")),
                    },
                    _ => Outcome::Unimplemented(format!("ArbitrateAddress type {kind}")),
                }
            }
            // WaitSynchronization1(r0 = handle, r2:r3 = timeout)
            0x24 => {
                let deadline = self.deadline(cpu.r[2], cpu.r[3]);
                self.wait(cpu, Wait::objects(vec![cpu.r[0]], false, deadline, false))
            }
            // WaitSynchronizationN(r1 = handles, r2 = count, r3 = wait all,
            // r0:r4 = timeout): r1 = index (the game's wrapper at 0x00107564)
            0x25 if (1..=64).contains(&(cpu.r[2] as i32)) => {
                let handles = (0..cpu.r[2]).map(|k| _mem.read_u32(cpu.r[1].wrapping_add(4 * k)));
                match handles.collect::<Result<Vec<u32>, _>>() {
                    Ok(handles) => {
                        let deadline = self.deadline(cpu.r[0], cpu.r[4]);
                        self.wait(cpu, Wait::objects(handles, cpu.r[3] & 0xFF != 0, deadline, true))
                    }
                    Err(fault) => Outcome::Unimplemented(format!("WaitSynchronizationN handle list: {fault:?}")),
                }
            }
            // CreateMutex(r1 = initially locked): r1 = handle
            0x13 => {
                let owner = (cpu.r[1] != 0).then_some(self.current_id());
                cpu.r[1] = self.open(Object::Mutex { owner, count: u32::from(owner.is_some()) });
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // ReleaseMutex(r0)
            0x14 => match (self.current_id(), self.handles.get_mut(&cpu.r[0])) {
                (me, Some(Object::Mutex { owner: Some(owner), count })) if *owner == me => {
                    *count -= 1;
                    if *count == 0 {
                        self.handles.insert(cpu.r[0], Object::Mutex { owner: None, count: 0 });
                    }
                    cpu.r[0] = RESULT_SUCCESS;
                    self.wake()
                }
                (_, other) => Outcome::Unimplemented(format!("ReleaseMutex on {other:?}")),
            },
            // CreateSemaphore(r1 = initial, r2 = maximum): r1 = handle
            0x15 if (cpu.r[1] as i32) >= 0 && (cpu.r[1] as i32) <= (cpu.r[2] as i32) => {
                cpu.r[1] = self.open(Object::Semaphore { count: cpu.r[1] as i32, max: cpu.r[2] as i32 });
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // ReleaseSemaphore(r1 = semaphore, r2 = count): r1 = previous count
            0x16 => match self.handles.get_mut(&cpu.r[1]) {
                Some(Object::Semaphore { count, max }) if (cpu.r[2] as i32) >= 0 && *count + cpu.r[2] as i32 <= *max => {
                    cpu.r[1] = *count as u32;
                    *count += cpu.r[2] as i32;
                    cpu.r[0] = RESULT_SUCCESS;
                    self.wake()
                }
                other => Outcome::Unimplemented(format!("ReleaseSemaphore({}) on {other:?}", cpu.r[2] as i32)),
            },
            // CreateEvent(r1 = reset type): r1 = handle
            0x17 if cpu.r[1] <= 1 => {
                cpu.r[1] = self.open(Object::Event { signaled: false, reset: cpu.r[1] });
                cpu.r[0] = RESULT_SUCCESS;
                Outcome::Continue
            }
            // SignalEvent(r0) / ClearEvent(r0)
            0x18 | 0x19 => match self.handles.get_mut(&cpu.r[0]) {
                Some(Object::Event { signaled, .. }) => {
                    // a one-shot event that wakes a thread clears (acquire)
                    *signaled = number == 0x18;
                    cpu.r[0] = RESULT_SUCCESS;
                    self.wake()
                }
                _ => {
                    cpu.r[0] = RESULT_INVALID_HANDLE;
                    Outcome::Continue
                }
            },
            // CloseHandle(r0)
            0x23 => {
                cpu.r[0] = if self.handles.remove(&cpu.r[0]).is_some() { RESULT_SUCCESS } else { RESULT_INVALID_HANDLE };
                Outcome::Continue
            }
            // ConnectToPort(r1 = name): r1 = session handle
            0x2D => match Self::read_name(_mem, cpu.r[1], 12) {
                // srv: and err:f (the fatal error port, 3dbrew "Error
                // Display": ThrowFatalError reaches it directly)
                Some(name) if name == "srv:" || name == "err:f" => {
                    cpu.r[1] = self.open(Object::Session { service: name });
                    cpu.r[0] = RESULT_SUCCESS;
                    Outcome::Continue
                }
                other => Outcome::Unimplemented(format!("ConnectToPort({other:?})")),
            },
            // SendSyncRequest(r0 = session)
            // OpenLinkFile on a file session: another session to the same
            // file (3dbrew lists it among the file commands)
            0x32 if matches!(self.handles.get(&cpu.r[0]), Some(Object::RomFsFile { .. } | Object::SaveFile { .. }))
                && _mem.read_u32(cpu.tls + services::COMMAND_BUFFER) == Ok(0x080C_0000) =>
            {
                let object = self.handles[&cpu.r[0]].clone();
                let h = self.open(object);
                let reply = fs::open_file_reply_for(0x80C, h);
                self.requests.push(Request {
                    service: "file".into(),
                    header: 0x080C_0000,
                    words: Vec::new(),
                    handled: true,
                    vblank: self.vblanks,
                });
                match Message::reply(_mem, cpu.tls + services::COMMAND_BUFFER, &reply) {
                    Ok(()) => {
                        cpu.r[0] = RESULT_SUCCESS;
                        Outcome::Continue
                    }
                    Err(f) => Outcome::Unimplemented(format!("command buffer unwritable: {f}")),
                }
            }
            0x32 => match self.handles.get(&cpu.r[0]) {
                Some(Object::Session { service }) => {
                    let service = service.clone();
                    self.request(cpu, _mem, service)
                }
                Some(&Object::RomFsFile { start, size }) => self.file_request(cpu, _mem, cpu.r[0], start, size),
                Some(Object::Directory { .. }) => self.directory_request(cpu, _mem, cpu.r[0]),
                Some(Object::SaveFile { path, flags }) => {
                    let (path, flags) = (path.clone(), *flags);
                    self.save_file_request(cpu, _mem, cpu.r[0], &path, flags)
                }
                _ => {
                    cpu.r[0] = RESULT_INVALID_HANDLE;
                    Outcome::Continue
                }
            },
            _ => Outcome::Unimplemented(format!("svc {number:#04x} {}", name(number))),
        };
        call.blocked = matches!(self.threads.get(self.current).map(|t| &t.state), Some(ThreadState::Waiting(_)));
        if matches!(outcome, Outcome::Continue | Outcome::Switch) && !call.blocked {
            call.result = Some(cpu.r[0]);
        }
        self.calls.push(call);
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_address_arbiters_with_distinct_handles() {
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                Vec::new(),
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x21), Outcome::Continue));
        let first = cpu.r[1];
        k.svc(&mut cpu, &mut mem, 0x21);
        assert_ne!(first, cpu.r[1]);
        assert_eq!(cpu.r[0], RESULT_SUCCESS);
        assert_eq!(k.object(first), Some(&Object::AddressArbiter));
        assert!(![CURRENT_THREAD, CURRENT_PROCESS].contains(&first));
    }

    #[test]
    fn query_memory_reports_blocks_and_free_gaps() {
        use crate::boot::{PERM_R, PERM_W, STATE_PRIVATE};
        let stack = Block { base: 0x0FFC_0000, size: 0x4_0000, perm: PERM_R | PERM_W, state: STATE_PRIVATE };
        let low = Block { base: 0x0010_0000, size: 0x1000, perm: PERM_R, state: 4 };
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                vec![stack, low],
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        cpu.r[2] = 0x0FFF_F000;
        k.svc(&mut cpu, &mut mem, 0x02);
        assert_eq!((cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4]), (0, 0x0FFC_0000, 0x4_0000, 3, STATE_PRIVATE));
        // just below the stack: the free block between the two mappings
        cpu.r[2] = 0x0FFB_F000;
        k.svc(&mut cpu, &mut mem, 0x02);
        assert_eq!((cpu.r[1], cpu.r[2], cpu.r[4]), (0x0010_1000, 0x0FFC_0000 - 0x0010_1000, STATE_FREE));
        // above everything
        assert_eq!(k.query(0x2000_0000), Block { base: 0x1000_0000, size: 0xF000_0000, perm: 0, state: STATE_FREE });
    }

    #[test]
    fn process_info_linear_offset() {
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                Vec::new(),
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        (cpu.r[1], cpu.r[2]) = (CURRENT_PROCESS, 20);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x2B), Outcome::Continue));
        assert_eq!((cpu.r[0], cpu.r[1], cpu.r[2]), (0, 0xF000_0000, 0));
        // other types are not guessed
        (cpu.r[1], cpu.r[2]) = (CURRENT_PROCESS, 0);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x2B), Outcome::Unimplemented(_)));
    }

    #[test]
    fn heap_and_linear_allocations_respect_the_commit_limit() {
        let process = Process {
            linear_base: 0x3000_0000,
            app_region: 0x0010_0000,
            main_priority: 0x30,
            executable_end: 0x0070_0000,
            save_data_size: 0,
        };
        let (mut k, mut cpu, mut mem) = (Kernel::new(process, Vec::new(), Vec::new()), Cpu::default(), Memory::new());
        (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]) = (3, 0x0800_0000, 0x8_0000, 3);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x01), Outcome::Continue));
        assert_eq!(cpu.r[1], 0x0800_0000);
        assert!(mem.write_u32(0x0807_FFFC, 1).is_ok());
        (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]) = (0x1_0003, 0, 0x4_0000, 3);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x01), Outcome::Continue));
        assert_eq!(cpu.r[1], 0x3008_0000, "LINEAR after what is committed");
        // the next allocation would pass the limit: reported, not granted
        (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]) = (0x1_0003, 0, 0x8_0000, 3);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x01), Outcome::Unimplemented(_)));
    }

    #[test]
    fn mutexes_semaphores_and_events_keep_state() {
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                Vec::new(),
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        cpu.r[1] = 1;
        k.svc(&mut cpu, &mut mem, 0x13);
        let mutex = cpu.r[1];
        assert_eq!(k.object(mutex), Some(&Object::Mutex { owner: Some(MAIN_THREAD_ID), count: 1 }));
        cpu.r[0] = mutex;
        k.svc(&mut cpu, &mut mem, 0x14);
        assert_eq!(k.object(mutex), Some(&Object::Mutex { owner: None, count: 0 }));
        cpu.r[0] = mutex;
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x14), Outcome::Unimplemented(_)), "releasing a free mutex is not guessed");
        (cpu.r[1], cpu.r[2]) = (1, 3);
        k.svc(&mut cpu, &mut mem, 0x15);
        let sem = cpu.r[1];
        (cpu.r[1], cpu.r[2]) = (sem, 2);
        k.svc(&mut cpu, &mut mem, 0x16);
        assert_eq!(cpu.r[1], 1, "previous count");
        (cpu.r[1], cpu.r[2]) = (sem, 1);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x16), Outcome::Unimplemented(_)), "past the maximum");
        cpu.r[1] = 0;
        k.svc(&mut cpu, &mut mem, 0x17);
        let event = cpu.r[1];
        cpu.r[0] = event;
        k.svc(&mut cpu, &mut mem, 0x18);
        assert_eq!(k.object(event), Some(&Object::Event { signaled: true, reset: 0 }));
    }

    #[test]
    fn higher_priority_threads_preempt_and_exit_returns() {
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                Vec::new(),
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        cpu.r[15] = 0x0010_0000;
        (cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4]) = (0x18, 0x0020_0001, 0x1234, 0x0800_1004, 0xFFFF_FFFE);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x08), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        // now the new thread: Thumb entry, argument, aligned stack, its own TLS
        assert_eq!(
            (cpu.r[15], cpu.thumb, cpu.r[0], cpu.r[13], cpu.tls),
            (0x0020_0000, true, 0x1234, 0x0800_1000, TLS + TLS_SLOT)
        );
        assert_eq!(k.current_thread().id, MAIN_THREAD_ID + 1);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x09), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        assert_eq!((k.current_thread().id, cpu.r[15]), (MAIN_THREAD_ID, 0x0010_0000), "back to main");
        // a lower-priority thread does not preempt
        (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]) = (0x3F, 0x0030_0000, 0x0800_2000, 0);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x08), Outcome::Continue));
    }

    fn two_threads() -> (Kernel, Cpu, Memory) {
        let process = Process {
            linear_base: 0x3000_0000,
            app_region: 0x0500_0000,
            main_priority: 0x30,
            executable_end: 0x0070_0000,
            save_data_size: 0,
        };
        let (mut k, mut cpu, mut mem) = (Kernel::new(process, Vec::new(), Vec::new()), Cpu::default(), Memory::new());
        cpu.r[15] = 0x0010_0000;
        // a lower-priority second thread: no preemption
        (cpu.r[0], cpu.r[1], cpu.r[3], cpu.r[4]) = (0x31, 0x0020_0000, 0x0800_1000, 0);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x08), Outcome::Continue));
        (k, cpu, mem)
    }

    #[test]
    fn a_wait_blocks_until_a_release_wakes_it() {
        let (mut k, mut cpu, mut mem) = two_threads();
        (cpu.r[1], cpu.r[2]) = (0, 1);
        k.svc(&mut cpu, &mut mem, 0x15);
        let semaphore = cpu.r[1];
        // timeout 0 on an empty semaphore: times out at once
        (cpu.r[0], cpu.r[2], cpu.r[3]) = (semaphore, 0, 0);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x24), Outcome::Continue));
        assert_eq!(cpu.r[0], RESULT_TIMEOUT);
        // no timeout: main blocks, the other thread runs
        (cpu.r[0], cpu.r[2], cpu.r[3]) = (semaphore, u32::MAX, u32::MAX);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x24), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        assert_eq!((k.current_thread().id, cpu.r[15]), (MAIN_THREAD_ID + 1, 0x0020_0000));
        // it releases: main wakes with success and, being more urgent, preempts
        (cpu.r[1], cpu.r[2]) = (semaphore, 1);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x16), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        assert_eq!((k.current_thread().id, cpu.r[0]), (MAIN_THREAD_ID, RESULT_SUCCESS));
        assert_eq!(k.object(semaphore), Some(&Object::Semaphore { count: 0, max: 1 }), "the wake took the count");
    }

    #[test]
    fn wait_any_reports_the_index_and_everyone_waiting_stops() {
        let (mut k, mut cpu, mut mem) = two_threads();
        let mut events = Vec::new();
        for _ in 0..2 {
            cpu.r[1] = 0;
            k.svc(&mut cpu, &mut mem, 0x17);
            events.push(cpu.r[1]);
        }
        cpu.r[0] = events[1];
        k.svc(&mut cpu, &mut mem, 0x18);
        mem.map(0x0800_0000, vec![0; 0x1000], usum_guest::memory::Perm::RW).unwrap();
        mem.write_u32(0x0800_0000, events[0]).unwrap();
        mem.write_u32(0x0800_0004, events[1]).unwrap();
        (cpu.r[0], cpu.r[1], cpu.r[2], cpu.r[3], cpu.r[4]) = (u32::MAX, 0x0800_0000, 2, 0, u32::MAX);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x25), Outcome::Continue));
        assert_eq!((cpu.r[0], cpu.r[1]), (RESULT_SUCCESS, 1));
        assert_eq!(k.object(events[1]), Some(&Object::Event { signaled: false, reset: 0 }), "one-shot");
        // both threads wait on nothing that will come: no thread can run
        (cpu.r[0], cpu.r[2], cpu.r[3]) = (events[0], u32::MAX, u32::MAX);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x24), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        (cpu.r[0], cpu.r[2], cpu.r[3]) = (events[0], u32::MAX, u32::MAX);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x24), Outcome::Switch));
        assert!(!k.switch(&mut cpu, &mut mem));
    }

    #[test]
    fn sleeping_threads_wake_after_their_timeout() {
        let (mut k, mut cpu, mut mem) = two_threads();
        (cpu.r[0], cpu.r[1]) = (1_000_000, 0);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x0A), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        assert_eq!(k.current_thread().id, MAIN_THREAD_ID + 1);
        // the other thread blocks for good; the host sleeps until main wakes
        cpu.r[1] = 0;
        k.svc(&mut cpu, &mut mem, 0x17);
        (cpu.r[0], cpu.r[2], cpu.r[3]) = (cpu.r[1], u32::MAX, u32::MAX);
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x24), Outcome::Switch));
        assert!(k.switch(&mut cpu, &mut mem));
        assert_eq!((k.current_thread().id, cpu.r[0], cpu.r[15]), (MAIN_THREAD_ID, RESULT_SUCCESS, 0x0010_0000));
    }

    #[test]
    fn unknown_svcs_are_reported_not_guessed() {
        let (mut k, mut cpu, mut mem) = (
            Kernel::new(
                Process {
                    linear_base: 0x3000_0000,
                    app_region: 0x0500_0000,
                    main_priority: 0x30,
                    executable_end: 0x0070_0000,
                    save_data_size: 0,
                },
                Vec::new(),
                Vec::new(),
            ),
            Cpu::default(),
            Memory::new(),
        );
        cpu.r[0] = 0x1234;
        assert!(matches!(k.svc(&mut cpu, &mut mem, 0x7F), Outcome::Unimplemented(_)));
        assert_eq!(cpu.r[0], 0x1234, "registers untouched");
        assert_eq!(k.calls.last().unwrap().result, None);
    }
}
