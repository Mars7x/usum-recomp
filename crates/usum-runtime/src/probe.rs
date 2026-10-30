//! Runs translated code from the entry point, handing SVCs to the HLE
//! kernel, until one is not implemented (or a fault), and says where it got
//! to: instructions run, the SVCs made, and the one it stopped at.

use std::process::ExitCode;

use usum_guest::{lookup, Code, Cpu, Exit, Fault, Memory};

use crate::kernel::{self, Kernel};

mod dispatch;

/// Instructions per budget refill, and the refills allowed before the
/// probe calls it a hang.
const BUDGET: i64 = 1 << 20;
const MAX_REFILLS: u64 = 1 << 14;

/// The translated CRO modules (ADR-0007): their functions (indexed after
/// the executable's), their entries keyed by virtual address, and
/// (name, file SHA-256, virtual base) per module.
#[derive(Clone, Copy)]
pub struct Modules {
    pub functions: &'static [Code],
    pub entries: &'static [(u32, Code)],
    pub translated: &'static [(&'static str, &'static str, u32)],
}

/// The code for `key` (address | Thumb bit) in a loaded module, and the
/// delta to run it with: the module must be translated and its file be
/// the one translated (same SHA-256).
pub fn module_code(kernel: &Kernel, modules: &Modules, key: u32) -> Option<(Code, u32)> {
    let address = key & !1;
    let m = kernel.modules.iter().find(|m| m.base != 0 && address.wrapping_sub(m.base) < m.cro.file_size)?;
    let &(_, _, virtual_base) = modules.translated.iter().find(|(name, sha, _)| *name == m.cro.name && *sha == m.sha256)?;
    let delta = m.base.wrapping_sub(virtual_base);
    lookup(modules.entries, key.wrapping_sub(delta)).map(|code| (code, delta))
}

#[derive(Debug)]
pub enum Outcome {
    /// Something the kernel does not implement yet.
    Unimplemented(String),
    Fault(Fault),
    OutOfBudget,
    /// Every thread is waiting or has exited.
    NoThread,
    /// The game-time limit passed while the game was running.
    TimeLimit,
    /// The caller's hook asked to stop (a window closed).
    Stopped,
}

/// Runs until something is not implemented, a fault, a hang, or no thread
/// can run; returns the outcome and the instructions run (an upper bound:
/// a block is charged on entry).
pub fn run(cpu: &mut Cpu, mem: &mut Memory, kernel: &mut Kernel, entries: &[(u32, Code)], modules: &Modules) -> (Outcome, u64) {
    // USUM_MAX_REFILLS=n: allow longer runs (each refill is 2^20
    // instructions) before calling it a hang, for long replays
    let max_refills = std::env::var("USUM_MAX_REFILLS").ok().and_then(|v| v.parse().ok()).unwrap_or(MAX_REFILLS);
    run_with(cpu, mem, kernel, entries, modules, max_refills, &mut |kernel, mem| {
        snapshot(kernel, mem);
        profile_window(kernel);
        speed_report(kernel);
        true
    })
}

/// `run` with a refill limit and a hook called at each budget refill
/// (about every 2^20 instructions); the hook returns false to stop.
pub fn run_with(
    cpu: &mut Cpu,
    mem: &mut Memory,
    kernel: &mut Kernel,
    entries: &[(u32, Code)],
    modules: &Modules,
    max_refills: u64,
    hook: &mut dyn FnMut(&mut Kernel, &Memory) -> bool,
) -> (Outcome, u64) {
    let boot = kernel::BootImage::capture(cpu, mem, kernel);
    let mut refills = 0u64;
    let mut ran = 0u64;
    let mut dispatch = dispatch::CodeCache::new(std::env::var("USUM_DISPATCH_CACHE").as_deref() != Ok("0"));
    cpu.budget = BUDGET;
    loop {
        let key = cpu.r[15] | u32::from(cpu.thumb);
        let code = match dispatch.resolve(kernel, modules, entries, key) {
            Some((code, delta)) => {
                if let Some(delta) = delta {
                    cpu.module_delta = delta;
                }
                code
            }
            None => return (Outcome::Fault(Fault::NoTranslation { address: cpu.r[15], thumb: cpu.thumb }), ran),
        };
        cpu.depth = 0;
        let before = cpu.budget;
        let exit = code(cpu, mem);
        let spent = (before - cpu.budget) as u64;
        ran += spent;
        kernel.advance(spent);
        match exit {
            Ok(Exit::Jump) => {}
            Ok(Exit::Svc(n)) => match kernel.tick_then_svc(cpu, mem, n) {
                kernel::Outcome::Continue => {}
                kernel::Outcome::Restart(args) => {
                    let Some(boot) = &boot else {
                        return (Outcome::Unimplemented("application restart without a boot image".into()), ran);
                    };
                    boot.restart(cpu, mem, kernel, args);
                    cpu.budget = BUDGET;
                    // Loaded CROs and all translated dispatch ownership belong to
                    // the terminated process, even if their addresses recur.
                    dispatch = dispatch::CodeCache::new(std::env::var("USUM_DISPATCH_CACHE").as_deref() != Ok("0"));
                    eprintln!("application restarted: {} at VBlank {}", kernel.restarts, kernel.vblanks);
                }
                kernel::Outcome::Switch => {
                    if !kernel.switch(cpu, mem) {
                        return (Outcome::NoThread, ran);
                    }
                }
                kernel::Outcome::Unimplemented(what) => return (Outcome::Unimplemented(what), ran),
            },
            Ok(Exit::Budget) => {
                refills += 1;
                if refills >= max_refills {
                    return (Outcome::OutOfBudget, ran);
                }
                // the game-time limit holds while the game is busy too
                if let (Some(now), Some(limit)) = (kernel.virtual_clock, kernel.sleep_limit) {
                    if u128::from(now) >= limit.as_nanos() {
                        return (Outcome::TimeLimit, ran);
                    }
                }
                if !hook(kernel, mem) {
                    return (Outcome::Stopped, ran);
                }
                cpu.budget = BUDGET;
                if kernel.hardware(mem) && kernel.wakes_more_urgent() {
                    if !kernel.switch(cpu, mem) {
                        return (Outcome::NoThread, ran);
                    }
                } else {
                    kernel.time_slice(cpu);
                }
            }
            Err(fault) => return (Outcome::Fault(fault), ran),
        }
    }
}

/// Inspects and boots `dump`, installs the function table (the
/// executable's, then the modules'), and makes the kernel with the
/// console's region and the RomFS image. The save directory and the clock
/// are the caller's choice. Also returns the program ID.
pub fn setup(
    dump: &std::path::Path,
    functions: &'static [Code],
    modules: &Modules,
) -> Result<(Cpu, Memory, Kernel, u64), String> {
    let inspection = usum_import::inspect(dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    let (cpu, mem, blocks) = crate::boot::boot(&inspection).map_err(|e| e.to_string())?;
    let table: Vec<Code> = functions.iter().chain(modules.functions).copied().collect();
    usum_guest::install_functions(Box::leak(table.into_boxed_slice()));
    let process = crate::boot::Process::from_exheader(&inspection.exheader);
    let mut kernel = Kernel::new(process, blocks, inspection.exheader.services.clone());
    kernel.program_id = inspection.ncch.program_id;
    if let usum_import::profile::Match::Profile { profile } = &inspection.profile {
        kernel.console = crate::services::cfg::Console::for_region(profile.region);
    }
    if let Some(range) = inspection.romfs_image {
        kernel.romfs = Some(crate::services::fs::Image::open(dump, range).map_err(|e| format!("{}: {e}", dump.display()))?);
    }
    for entry in inspection.exefs.iter().filter(|e| matches!(e.name.as_str(), "icon" | "banner" | "logo")) {
        let range = usum_import::romfs::Image {
            offset: inspection.partition_offset + inspection.ncch.exefs.offset + entry.offset,
            size: entry.size,
        };
        let image = crate::services::fs::Image::open(dump, range).map_err(|e| format!("{}: {e}", dump.display()))?;
        kernel.exefs.insert(entry.name.clone(), image);
    }
    Ok((cpu, mem, kernel, inspection.ncch.program_id))
}

/// The `main` of a generated probe program: `probe <dump>`.
pub fn main(functions: &'static [Code], entries: &'static [(u32, Code)], modules: Modules) -> ExitCode {
    let _resolution_scale = match crate::pica::resolution_scale_env() {
        Ok(scale) if scale == 1 || (cfg!(feature = "vulkan-compare") && std::env::var_os("USUM_VULKAN_ACCELERATE").is_some()) => {
            scale
        }
        Ok(_) => {
            eprintln!("resolution scaling requires Vulkan acceleration");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    let Some(dump) = std::env::args_os().nth(1) else {
        eprintln!("usage: probe <dump>");
        return ExitCode::FAILURE;
    };
    let (mut cpu, mut mem, mut kernel, _) = match setup(std::path::Path::new(&dump), functions, &modules) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let start = std::time::Instant::now();
    #[cfg(feature = "vulkan-compare")]
    if let Some(draws) = std::env::var("USUM_VULKAN_CAPTURE").ok().and_then(|value| value.parse::<usize>().ok()) {
        let draws = draws.min(64);
        let skip = std::env::var("USUM_VULKAN_SKIP").ok().and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
        kernel.pica.enable_vulkan_capture_after(skip, draws);
        eprintln!("capturing {draws} draws for Vulkan comparison after skipping {skip}");
    }
    #[cfg(feature = "vulkan-compare")]
    if std::env::var_os("USUM_VULKAN_ACCELERATE").is_some() {
        kernel.pica.enable_vulkan_acceleration();
        let scale = crate::pica::resolution_scale_env().expect("validated resolution scale");
        if let Err(error) = kernel.pica.set_vulkan_resolution_scale(scale) {
            eprintln!("{error}");
        }
        eprintln!("internal resolution scale {scale}x: top {}x{}", 400 * scale, 240 * scale);
        if std::env::var("USUM_VULKAN_DEFER_READBACK").as_deref() != Ok("0") {
            kernel.pica.enable_vulkan_deferred_readback();
            eprintln!("deferred Vulkan readbacks enabled");
        }
        if !std::env::var("USUM_VULKAN_VERTEX").is_ok_and(|value| value == "0") {
            kernel.pica.enable_vulkan_vertex();
            eprintln!("experimental hardware PICA vertex shaders enabled");
        }
        eprintln!("guarded Vulkan raster acceleration enabled; dependent or failed lists rerun in software");
        if std::env::var_os("USUM_VULKAN_DEPTH_FEEDBACK").is_some()
            || std::env::var_os("USUM_VULKAN_CPU_DEPTH_FEEDBACK").is_some()
        {
            kernel.pica.enable_vulkan_cpu_depth_feedback();
            eprintln!("experimental Vulkan depth-feedback barriers enabled");
        }
    } else if std::env::var_os("USUM_VULKAN_LIVE_DEPTH").is_some() {
        kernel.pica.enable_vulkan_live_depth();
        eprintln!("experimental Vulkan live color+depth mode enabled; software rendering remains active");
    } else if std::env::var_os("USUM_VULKAN_LIVE").is_some() {
        kernel.pica.enable_vulkan_live();
        eprintln!("experimental Vulkan live color mode enabled; software rendering and depth remain active");
    }
    // USUM_WAV=file.wav: record the mixed audio (game-derived: private)
    let wav = std::env::var_os("USUM_WAV");
    if wav.is_some() {
        kernel.audio.record = Some(Vec::new());
    }
    // a scratch save directory beside the probe binary (in the private
    // translation workspace), emptied each run so runs repeat exactly;
    // never the user's real saves
    let save_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("probe-save")));
    if let Some(dir) = &save_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
    // USUM_SAVE_FROM=dir: start from a copy of those saves (a play
    // session's, to replay its recorded input)
    if let (Some(from), Some(dir)) = (std::env::var_os("USUM_SAVE_FROM"), &save_dir) {
        if let Err(e) = copy_saves(std::path::Path::new(&from), dir) {
            eprintln!("USUM_SAVE_FROM: {e}");
            return ExitCode::FAILURE;
        }
    }
    kernel.save_dir = save_dir;
    // the probe runs on a virtual clock (deterministic); it stops when
    // every thread waits past USUM_SECONDS of game time (default 10)
    kernel.use_virtual_clock();
    let seconds = std::env::var("USUM_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(10);
    kernel.sleep_limit = Some(std::time::Duration::from_secs(seconds));
    // USUM_WATCH: a call target to watch (its last caller is reported)
    if let Some(w) = std::env::var("USUM_WATCH").ok().and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
        usum_guest::WATCH.store(w, std::sync::atomic::Ordering::Relaxed);
    }
    // USUM_INPUT or USUM_INPUT_FILE: buttons by VBlank, e.g.
    // "120=A,126=" (see services::hid).
    let input = std::env::var("USUM_INPUT").ok().or_else(|| {
        std::env::var_os("USUM_INPUT_FILE").and_then(|path| match std::fs::read_to_string(&path) {
            Ok(script) => Some(script),
            Err(error) => {
                eprintln!("USUM_INPUT_FILE {}: {error}", std::path::Path::new(&path).display());
                None
            }
        })
    });
    if let Some(script) = input {
        match crate::services::hid::parse_script(&script) {
            Ok(steps) => kernel.input = steps,
            Err(e) => {
                eprintln!("USUM_INPUT: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (outcome, instructions) = run(&mut cpu, &mut mem, &mut kernel, entries, &modules);
    let took = start.elapsed();
    if matches!(outcome, Outcome::OutOfBudget) {
        eprintln!(
            "probe instruction cap reached at {:.3} s of game time (requested {seconds} s); \
             increase USUM_MAX_REFILLS for a longer replay, or investigate a possible hang",
            kernel.virtual_clock.unwrap_or(0) as f64 / 1e9
        );
    }
    #[cfg(feature = "vulkan-compare")]
    {
        let (lists, draws, targets, time) = kernel.pica.vulkan_live_stats();
        if lists != 0 {
            println!("vulkan live    {draws} draws, {targets} targets, {lists} lists, {:.3} s", time.as_secs_f64());
            let ([gpu, vertices, geometry, unsupported], error) = kernel.pica.vulkan_vertex_stats();
            if gpu + geometry + unsupported != 0 {
                println!("vulkan vertex  {gpu} GPU draws, {vertices} distinct inputs; CPU draws: {geometry} geometry, {unsupported} other");
                if let Some(error) = error {
                    println!("vulkan vertex fallback: {error}");
                }
            }
            let frozen = kernel.pica.vulkan_frozen_texture_captures();
            let [planned, inputs] = kernel.pica.vulkan_input_plan_stats();
            println!("vulkan vertex inputs {planned} planned draws, {inputs} distinct inputs");
            if frozen != 0 {
                println!("vulkan frozen textures {frozen} captures");
            }
            let (queued, materialized, elapsed) = kernel.pica.vulkan_deferred_stats();
            if queued != 0 {
                println!(
                    "vulkan deferred {queued} queued, {materialized} materialized; wait/encode {:.3} s",
                    elapsed.as_secs_f64()
                );
            }
            let (reused, uploaded) = kernel.pica.vulkan_target_stats();
            let (created, reused_draws, retained, bytes) = kernel.pica.vulkan_draw_upload_stats();
            println!("vulkan draw uploads {created} allocations, {reused_draws} reuses, {retained} retained ({bytes} bytes)");
            println!("vulkan uniform depth {} imports", kernel.pica.vulkan_uniform_depth_imports());
            println!("vulkan targets {reused} resident reuses, {uploaded} guest uploads");
            let [targets, textures, vertices] = kernel.pica.vulkan_capture_times().map(|time| time.as_secs_f64());
            println!("vulkan capture targets {targets:.3} s, textures {textures:.3} s, vertex packing {vertices:.3} s");
            let [readback, writeback] = kernel.pica.vulkan_transfer_times().map(|time| time.as_secs_f64());
            println!("vulkan transfer submit/readback {readback:.3} s, CPU writeback {writeback:.3} s");
            let (texture_uses, texture_uploads, lighting_uses, lighting_uploads) = kernel.pica.vulkan_live_resource_stats();
            println!(
                "vulkan cache   {texture_uploads}/{texture_uses} texture uploads/uses, {lighting_uploads}/{lighting_uses} lighting uploads/uses"
            );
            let [compatible, other_color, depth, partial] = kernel.pica.vulkan_live_feedback_kinds();
            println!(
                "vulkan feedback {} reads: {compatible} compatible color, {other_color} other color, {depth} depth, {partial} partial",
                kernel.pica.vulkan_live_feedback_reads()
            );
            let depth_pairs = kernel
                .pica
                .vulkan_live_feedback_depth_pairs()
                .into_iter()
                .enumerate()
                .filter(|&(_, count)| count != 0)
                .map(|(index, count)| format!("texture {}/depth {}: {count}", index / 4, index % 4))
                .collect::<Vec<_>>();
            if !depth_pairs.is_empty() {
                println!("vulkan depth feedback {}", depth_pairs.join(", "));
            }
            let (accelerated, fallbacks) = kernel.pica.vulkan_acceleration_stats();
            if accelerated != 0 || fallbacks != 0 {
                println!("vulkan accel    {accelerated} lists, {fallbacks} software fallbacks");
            }
        }
        let snapshots = kernel.pica.take_vulkan_snapshots();
        if snapshots.is_empty() && std::env::var_os("USUM_VULKAN_CAPTURE").is_some() {
            println!("vulkan          no requested draws were reached");
        } else if !snapshots.is_empty() {
            match usum_render::Renderer::new() {
                Ok(renderer) => {
                    println!("vulkan adapter {}", renderer.adapter_info().name);
                    for (index, snapshot) in snapshots.iter().enumerate() {
                        if let Some(directory) = std::env::var_os("USUM_VULKAN_CAPTURE_DIR") {
                            if let Err(error) = snapshot.write_diagnostic(&renderer, std::path::Path::new(&directory)) {
                                eprintln!("Vulkan diagnostic export failed: {error}");
                            }
                        }
                        match snapshot.compare(&renderer) {
                            Ok(comparison) => println!(
                                "vulkan draw    {} (guest #{}, {} triangles, lighting {}): {}/{} pixels differ, {} over 1; maximum delta {} at {:?}, Vulkan {:?}, software {:?}; depth {} differ, maximum {} at {:?}; stencil {} differ",
                                index + 1,
                                snapshot.draw_number,
                                snapshot.vertices.len() / 3,
                                snapshot.state.lighting.is_some(),
                                comparison.different_pixels,
                                comparison.pixels,
                                comparison.pixels_over_one,
                                comparison.maximum_channel_delta,
                                comparison.maximum_at,
                                comparison.actual_at_maximum,
                                comparison.expected_at_maximum,
                                comparison.different_depth_values,
                                comparison.maximum_depth_delta,
                                comparison.maximum_depth_at,
                                comparison.different_stencil_values
                            ),
                            Err(error) => println!("vulkan draw    {}: unsupported: {error}", index + 1),
                        }
                    }
                }
                Err(error) => println!("vulkan          unavailable: {error}"),
            }
        }
    }
    println!(
        "display transfer {} copies, {:.3} s",
        kernel.pica.display_transfers,
        kernel.pica.display_transfer_time.as_secs_f64()
    );
    println!("video conversion {} jobs, {:.3} s", kernel.y2r_conversions, kernel.y2r_time.as_secs_f64());
    let top_frames = kernel.presented.iter().filter(|p| p.screen == 0).count();
    let groups: Vec<_> = kernel
        .gsp_list_groups
        .iter()
        .enumerate()
        .filter(|(size, count)| *size != 0 && **count != 0)
        .map(|(size, count)| format!("{size} lists: {count} groups"))
        .collect();
    println!("gx list groups {}", groups.join(", "));
    let game_seconds = kernel.virtual_clock.unwrap_or(0) as f64 / 1e9;
    println!(
        "game frames    {top_frames} top-screen updates, {:.2} updates/s wall throughput, {:.2} updates/s game clock",
        top_frames as f64 / took.as_secs_f64().max(1e-9),
        top_frames as f64 / game_seconds.max(1e-9)
    );
    if std::env::var_os("USUM_QUIET").is_some() {
        println!("ran            {instructions} instructions in {:.3} s", took.as_secs_f64());
        println!("game time      {:.3} s (requested {seconds} s)", kernel.virtual_clock.unwrap_or(0) as f64 / 1e9);
        println!(
            "shaders        {} runs, {} instructions",
            crate::pica::shader::RUNS.load(std::sync::atomic::Ordering::Relaxed),
            crate::pica::shader::STEPS.load(std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "hardware       {} VBlanks, {} draws, {} triangles, {} fragments",
            kernel.vblanks, kernel.pica.draws, kernel.pica.triangles, kernel.pica.fragments
        );
        println!("stopped        {outcome:?}");
        return ExitCode::SUCCESS;
    }
    println!("ran            {instructions} instructions in {:.3} s", took.as_secs_f64());
    let handled = kernel.calls.iter().filter(|c| c.result.is_some()).count();
    println!("svcs handled   {handled}");
    println!("romfs read     {} bytes", kernel.romfs_read);
    println!(
        "save data      {} bytes read, {} bytes written, {} commits",
        kernel.save_bytes_read, kernel.save_bytes_written, kernel.save_commits
    );
    println!(
        "hardware       {} VBlanks, {} audio frames, {} framebuffers presented",
        kernel.vblanks,
        kernel.audio_frames,
        kernel.presented.len()
    );
    for p in kernel.presented.iter().take(4) {
        println!("present        {p:x?}");
    }
    for (frame, source, playing, id, samples, flags) in &kernel.audio.events {
        let state = if *playing { "plays" } else { "idle " };
        println!("dsp source     frame {frame} source {source} {state} buffer {id} ({samples} samples), flags {flags:#010x}");
    }
    // guest-visible addresses: a module's code runs at its virtual base
    let mut counts: Vec<((u32, u32, u32), u64)> =
        usum_guest::PROFILE_COUNTS.with(|p| p.borrow().iter().map(|(k, v)| (*k, *v)).collect());
    counts.sort_by_key(|&(k, n)| (std::cmp::Reverse(n), k));
    for ((target, lr, delta), n) in counts.iter().take(50_000) {
        let visible = |a: u32| if a >= 0x4000_0000 { a.wrapping_add(*delta) } else { a };
        println!("profile        {n:>8} x {:#010x} from {:#010x}", visible(*target), visible(*lr));
    }
    if let (Some(path), Some(record)) = (&wav, &kernel.audio.record) {
        let peak = record.iter().flatten().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        match std::fs::write(path, crate::audio::wav(record)) {
            Ok(()) => println!(
                "audio          {:.1} s written to {} (peak {peak})",
                record.len() as f64 / f64::from(crate::audio::OUTPUT_RATE),
                std::path::Path::new(path).display()
            ),
            Err(e) => println!("audio          not written: {e}"),
        }
    }
    // USUM_DUMP=address:length[,address:length...] (hex): those guest
    // bytes at the end, to probe-frames/dump-ADDRESS.bin (private: game
    // memory). An address may follow pointers: [a] is the word at a, and
    // +n / -n add, so [[6a5518]+18]+c is a field of an object two
    // pointers away.
    let dumps: Vec<(u32, u32)> = std::env::var("USUM_DUMP")
        .unwrap_or_default()
        .split(',')
        .filter_map(|v| {
            let (a, l) = v.rsplit_once(':')?;
            Some((guest_address(&mem, a)?, u32::from_str_radix(l.trim_start_matches("0x"), 16).ok()?))
        })
        .collect();
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("probe-frames")));
    for (address, len) in dumps {
        let Some(dir) = &dir else { break };
        let path = dir.join(format!("dump-{address:08x}.bin"));
        match mem.read_range(address, len) {
            Ok(bytes) if std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, &bytes)).is_ok() => {
                println!("dump           {address:#010x}..+{len:#x} to {}", path.display());
            }
            r => println!("dump           {address:#010x}..+{len:#x} failed: {:?}", r.err()),
        }
    }
    // the last framebuffer of each screen, as the LCD shows it, beside the
    // probe binary (private: screen images are game-derived)
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("probe-frames"))) {
        for screen in 0..2 {
            let Some(p) = kernel.presented.iter().rev().find(|p| p.screen == screen) else { continue };
            match framebuffer_ppm(&mem, p, if screen == 0 { 400 } else { 320 }) {
                Ok(ppm) => {
                    let path = dir.join(if screen == 0 { "top.ppm" } else { "bottom.ppm" });
                    if std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, ppm)).is_ok() {
                        println!("frame          {}", path.display());
                    }
                }
                Err(e) => println!("frame          screen {screen}: {e}"),
            }
        }
    }
    for m in &kernel.modules {
        let text = m.text().map_or("no text".to_owned(), |t| format!("text {:#010x}..{:#010x}", t.start, t.end));
        println!(
            "module         {:<20} {text}, {} imports ({} unresolved), {} internal relocations",
            m.cro.name,
            m.cro.imports.len(),
            m.unresolved.len(),
            m.cro.internal.len()
        );
    }
    if let Some(e) = &kernel.hardware_error {
        println!("hardware error {e}");
    }
    println!(
        "gpu            {} GX commands, {} command lists, {} register writes, {} draws, {} triangles, {} fragments",
        kernel.gx_log.len(),
        kernel.pica.lists,
        kernel.pica.writes,
        kernel.pica.draws,
        kernel.pica.triangles,
        kernel.pica.fragments
    );
    println!("vertices       {} with w > 0, {} without", kernel.pica.vertices_positive_w, kernel.pica.vertices_other_w);
    println!(
        "gpu time       {:.3} s drawing, of which {:.3} s rasterizing",
        kernel.pica.draw_time.as_secs_f64(),
        kernel.pica.raster_time.as_secs_f64()
    );
    {
        use crate::pica::draw::{LIGHTING_READ_DRAWS, LIT_DRAWS, TEXTURE3_DRAWS};
        use std::sync::atomic::Ordering::Relaxed;
        println!(
            "gpu features   {} draws with fragment lighting on, {} reading its results, {} reading texture 3 (not emulated)",
            LIT_DRAWS.load(Relaxed),
            LIGHTING_READ_DRAWS.load(Relaxed),
            TEXTURE3_DRAWS.load(Relaxed)
        );
    }
    let (runs, steps) = (
        crate::pica::shader::RUNS.load(std::sync::atomic::Ordering::Relaxed),
        crate::pica::shader::STEPS.load(std::sync::atomic::Ordering::Relaxed),
    );
    println!("shaders        {runs} runs, {steps} instructions ({:.1} per run)", steps as f64 / runs.max(1) as f64);
    let ph = kernel.pica.phases.map(|d| d.as_secs_f64());
    println!(
        "gpu phases     vertices {:.3} s (indices {:.3}, fused loading/shading {:.3}), buffers out {:.3} s, textures {:.3} s, bands {:.3} s, buffers back {:.3} s",
        ph[0], ph[5], ph[7], ph[1], ph[2], ph[3], ph[4]
    );
    if let Some((size, hash)) = &kernel.dsp_firmware {
        println!("dsp firmware   {size} bytes, sha256 {hash} (accepted, not run)");
    }
    for (channel, bytes) in &kernel.dsp_pipe_writes {
        println!("dsp pipe       channel {channel} <- {bytes:02x?}");
    }
    let mut counts = std::collections::BTreeMap::new();
    for c in kernel.calls.iter().filter(|c| c.result.is_some()) {
        *counts.entry(c.number).or_insert(0usize) += 1;
    }
    for (n, count) in counts {
        println!("               {count:>6} x svc {n:#04x} {}", kernel::name(n));
    }
    for c in kernel.calls.iter().rev().take(8).collect::<Vec<_>>().into_iter().rev() {
        let args: Vec<String> = c.args.iter().map(|a| format!("{a:08x}")).collect();
        let result = match c.result {
            Some(r) => format!("-> {r:#010x}"),
            None if c.blocked => "blocked".to_owned(),
            None => "not implemented".to_owned(),
        };
        println!(
            "call           svc {:#04x} {:<24} at {:#010x} args {} {result}",
            c.number,
            kernel::name(c.number),
            c.resume - 4,
            args.join(" ")
        );
    }
    for r in &kernel.requests {
        let words: Vec<String> = r.words.iter().map(|w| format!("{w:08x}")).collect();
        println!(
            "ipc            {:>6} {:<10} {:#010x} [{}] {}",
            r.vblank,
            r.service,
            r.header,
            words.join(" "),
            if r.handled { "" } else { "not implemented" }
        );
    }
    match &outcome {
        Outcome::Unimplemented(what) => println!("stopped at     {what}: not implemented; next instruction {:#010x}", cpu.r[15]),
        Outcome::Fault(f) => println!("stopped by     {f}"),
        Outcome::OutOfBudget => {
            println!("stopped        after {instructions} instructions: budget exhausted (a hang, or just long)")
        }
        Outcome::TimeLimit => println!("stopped        at the game-time limit, the game running"),
        Outcome::Stopped => println!("stopped        on request"),
        Outcome::NoThread => match kernel.idle_wait {
            Some(ns) => println!(
                "stopped        at {:.3} s of game time (the limit), every thread waiting; next event in {:.3} s; {:.3} s idle in all",
                kernel.virtual_clock.unwrap_or(0) as f64 / 1e9,
                ns as f64 / 1e9,
                kernel.slept as f64 / 1e9
            ),
            None => println!("stopped        no thread can run"),
        },
    }
    if let Outcome::Fault(Fault::NoTranslation { address, thumb }) = &outcome {
        // where in memory, at this point, something points at the target
        let target = *address | u32::from(*thumb);
        let mut holders = Vec::new();
        for (base, bytes, _) in mem.regions() {
            for (i, word) in bytes.as_chunks::<4>().0.iter().enumerate() {
                if u32::from_le_bytes(*word) == target {
                    holders.push(base + 4 * i as u32);
                }
            }
        }
        let shown: Vec<String> = holders.iter().take(8).map(|a| format!("{a:#010x}")).collect();
        println!("pointed to by  {} words in memory now: {}", holders.len(), shown.join(" "));
        println!("jumped from    {:#010x} (the last indirect branch)", cpu.from);
    }
    for t in &kernel.threads {
        let pc = if t.id == kernel.current_thread().id { cpu.r[15] } else { t.cpu.r[15] };
        println!("thread         id {} priority {:#04x} core {} {:?} pc {pc:#010x}", t.id, t.priority, t.core, t.state);
    }
    println!("thread now     {}", kernel.current_thread().id);
    let hit = usum_guest::WATCH_HIT.load(std::sync::atomic::Ordering::Relaxed);
    if hit != 0 {
        println!(
            "watch          last call to {:#010x} returned to {hit:#010x}",
            usum_guest::WATCH.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    // a heuristic backtrace: stack words that point into code, from sp up
    let in_code = |w: u32| {
        let a = w & !1;
        (0x0010_0000..0x0060_0000).contains(&a) || kernel.modules.iter().any(|m| m.text().is_some_and(|t| t.contains(&a)))
    };
    let stack: Vec<String> = (0..0x200u32)
        .filter_map(|k| mem.read_u32(cpu.r[13].wrapping_add(4 * k)).ok().filter(|&w| in_code(w)).map(|w| format!("{w:08x}")))
        .take(24)
        .collect();
    println!("stack code     lr {:08x}; {}", cpu.r[14], stack.join(" "));
    for (handle, object) in kernel.handles() {
        println!("handle         {handle:#010x} {object:?}");
    }
    println!("registers      {}", (0..16).map(|i| format!("r{i}={:08x}", cpu.r[i])).collect::<Vec<_>>().join(" "));
    println!("flags          n={} z={} c={} v={} thumb={}", cpu.n as u8, cpu.z as u8, cpu.c as u8, cpu.v as u8, cpu.thumb as u8);
    ExitCode::SUCCESS
}

/// A USUM_DUMP address: hex, `[expression]` for the word there, and
/// `+hex` / `-hex` terms.
fn guest_address(mem: &Memory, text: &str) -> Option<u32> {
    let text = text.trim();
    let (mut value, mut rest) = if let Some(inner) = text.strip_prefix('[') {
        let mut depth = 1;
        let close = inner.char_indices().find(|&(_, c)| {
            depth += i32::from(c == '[') - i32::from(c == ']');
            depth == 0
        })?;
        (mem.read_u32(guest_address(mem, &inner[..close.0])?).ok()?, &inner[close.0 + 1..])
    } else {
        let end = text.find(['+', '-']).unwrap_or(text.len());
        (u32::from_str_radix(text[..end].trim_start_matches("0x"), 16).ok()?, &text[end..])
    };
    while !rest.is_empty() {
        let sign = rest.as_bytes()[0];
        let end = rest[1..].find(['+', '-']).map_or(rest.len(), |e| e + 1);
        let n = u32::from_str_radix(rest[1..end].trim_start_matches("0x"), 16).ok()?;
        value = if sign == b'+' { value.wrapping_add(n) } else { value.wrapping_sub(n) };
        rest = &rest[end..];
    }
    Some(value)
}

/// Copies directory `from` to `to`, recursively.
fn copy_saves(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_saves(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// USUM_SPEED=n: every n VBlanks, how fast those VBlanks ran against real
/// time (game time / wall time; below 1 the game would fall behind).
pub(crate) fn speed_report(kernel: &Kernel) {
    thread_local! {
        static EVERY: Option<u64> = std::env::var("USUM_SPEED").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0);
        static LAST: std::cell::Cell<Option<(u64, u64, std::time::Instant)>> = const { std::cell::Cell::new(None) };
        static PHASES: std::cell::Cell<[std::time::Duration; 8]> = const { std::cell::Cell::new([std::time::Duration::ZERO; 8]) };
        static LISTS: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
        static DRAWS: std::cell::Cell<std::time::Duration> = const { std::cell::Cell::new(std::time::Duration::ZERO) };
        static BAND_CPU: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        static FRAGS: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
    }
    let Some(every) = EVERY.with(|e| *e) else { return };
    let now = (kernel.vblanks, kernel.virtual_clock.unwrap_or(0), std::time::Instant::now());
    match LAST.with(std::cell::Cell::get) {
        None => LAST.with(|l| l.set(Some(now))),
        Some((vblank, game, wall)) if now.0 >= vblank + every => {
            let game_s = (now.1 - game) as f64 / 1e9;
            let wall_s = now.2.duration_since(wall).as_secs_f64();
            let phases = kernel.pica.phases;
            let before = PHASES.with(std::cell::Cell::get);
            PHASES.with(|p| p.set(phases));
            let d = |i: usize| (phases[i] - before[i]).as_secs_f64();
            let lists = (kernel.pica.list_time - LISTS.with(std::cell::Cell::get)).as_secs_f64();
            LISTS.with(|l| l.set(kernel.pica.list_time));
            let draws = (kernel.pica.draw_time - DRAWS.with(std::cell::Cell::get)).as_secs_f64();
            DRAWS.with(|l| l.set(kernel.pica.draw_time));
            let measured = d(0) + d(1) + d(2) + d(3) + d(4);
            let band_cpu_total = crate::pica::draw::BAND_CPU_NS.load(std::sync::atomic::Ordering::Relaxed);
            let band_cpu = (band_cpu_total - BAND_CPU.with(std::cell::Cell::get)) as f64 / 1e9;
            BAND_CPU.with(|b| b.set(band_cpu_total));
            let frags = (
                crate::pica::draw::SHADED.load(std::sync::atomic::Ordering::Relaxed),
                crate::pica::draw::ALPHA_REJECTED.load(std::sync::atomic::Ordering::Relaxed),
            );
            let before_frags = FRAGS.with(std::cell::Cell::get);
            FRAGS.with(|f| f.set(frags));
            let (shaded, rejected) = (frags.0 - before_frags.0, frags.1 - before_frags.1);
            println!(
                "speed          VBlanks {vblank}-{}: {:.2}x real time ({wall_s:.2} s: outside command lists {:.2}, in them {lists:.2}: between draws {:.2}, draws outside the phases {:.2}, vertex processing {:.2}, other vertex work {:.2}, textures {:.2}, bands {:.2} (cpu {band_cpu:.2}, {:.1} busy), buffers {:.2}; {shaded} fragments shaded, {rejected} rejected by the alpha test)",
                now.0,
                game_s / wall_s.max(1e-9),
                wall_s - lists,
                lists - draws,
                draws - measured,
                d(7),
                d(0) - d(7),
                d(2),
                d(3),
                band_cpu / d(3).max(1e-9),
                d(1) + d(4)
            );
            LAST.with(|l| l.set(Some(now)));
        }
        Some(_) => {}
    }
}

/// USUM_PROFILE=first:last (VBlanks): count guest calls in that window
/// (reported as `profile` lines, most frequent first).
fn profile_window(kernel: &Kernel) {
    thread_local! {
        static WINDOW: Option<(u64, u64)> = std::env::var("USUM_PROFILE").ok().and_then(|v| {
            let (a, b) = v.split_once(':')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        });
    }
    if let Some((first, last)) = WINDOW.with(|w| *w) {
        let on = (first..=last).contains(&kernel.vblanks);
        usum_guest::PROFILE.store(on, std::sync::atomic::Ordering::Relaxed);
    }
}

/// USUM_FRAMES_EVERY=n: write both screens every n VBlanks, as
/// probe-frames/NNNNNN-top.ppm and -bottom.ppm (game-derived: private).
fn snapshot(kernel: &mut Kernel, mem: &Memory) {
    thread_local! {
        static EVERY: Option<u64> = std::env::var("USUM_FRAMES_EVERY").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0);
    }
    let Some(every) = EVERY.with(|e| *e) else { return };
    if kernel.vblanks < kernel.snapshot_next {
        return;
    }
    kernel.snapshot_next = (kernel.vblanks / every + 1) * every;
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("probe-frames"))) else { return };
    let _ = std::fs::create_dir_all(&dir);
    for (screen, name, columns) in [(0, "top", 400), (1, "bottom", 320)] {
        if let Some(p) = kernel.presented.iter().rev().find(|p| p.screen == screen) {
            #[cfg(feature = "vulkan-compare")]
            if std::env::var_os("USUM_SCALE_TRACE").is_some() {
                if let Ok(raw) = framebuffer_raw(mem, p, columns) {
                    if let Some(frame) = kernel.pica.scaled_frame(p, columns, &raw) {
                        let mut ppm = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
                        match frame.rgb() {
                            Ok(rgb) => {
                                ppm.extend_from_slice(&rgb);
                                let _ = std::fs::write(dir.join(format!("{:06}-{name}-scaled.ppm", kernel.vblanks)), ppm);
                            }
                            Err(error) => eprintln!("cannot capture scaled display: {error}"),
                        }
                    }
                }
            }
            if let Ok(ppm) = framebuffer_ppm(mem, p, columns) {
                let _ = std::fs::write(dir.join(format!("{:06}-{name}.ppm", kernel.vblanks)), ppm);
            }
        }
    }
}

/// A presented framebuffer as a binary PPM. The LCDs scan 240-pixel
/// columns: framebuffer row y is screen column y, its pixel x the screen
/// row 239 - x.
fn framebuffer_ppm(mem: &Memory, p: &crate::kernel::Present, columns: u32) -> Result<Vec<u8>, String> {
    let mut out = format!("P6\n{columns} 240\n255\n").into_bytes();
    out.extend(framebuffer_rgb(mem, p, columns)?);
    Ok(out)
}

/// A presented framebuffer's bytes as stored, with what decoding them
/// needs: the pixel format and the stride.
#[derive(Clone, Debug)]
pub struct ScaledFrame {
    pub width: u32,
    pub height: u32,
    pixels: ScaledPixels,
}

#[derive(Clone, Debug)]
enum ScaledPixels {
    Cpu(std::sync::Arc<[u8]>),
    #[cfg(feature = "vulkan-compare")]
    Gpu(std::sync::Arc<usum_render::DisplayFrame>),
}

impl ScaledFrame {
    pub fn from_rgb(width: u32, height: u32, rgb: std::sync::Arc<[u8]>) -> Self {
        Self { width, height, pixels: ScaledPixels::Cpu(rgb) }
    }

    #[cfg(feature = "vulkan-compare")]
    pub(crate) fn from_gpu(frame: usum_render::DisplayFrame) -> Self {
        let (width, height) = frame.dimensions();
        Self { width, height, pixels: ScaledPixels::Gpu(std::sync::Arc::new(frame)) }
    }

    pub fn rgb(&self) -> Result<std::sync::Arc<[u8]>, String> {
        match &self.pixels {
            ScaledPixels::Cpu(rgb) => Ok(std::sync::Arc::clone(rgb)),
            #[cfg(feature = "vulkan-compare")]
            ScaledPixels::Gpu(frame) => frame.read_rgb().map(std::sync::Arc::clone),
        }
    }

    #[cfg(all(feature = "vulkan-compare", feature = "window"))]
    pub(crate) fn gpu(&self) -> Option<&std::sync::Arc<usum_render::DisplayFrame>> {
        match &self.pixels {
            ScaledPixels::Gpu(frame) => Some(frame),
            _ => None,
        }
    }
}

pub struct RawFrame {
    pub scaled: Option<ScaledFrame>,
    pub bytes: Vec<u8>,
    pub format: u32,
    pub stride: u32,
}

/// Copies a presented framebuffer out of guest memory (no conversion, so
/// a caller on a hot path can leave that to another thread).
pub fn framebuffer_raw(mem: &Memory, p: &crate::kernel::Present, columns: u32) -> Result<RawFrame, String> {
    let format = p.format & 7;
    crate::pica::bytes_per_pixel(format).ok_or(format!("framebuffer format {:#x}", p.format))?;
    let bytes = mem.read_range(p.left, p.stride * columns).map_err(|f| f.to_string())?;
    Ok(RawFrame { bytes, format, stride: p.stride, scaled: None })
}

/// A framebuffer as the LCD shows it: 240 rows of `columns` RGB pixels,
/// top row first. The 3DS stores it rotated: each stored row is one LCD
/// column, bottom pixel first.
pub fn raw_to_rgb(raw: &RawFrame, columns: u32) -> Vec<u8> {
    let bpp = crate::pica::bytes_per_pixel(raw.format).unwrap_or(4) as u32;
    let mut out = Vec::with_capacity((columns * 240 * 3) as usize);
    for row in 0..240u32 {
        for col in 0..columns {
            let at = (col * raw.stride + (239 - row) * bpp) as usize;
            let [r, g, b, _] =
                raw.bytes.get(at..at + bpp as usize).map_or([0; 4], |px| crate::pica::decode_pixel(raw.format, px));
            out.extend([r, g, b]);
        }
    }
    out
}

/// A presented framebuffer as the LCD shows it (`raw_to_rgb`).
pub fn framebuffer_rgb(mem: &Memory, p: &crate::kernel::Present, columns: u32) -> Result<Vec<u8>, String> {
    Ok(raw_to_rgb(&framebuffer_raw(mem, p, columns)?, columns))
}
