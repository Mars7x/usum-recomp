//! A window to play in: the two screens, keyboard input, sound
//! (sound.rs), the game running live. Built only with the `window` feature (winit and softbuffer; see
//! DEPENDENCIES.md).
//!
//! The game runs on its own thread on the virtual clock, throttled so it
//! never gets ahead of real time; when the host is too slow it runs in
//! slow motion instead of dropping frames. At each VBlank the newest
//! framebuffers are copied out for the window. Saves go to the user's
//! data directory, by title, apart from the dump and everything
//! generated.
//!
//! Keys (physical positions, so the layout does not matter):
//! arrows: circle pad; I J K L: D-pad; X: A; Z: B; S: X; A: Y; Q: L;
//! W: R; Enter: Start; Backspace: Select; Escape: quit. The left mouse
//! button on the bottom screen touches it.

mod controller;
mod hud;

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use usum_guest::Code;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::probe::{self, Modules, Outcome};
use crate::timing::{FrameSample, FrameStamp, FrameTelemetry, PresentationPacer, PresentationRate};

const TOP: (u32, u32) = (400, 240);
const BOTTOM: (u32, u32) = (320, 240);
/// The canvas: the top screen above the bottom one, centred.
const CANVAS: (u32, u32) = (400, 480);

/// The latest screens, as RGB rows, and what stopped the game, if it has.
#[derive(Default)]
struct Shared {
    stopped: Option<String>,
}

/// PAD bits per key (services::hid::BUTTONS).
fn pad_bit(key: KeyCode) -> Option<u32> {
    Some(match key {
        KeyCode::KeyX => 1 << 0,
        KeyCode::KeyZ => 1 << 1,
        KeyCode::Backspace => 1 << 2,
        KeyCode::Enter => 1 << 3,
        KeyCode::KeyL => 1 << 4,
        KeyCode::KeyJ => 1 << 5,
        KeyCode::KeyI => 1 << 6,
        KeyCode::KeyK => 1 << 7,
        KeyCode::KeyW => 1 << 8,
        KeyCode::KeyQ => 1 << 9,
        KeyCode::KeyS => 1 << 10,
        KeyCode::KeyA => 1 << 11,
        KeyCode::ArrowRight => 1 << 28,
        KeyCode::ArrowLeft => 1 << 29,
        KeyCode::ArrowUp => 1 << 30,
        KeyCode::ArrowDown => 1 << 31,
        _ => return None,
    })
}

/// Where saves go: $XDG_DATA_HOME (or ~/.local/share)/usum-recomp/saves/
/// <program ID>.
fn save_dir(program_id: u64) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    Some(base.join("usum-recomp/saves").join(format!("{program_id:016x}")))
}

/// Where a session's recording goes: $XDG_CACHE_HOME/usum-recomp/
/// recordings/play-<seconds since 1970> (.txt for the input, .save/ for
/// the saves as they were when the session started).
fn recording_base() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Some(base.join("usum-recomp/recordings").join(format!("play-{stamp}")))
}

/// Copies directory `from` (the saves) to `to`, recursively.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Writes the session's input (as HID read it) as an input script beside
/// the saves it started from. The probe replays it exactly:
/// `USUM_SAVE_FROM=<base>.save USUM_INPUT="$(cat <base>.txt)"`. It holds
/// only buttons and touches by VBlank.
fn save_recording(kernel: &crate::kernel::Kernel, base: Option<&std::path::Path>, vblanks: u64) {
    let Some(base) = base.filter(|_| !kernel.recorded.is_empty()) else { return };
    let path = base.with_extension("txt");
    let Some(dir) = path.parent() else { return };
    let script = crate::services::hid::format_script(&kernel.recorded);
    match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, script)) {
        Ok(()) => eprintln!("input recorded: {} ({vblanks} VBlanks)", path.display()),
        Err(e) => eprintln!("input not recorded: {e}"),
    }
}

#[cfg(feature = "vulkan-compare")]
fn compare_vulkan_captures(kernel: &mut crate::kernel::Kernel) {
    let snapshots = kernel.pica.take_vulkan_snapshots();
    if snapshots.is_empty() {
        return;
    }
    let renderer = match usum_render::Renderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("Vulkan comparison unavailable: {error}");
            return;
        }
    };
    eprintln!("Vulkan comparison adapter: {}", renderer.adapter_info().name);
    for (index, snapshot) in snapshots.iter().enumerate() {
        match snapshot.compare(&renderer) {
            Ok(comparison) => eprintln!(
                "Vulkan draw {} (guest #{}, {} triangles, lighting {}): {}/{} pixels differ, {} over 1; maximum delta {} at {:?}, Vulkan {:?}, software {:?}; depth {} differ, maximum {} at {:?}",
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
                comparison.maximum_depth_at
            ),
            Err(error) => eprintln!("Vulkan draw {} unsupported: {error}", index + 1),
        }
    }
}

// Explicit live-comparison modes keep software rendering active. Otherwise a
// normal Vulkan-capable build accelerates by default, with 0 as the opt-out.
fn acceleration_policy(setting: Option<&str>, live_comparison: bool) -> bool {
    setting.map_or(!live_comparison, |value| value != "0")
}

#[cfg(test)]
#[test]
fn default_acceleration_preserves_explicit_software_and_live_comparison_modes() {
    assert!(acceleration_policy(None, false));
    assert!(!acceleration_policy(Some("0"), false));
    assert!(!acceleration_policy(None, true));
    assert!(!acceleration_policy(Some("0"), true));
    assert!(acceleration_policy(Some("1"), false));
    assert!(acceleration_policy(Some("1"), true));
}

fn vulkan_enabled() -> bool {
    cfg!(feature = "vulkan-compare")
        && acceleration_policy(
            std::env::var("USUM_VULKAN_ACCELERATE").ok().as_deref(),
            std::env::var_os("USUM_VULKAN_LIVE").is_some() || std::env::var_os("USUM_VULKAN_LIVE_DEPTH").is_some(),
        )
}

/// The `main` of a generated play program: `play <dump>`.
pub fn main(functions: &'static [Code], entries: &'static [(u32, Code)], modules: Modules) -> ExitCode {
    let frontend_started = Instant::now();
    let presentation_rate = match std::env::var("USUM_PRESENT_FPS") {
        Ok(value) => match value.parse::<PresentationRate>() {
            Ok(rate) => rate,
            Err(error) => {
                eprintln!("USUM_PRESENT_FPS: {error}");
                return ExitCode::FAILURE;
            }
        },
        Err(_) => PresentationRate::Auto,
    };
    eprintln!(
        "presentation cap: {presentation_rate}; guest clock remains at normal speed (F6 cap, F5 title statistics, F7 testing FPS counter)"
    );
    if let Err(error) = controller::configure_discovery() {
        eprintln!("cannot select container controller discovery: {error}; using default discovery");
    }
    let resolution_scale = match crate::pica::resolution_scale_env() {
        Ok(scale) if scale == 1 || vulkan_enabled() => scale,
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
        eprintln!("usage: play <dump>");
        return ExitCode::FAILURE;
    };
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("no window system: {e}");
            return ExitCode::FAILURE;
        }
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let pad = Arc::new(AtomicU64::new(0));
    let touch = Arc::new(AtomicU32::new(0));
    let quit = Arc::new(AtomicBool::new(false));
    let proxy = event_loop.create_proxy();
    let sound: crate::sound::Queue = Arc::default();
    let sound_thread = {
        let (sound, quit) = (sound.clone(), quit.clone());
        let started = std::thread::Builder::new().name("sound".into()).spawn(move || {
            eprintln!("{}", crate::sound::play(&sound, &quit));
        });
        started.map_err(|e| eprintln!("no sound: {e}")).ok()
    };
    // the kernel copies the screens here at every VBlank and wakes the
    // window
    let frames = {
        let proxy = proxy.clone();
        Arc::new(crate::kernel::FrameOut {
            frames: Mutex::default(),
            resolution_scale: AtomicU32::new(resolution_scale),
            dialog: Mutex::default(),
            notify: Box::new(move || {
                let _ = proxy.send_event(());
            }),
        })
    };
    let game = {
        let (shared, pad, touch, quit, sound) = (shared.clone(), pad.clone(), touch.clone(), quit.clone(), sound.clone());
        let frames = frames.clone();
        let dump = std::path::PathBuf::from(dump);
        // translated code recurses on the host stack: give it room
        std::thread::Builder::new()
            .name("game".into())
            .stack_size(256 << 20)
            .spawn(move || run_game(&dump, functions, entries, modules, &shared, (pad, touch), &sound, quit, &proxy, frames))
    };
    let game = match game {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("cannot start the game thread: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut app = App {
        window: None,
        surface: None,
        #[cfg(feature = "vulkan-compare")]
        gpu_presenter: None,
        #[cfg(feature = "vulkan-compare")]
        gpu_frames: [None, None],
        #[cfg(feature = "vulkan-compare")]
        gpu_versions: [0; 2],
        #[cfg(feature = "vulkan-compare")]
        gpu_distinct: [0; 2],
        #[cfg(feature = "vulkan-compare")]
        gpu_uploads: 0,
        #[cfg(feature = "vulkan-compare")]
        gpu_mapped_uses: 0,
        shared,
        frames,
        pad,
        keyboard_pad: 0,
        controllers: controller::Controllers::new(),
        focused: true,
        touch,
        cursor: None,
        pressing: false,
        quit: quit.clone(),
        draws: 0,
        rgb: (Arc::from([]), Arc::from([])),
        rgb_dimensions: [TOP, BOTTOM],
        draw_time: Duration::ZERO,
        started: frontend_started,
        pacer: PresentationPacer::new(presentation_rate),
        telemetry: FrameTelemetry::default(),
        sample: FrameSample::default(),
        show_stats: std::env::var("USUM_FRAME_STATS").as_deref() == Ok("1"),
        log_stats: std::env::var("USUM_FRAME_LOG").as_deref() == Ok("1"),
        rgb_versions: [0; 2],
        displayed_top: 0,
        last_title: String::new(),
        test_fps: std::env::var("USUM_TEST_FPS").as_deref() == Ok("1"),
        hud_text: String::new(),
        hud_bitmap: None,
        #[cfg(feature = "vulkan-compare")]
        hud_gpu: None,
    };
    let result = event_loop.run_app(&mut app);
    if app.draws > 0 {
        eprintln!(
            "window: {} redraws, {:.2} ms each on average",
            app.draws,
            app.draw_time.as_secs_f64() * 1000.0 / app.draws as f64
        );
    }
    #[cfg(feature = "vulkan-compare")]
    if app.gpu_distinct != [0, 0] || app.gpu_uploads != 0 {
        eprintln!(
            "window GPU: {} top / {} bottom distinct resident buffers, {} CPU uploads, {} materialized source uses",
            app.gpu_distinct[0], app.gpu_distinct[1], app.gpu_uploads, app.gpu_mapped_uses
        );
    }
    quit.store(true, Ordering::Relaxed);
    // the game thread sees `quit` at its next budget refill, then writes
    // the input recording: wait for it
    let _ = game.join();
    eprintln!("{}", app.telemetry.summary(app.started.elapsed(), app.frame_stamp()));
    // the server asks for data every few milliseconds, so the sound
    // thread sees `quit` promptly
    if let Some(t) = sound_thread {
        let _ = t.join();
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_game(
    dump: &std::path::Path,
    functions: &'static [Code],
    entries: &'static [(u32, Code)],
    modules: Modules,
    shared: &Mutex<Shared>,
    (pad, touch): (Arc<AtomicU64>, Arc<AtomicU32>),
    sound: &crate::sound::Queue,
    quit: Arc<AtomicBool>,
    proxy: &EventLoopProxy<()>,
    frames: Arc<crate::kernel::FrameOut>,
) {
    let stop = |why: String| {
        eprintln!("{why}");
        if let Ok(mut s) = shared.lock() {
            s.stopped = Some(why);
        }
        let _ = proxy.send_event(());
    };
    let (mut cpu, mut mem, mut kernel, program_id) = match probe::setup(dump, functions, &modules) {
        Ok(state) => state,
        Err(e) => return stop(e),
    };
    #[cfg(feature = "vulkan-compare")]
    if let Some(draws) = std::env::var("USUM_VULKAN_CAPTURE").ok().and_then(|value| value.parse::<usize>().ok()) {
        let draws = draws.min(64);
        let skip = std::env::var("USUM_VULKAN_SKIP").ok().and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
        kernel.pica.enable_vulkan_capture_after(skip, draws);
        eprintln!("capturing {draws} draws for Vulkan comparison after skipping {skip}");
    }
    #[cfg(feature = "vulkan-compare")]
    if vulkan_enabled() {
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
        let depth_feedback =
            std::env::var("USUM_VULKAN_DEPTH_FEEDBACK").or_else(|_| std::env::var("USUM_VULKAN_CPU_DEPTH_FEEDBACK"));
        if depth_feedback.as_deref() != Ok("0") {
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
    kernel.save_dir = save_dir(program_id);
    // the saves as the session starts, so its input can be replayed
    let recording = recording_base();
    if let (Some(base), Some(saves)) = (&recording, &kernel.save_dir) {
        if saves.exists() {
            if let Err(e) = copy_dir(saves, &base.with_extension("save")) {
                eprintln!("saves not copied for the recording: {e}");
            }
        }
    }
    let replay = std::env::var("USUM_PLAY_REPLAY").as_deref() == Ok("1");
    if replay {
        let configure = || -> Result<_, String> {
            let text = match std::env::var("USUM_INPUT").ok() {
                Some(text) => text,
                None => match std::env::var_os("USUM_INPUT_FILE") {
                    Some(path) => std::fs::read_to_string(&path).map_err(|e| format!("replay input: {e}"))?,
                    None => String::new(),
                },
            };
            let steps = crate::services::hid::parse_script(&text)?;
            let seconds = std::env::var("USUM_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(10);
            Ok((steps, seconds))
        };
        match configure() {
            Ok((steps, seconds)) => {
                kernel.input = steps;
                kernel.sleep_limit = Some(Duration::from_secs(seconds));
            }
            Err(error) => {
                stop(error);
                quit.store(true, Ordering::Relaxed);
                let _ = proxy.send_event(());
                return;
            }
        }
    } else {
        kernel.live_pad = Some(pad);
        kernel.live_touch = Some(touch);
    }
    kernel.frame_out = Some(frames);
    kernel.use_virtual_clock();
    let start = Instant::now();
    kernel.pace = Some(start);
    kernel.quit = Some(quit.clone());
    kernel.audio.sink = Some((sound.clone(), crate::sound::MAX_QUEUED));
    // USUM_PLAY_SECONDS=n: quit after n seconds (for checks)
    let stop_after = std::env::var("USUM_PLAY_SECONDS").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs);
    let (outcome, ran) = probe::run_with(&mut cpu, &mut mem, &mut kernel, entries, &modules, u64::MAX, &mut |kernel, _mem| {
        // USUM_SPEED=n, as in the probe
        probe::speed_report(kernel);
        if quit.load(Ordering::Relaxed) || stop_after.is_some_and(|d| start.elapsed() >= d) {
            quit.store(true, Ordering::Relaxed);
            let _ = proxy.send_event(());
            return false;
        }

        // never ahead of real time
        if let Some(now) = kernel.virtual_clock {
            let ahead = Duration::from_nanos(now).saturating_sub(start.elapsed());
            if !ahead.is_zero() {
                std::thread::sleep(ahead.min(Duration::from_millis(50)));
            }
        }
        true
    });
    if replay {
        eprintln!(
            "window replay: {ran} instructions, {} VBlanks, {} draws, {} triangles in {:.3} s; {outcome:?}",
            kernel.vblanks,
            kernel.pica.draws,
            kernel.pica.triangles,
            start.elapsed().as_secs_f64()
        );
    }
    #[cfg(feature = "vulkan-compare")]
    {
        let (lists, draws, targets, time) = kernel.pica.vulkan_live_stats();
        if lists != 0 {
            eprintln!(
                "Vulkan live: {draws} draws across {targets} targets in {lists} command lists, {:.3} s",
                time.as_secs_f64()
            );
            let (texture_uses, texture_uploads, lighting_uses, lighting_uploads) = kernel.pica.vulkan_live_resource_stats();
            eprintln!(
                "Vulkan resources: {texture_uploads}/{texture_uses} texture uploads/uses, {lighting_uploads}/{lighting_uses} lighting uploads/uses"
            );
            eprintln!("Vulkan feedback reads within command lists: {}", kernel.pica.vulkan_live_feedback_reads());
            let (accelerated, fallbacks) = kernel.pica.vulkan_acceleration_stats();
            if accelerated != 0 || fallbacks != 0 {
                eprintln!("Vulkan acceleration: {accelerated} command lists, {fallbacks} software fallbacks");
            }
        }
    }
    #[cfg(feature = "vulkan-compare")]
    compare_vulkan_captures(&mut kernel);
    save_recording(&kernel, recording.as_deref(), kernel.vblanks);
    if replay {
        quit.store(true, Ordering::Relaxed);
        let _ = proxy.send_event(());
        return;
    }
    // quitting from a paced wait ends the run as "no thread": not an error
    if quit.load(Ordering::Relaxed) {
        return;
    }
    let why = match outcome {
        Outcome::Stopped => return,
        Outcome::Unimplemented(what) => format!("stopped: {what} is not implemented (pc {:#010x})", cpu.r[15]),
        Outcome::Fault(f) => format!("stopped: {f}"),
        Outcome::NoThread => "stopped: no thread can run".to_owned(),
        Outcome::OutOfBudget | Outcome::TimeLimit => "stopped".to_owned(),
    };
    stop(why);
}

struct App {
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    #[cfg(feature = "vulkan-compare")]
    gpu_presenter: Option<usum_render::WindowPresenter>,
    #[cfg(feature = "vulkan-compare")]
    gpu_frames: [Option<Arc<usum_render::DisplayFrame>>; 2],
    #[cfg(feature = "vulkan-compare")]
    gpu_versions: [u64; 2],
    #[cfg(feature = "vulkan-compare")]
    gpu_distinct: [u64; 2],
    #[cfg(feature = "vulkan-compare")]
    gpu_uploads: u64,
    #[cfg(feature = "vulkan-compare")]
    gpu_mapped_uses: u64,
    shared: Arc<Mutex<Shared>>,
    /// The screens, as the kernel hands them over at each VBlank.
    frames: Arc<crate::kernel::FrameOut>,
    pad: Arc<AtomicU64>,
    keyboard_pad: u32,
    controllers: controller::Controllers,
    focused: bool,
    /// The touch screen (`hid::pack_touch`), the cursor's position in the
    /// window, and whether the left button is down.
    touch: Arc<AtomicU32>,
    cursor: Option<(f64, f64)>,
    pressing: bool,
    quit: Arc<AtomicBool>,
    draws: u64,
    /// Time spent redrawing, for the report at exit.
    draw_time: Duration,
    /// The screens as last converted: top, bottom (RGB rows).
    rgb: (Arc<[u8]>, Arc<[u8]>),
    rgb_dimensions: [(u32, u32); 2],
    rgb_versions: [u64; 2],
    displayed_top: u64,
    started: Instant,
    pacer: PresentationPacer,
    telemetry: FrameTelemetry,
    sample: FrameSample,
    show_stats: bool,
    log_stats: bool,
    last_title: String,
    test_fps: bool,
    hud_text: String,
    hud_bitmap: Option<hud::Bitmap>,
    #[cfg(feature = "vulkan-compare")]
    hud_gpu: Option<Arc<usum_render::DisplayFrame>>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("usum-recomp")
            .with_inner_size(winit::dpi::LogicalSize::new(CANVAS.0 * 2, CANVAS.1 * 2));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("cannot open a window: {e}");
                event_loop.exit();
                return;
            }
        };
        #[cfg(feature = "vulkan-compare")]
        if vulkan_enabled() && std::env::var("USUM_VULKAN_GPU_PRESENT").as_deref() != Ok("0") {
            match crate::pica::Pica::window_renderer().and_then(|r| r.create_window_presenter(window.clone())) {
                Ok(presenter) => {
                    eprintln!("window: Vulkan resident LCD presentation");
                    self.gpu_presenter = Some(presenter);
                    self.window = Some(window);
                    return;
                }
                Err(error) => eprintln!("Vulkan window unavailable, using CPU presentation: {error}"),
            }
        }
        let surface = softbuffer::Context::new(window.clone()).and_then(|c| softbuffer::Surface::new(&c, window.clone()));
        match surface {
            Ok(s) => self.surface = Some(s),
            Err(e) => {
                eprintln!("cannot draw in the window: {e}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.publish_inputs();
        self.request_pending_redraw();
        let stamp = self.frame_stamp();
        if let Some(sample) = self.telemetry.report(self.started.elapsed(), stamp) {
            self.sample = sample;
            if self.log_stats {
                eprintln!("{sample}");
            }
            self.update_title();
            // The testing counter must fall to zero when the guest stops
            // publishing frames. These HUD-only redraws count as repeats,
            // never as game updates, and still respect the presentation cap.
            if self.test_fps {
                self.pacer.request();
                self.request_pending_redraw();
            }
        }
        // Input polling remains independent of the presentation cap.
        let poll = Instant::now() + Duration::from_millis(8);
        let next = self.pacer.deadline().map_or(poll, |due| (self.started + due).min(poll));
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(next));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
        if self.quit.load(Ordering::Relaxed) {
            event_loop.exit();
            return;
        }
        self.pacer.request();
        self.request_pending_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.quit.store(true, Ordering::Relaxed);
                event_loop.exit();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(key) = event.physical_key else { return };
                if key == KeyCode::Escape {
                    self.quit.store(true, Ordering::Relaxed);
                    event_loop.exit();
                    return;
                }
                if event.state == ElementState::Pressed && !event.repeat {
                    match key {
                        KeyCode::F5 => {
                            self.show_stats = !self.show_stats;
                            self.update_title();
                        }
                        KeyCode::F6 => {
                            self.pacer.set_rate(self.pacer.rate().next_preset());
                            eprintln!("presentation cap: {}; guest clock remains at normal speed", self.pacer.rate());
                            self.update_title();
                            self.request_pending_redraw();
                        }
                        KeyCode::F7 => {
                            self.test_fps = !self.test_fps;
                            self.pacer.request();
                            self.request_pending_redraw();
                        }
                        _ => {}
                    }
                }
                if event.state == ElementState::Pressed
                    && matches!(key, KeyCode::F3 | KeyCode::F4)
                    && cfg!(feature = "vulkan-compare")
                    && vulkan_enabled()
                {
                    let scale = self.frames.resolution_scale.load(Ordering::Relaxed);
                    let scale = if key == KeyCode::F3 { scale.saturating_sub(1).max(1) } else { (scale + 1).min(8) };
                    self.frames.resolution_scale.store(scale, Ordering::Relaxed);
                }
                if let Some(bit) = pad_bit(key) {
                    match event.state {
                        ElementState::Pressed if self.focused => self.keyboard_pad |= bit,
                        ElementState::Released => self.keyboard_pad &= !bit,
                        _ => {}
                    };
                    self.publish_inputs();
                }
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if !focused {
                    self.keyboard_pad = 0;
                    self.pressing = false;
                    self.touch.store(0, Ordering::Relaxed);
                }
                self.publish_inputs();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                self.update_touch();
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.update_touch();
            }
            WindowEvent::MouseInput { state, button: winit::event::MouseButton::Left, .. } => {
                self.pressing = state == ElementState::Pressed;
                self.update_touch();
            }
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }
}

impl App {
    fn frame_stamp(&self) -> FrameStamp {
        self.frames.frames.lock().map(|frames| frames.stamp).unwrap_or_default()
    }

    fn request_pending_redraw(&mut self) {
        if let Some(window) = &self.window {
            if self.pacer.queue(self.started.elapsed()) {
                window.request_redraw();
            }
        }
    }

    fn update_title(&mut self) {
        let Some(window) = &self.window else { return };
        let stopped = self.shared.lock().ok().and_then(|shared| shared.stopped.clone());
        let title = if let Some(why) = stopped {
            format!("usum-recomp — {why}")
        } else {
            let scale = self.frames.resolution_scale.load(Ordering::Relaxed);
            if self.show_stats {
                format!("usum-recomp — {scale}× — updates {:.1}/s | present {:.1}/s | speed {:.0}% | p95 {:.1} ms | cap {} — F5 stats / F6 cap / F7 FPS",
                    self.sample.updates_per_second, self.sample.presents_per_second, self.sample.game_speed * 100.0,
                    self.sample.interval_p95_ms, self.pacer.rate())
            } else {
                format!(
                    "usum-recomp — {scale}× ({}×{} top) — F3/F4 scale — cap {} — F5 stats / F6 cap / F7 FPS",
                    400 * scale,
                    240 * scale,
                    self.pacer.rate()
                )
            }
        };
        if title != self.last_title {
            window.set_title(&title);
            self.last_title = title;
        }
    }

    fn publish_inputs(&mut self) {
        let controller = self.controllers.poll();
        let input = if self.focused { controller::combine(self.keyboard_pad, controller) } else { 0 };
        self.pad.store(input, Ordering::Relaxed);
    }

    /// Where the canvas lies in the window: (scale, x0, y0).
    fn layout(width: u32, height: u32) -> (u32, u32, u32) {
        let scale = (width / CANVAS.0).min(height / CANVAS.1).max(1);
        (scale, width.saturating_sub(CANVAS.0 * scale) / 2, height.saturating_sub(CANVAS.1 * scale) / 2)
    }

    /// The touch screen follows the left button over the bottom screen.
    fn update_touch(&self) {
        let point = match (self.pressing, self.cursor, &self.window) {
            (true, Some((x, y)), Some(w)) => {
                let size = w.inner_size();
                let (scale, x0, y0) = Self::layout(size.width, size.height);
                let bx = (x - f64::from(x0)) / f64::from(scale) - f64::from((CANVAS.0 - BOTTOM.0) / 2);
                let by = (y - f64::from(y0)) / f64::from(scale) - f64::from(TOP.1);
                ((0.0..f64::from(BOTTOM.0)).contains(&bx) && (0.0..f64::from(BOTTOM.1)).contains(&by))
                    .then_some((bx as u16, by as u16))
            }
            _ => None,
        };
        self.touch.store(crate::services::hid::pack_touch(point), Ordering::Relaxed);
    }

    fn draw(&mut self) {
        let attempt = self.started.elapsed();
        if !self.pacer.begin(attempt) {
            return;
        }
        let previous_draws = self.draws;
        let started = Instant::now();
        let dialog = self.frames.dialog.lock().ok().and_then(|text| text.clone());
        let text = dialog.unwrap_or_else(|| {
            if self.test_fps {
                format!("FPS {:.1}\nWIN {:.1}", self.sample.updates_per_second, self.sample.presents_per_second)
            } else {
                String::new()
            }
        });
        if self.hud_text != text {
            self.hud_bitmap = (!text.is_empty()).then(|| hud::Bitmap::new(&text));
            self.hud_text = text;
            #[cfg(feature = "vulkan-compare")]
            {
                self.hud_gpu = None;
            }
        }
        self.redraw();
        let took = started.elapsed();
        self.draw_time += took;
        let now = self.started.elapsed();
        self.pacer.finish(attempt);
        if self.draws != previous_draws {
            self.telemetry.presented(now, took, self.displayed_top);
        }
        self.update_title();
    }

    fn redraw(&mut self) {
        #[cfg(feature = "vulkan-compare")]
        if self.gpu_presenter.is_some() {
            let before = self.draws;
            match self.redraw_gpu() {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("Vulkan window failed, using CPU presentation: {error}");
                    self.gpu_presenter = None;
                    // Preserve the last complete immutable screens when changing
                    // presenter, even if the game has no newer frame queued.
                    for (screen, frame) in self.gpu_frames.iter().enumerate() {
                        if let Some(frame) = frame {
                            if let Ok(rgb) = frame.read_rgb() {
                                if screen == 0 {
                                    self.rgb.0 = Arc::clone(rgb);
                                } else {
                                    self.rgb.1 = Arc::clone(rgb);
                                }
                                self.rgb_dimensions[screen] = frame.dimensions();
                                self.rgb_versions[screen] = self.gpu_versions[screen];
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        self.surface = softbuffer::Context::new(window.clone())
                            .and_then(|c| softbuffer::Surface::new(&c, window.clone()))
                            .ok();
                    }
                    // If presentation succeeded before an optional capture
                    // failed, switch backend on the next frame, without a
                    // second present of the same redraw.
                    if self.draws != before {
                        return;
                    }
                }
            }
        }
        self.redraw_cpu();
    }

    #[cfg(feature = "vulkan-compare")]
    fn redraw_gpu(&mut self) -> Result<(), String> {
        let Some(window) = &self.window else {
            return Ok(());
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let renderer = crate::pica::Pica::window_renderer()?;
        let (new_frames, versions) = {
            let mut frames = self.frames.frames.lock().map_err(|e| e.to_string())?;
            frames.take()
        };
        for (screen, raw) in new_frames.into_iter().enumerate() {
            let Some(raw) = raw else {
                continue;
            };
            self.gpu_frames[screen] = Some(match &raw.scaled {
                Some(frame) => match frame.gpu() {
                    Some(gpu) => {
                        if self.gpu_frames[screen].as_ref().is_none_or(|old| !Arc::ptr_eq(old, gpu)) {
                            self.gpu_distinct[screen] += 1;
                        }
                        Arc::clone(gpu)
                    }
                    None => {
                        self.gpu_uploads += 1;
                        Arc::new(renderer.upload_display_rgb(frame.width, frame.height, &frame.rgb()?)?)
                    }
                },
                None => {
                    self.gpu_uploads += 1;
                    let (width, height) = if screen == 0 { TOP } else { BOTTOM };
                    Arc::new(renderer.upload_display_rgb(width, height, &probe::raw_to_rgb(&raw, width))?)
                }
            });
            self.gpu_versions[screen] = versions[screen];
        }
        let (scale, x0, y0) = Self::layout(size.width, size.height);
        if self.hud_gpu.is_none() {
            if let Some(bitmap) = &self.hud_bitmap {
                self.hud_gpu = Some(Arc::new(renderer.upload_display_rgb(bitmap.width, bitmap.height, &bitmap.rgb)?));
            }
        }
        let mut screens: Vec<_> = self
            .gpu_frames
            .iter()
            .enumerate()
            .filter_map(|(screen, frame)| {
                frame.as_ref().map(|frame| {
                    let (width, height, cx, cy) =
                        if screen == 0 { (TOP.0, TOP.1, 0, 0) } else { (BOTTOM.0, BOTTOM.1, (CANVAS.0 - BOTTOM.0) / 2, TOP.1) };
                    usum_render::DisplayScreen { frame, rect: [x0 + cx * scale, y0 + cy * scale, width * scale, height * scale] }
                })
            })
            .collect();
        self.gpu_mapped_uses += screens.iter().filter(|screen| screen.frame.ready()).count() as u64;
        if let (Some(bitmap), Some(frame)) = (&self.hud_bitmap, &self.hud_gpu) {
            screens.push(usum_render::DisplayScreen { frame, rect: [hud::ORIGIN, hud::ORIGIN, bitmap.width, bitmap.height] });
        }
        let presenter = self.gpu_presenter.as_mut().ok_or("Vulkan presenter disappeared")?;
        if !presenter.present(size.width, size.height, &screens)? {
            return Ok(());
        }
        self.displayed_top = self.gpu_versions[0];
        self.draws += 1;
        if let Some(path) = std::env::var_os("USUM_PLAY_SNAPSHOT").filter(|_| self.draws.is_multiple_of(60)) {
            let rgb = presenter.capture_rgb(size.width, size.height, &screens)?;
            let mut ppm = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
            ppm.extend_from_slice(&rgb);
            // Match the software presenter's diagnostic policy: a failed
            // screenshot write must not change the active display backend.
            let _ = std::fs::write(path, ppm);
        }
        Ok(())
    }

    fn redraw_cpu(&mut self) {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else { return };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else { return };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else { return };
        buffer.fill(0);
        let (width, height) = (size.width, size.height);
        // the largest whole scale that fits, centred
        let (scale, x0, y0) = Self::layout(width, height);
        // take new frames under the lock, convert them after releasing it,
        // so the game thread never waits for the window
        let ([new_top, new_bottom], versions) = match self.frames.frames.lock() {
            Ok(mut f) => f.take(),
            Err(_) => return,
        };
        for (screen, raw) in [new_top, new_bottom].into_iter().enumerate() {
            if let Some(raw) = raw {
                let (pixels, dims) = match &raw.scaled {
                    Some(frame) => match frame.rgb() {
                        Ok(rgb) => (rgb, (frame.width, frame.height)),
                        Err(error) => {
                            eprintln!("cannot read scaled display: {error}");
                            continue;
                        }
                    },
                    None => (
                        probe::raw_to_rgb(&raw, if screen == 0 { TOP.0 } else { BOTTOM.0 }).into(),
                        if screen == 0 { TOP } else { BOTTOM },
                    ),
                };
                if screen == 0 {
                    self.rgb.0 = pixels;
                } else {
                    self.rgb.1 = pixels;
                }
                self.rgb_dimensions[screen] = dims;
                self.rgb_versions[screen] = versions[screen];
            }
        }
        let screens = [
            (&self.rgb.0, TOP, (0, 0), self.rgb_dimensions[0]),
            (&self.rgb.1, BOTTOM, ((CANVAS.0 - BOTTOM.0) / 2, TOP.1), self.rgb_dimensions[1]),
        ];
        for (rgb, (native_w, native_h), (cx, cy), (sw, sh)) in screens {
            if rgb.len() != (sw * sh * 3) as usize {
                continue;
            }
            let (left, top) = (x0 + cx * scale, y0 + cy * scale);
            if left >= width || top >= height {
                continue;
            }
            let (dw, dh) = (native_w * scale, native_h * scale);
            let visible = dw.min(width - left);
            let mut line = Vec::with_capacity(visible as usize);
            let mut previous_source = None;
            for dy in 0..dh.min(height - top) {
                let sy = (u64::from(dy) * u64::from(sh) / u64::from(dh)) as u32;
                if previous_source != Some(sy) {
                    line.clear();
                    for dx in 0..visible {
                        let sx = (u64::from(dx) * u64::from(sw) / u64::from(dw)) as u32;
                        let at = ((sy * sw + sx) * 3) as usize;
                        line.push(u32::from(rgb[at]) << 16 | u32::from(rgb[at + 1]) << 8 | u32::from(rgb[at + 2]));
                    }
                    previous_source = Some(sy);
                }
                let at = ((top + dy) * width + left) as usize;
                buffer[at..at + visible as usize].copy_from_slice(&line);
            }
        }
        // USUM_PLAY_SNAPSHOT=path: every 60 draws, the window's pixels as
        // a PPM (a check of what is shown; game-derived, private)
        if let Some(bitmap) = &self.hud_bitmap {
            bitmap.composite(&mut buffer, width, height);
        }
        if let Some(path) = std::env::var_os("USUM_PLAY_SNAPSHOT").filter(|_| (self.draws + 1).is_multiple_of(60)) {
            let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
            ppm.extend(buffer.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
            let _ = std::fs::write(path, ppm);
        }
        if buffer.present().is_ok() {
            self.draws += 1;
            self.displayed_top = self.rgb_versions[0];
        }
    }
}
