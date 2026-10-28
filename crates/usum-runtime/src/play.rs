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

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use usum_guest::Code;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::probe::{self, Modules, Outcome};

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

/// The `main` of a generated play program: `play <dump>`.
pub fn main(functions: &'static [Code], entries: &'static [(u32, Code)], modules: Modules) -> ExitCode {
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
    let pad = Arc::new(AtomicU32::new(0));
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
            frames: Mutex::new([None, None]),
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
        shared,
        frames,
        pad,
        touch,
        cursor: None,
        pressing: false,
        quit: quit.clone(),
        title_stopped: false,
        draws: 0,
        rgb: (Vec::new(), Vec::new()),
        draw_time: Duration::ZERO,
    };
    let result = event_loop.run_app(&mut app);
    if app.draws > 0 {
        eprintln!(
            "window: {} redraws, {:.2} ms each on average",
            app.draws,
            app.draw_time.as_secs_f64() * 1000.0 / app.draws as f64
        );
    }
    quit.store(true, Ordering::Relaxed);
    // the game thread sees `quit` at its next budget refill, then writes
    // the input recording: wait for it
    let _ = game.join();
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
    (pad, touch): (Arc<AtomicU32>, Arc<AtomicU32>),
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
    if std::env::var_os("USUM_VULKAN_ACCELERATE").is_some() {
        kernel.pica.enable_vulkan_acceleration();
        if std::env::var_os("USUM_VULKAN_VERTEX").is_some() {
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
    kernel.live_pad = Some(pad);
    kernel.live_touch = Some(touch);
    kernel.frame_out = Some(frames);
    kernel.use_virtual_clock();
    let start = Instant::now();
    kernel.pace = Some(start);
    kernel.quit = Some(quit.clone());
    kernel.audio.sink = Some((sound.clone(), crate::sound::MAX_QUEUED));
    // USUM_PLAY_SECONDS=n: quit after n seconds (for checks)
    let stop_after = std::env::var("USUM_PLAY_SECONDS").ok().and_then(|v| v.parse().ok()).map(Duration::from_secs);
    let (outcome, _) = probe::run_with(&mut cpu, &mut mem, &mut kernel, entries, &modules, u64::MAX, &mut |kernel, _mem| {
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
    shared: Arc<Mutex<Shared>>,
    /// The screens, as the kernel hands them over at each VBlank.
    frames: Arc<crate::kernel::FrameOut>,
    pad: Arc<AtomicU32>,
    /// The touch screen (`hid::pack_touch`), the cursor's position in the
    /// window, and whether the left button is down.
    touch: Arc<AtomicU32>,
    cursor: Option<(f64, f64)>,
    pressing: bool,
    quit: Arc<AtomicBool>,
    title_stopped: bool,
    draws: u64,
    /// Time spent redrawing, for the report at exit.
    draw_time: Duration,
    /// The screens as last converted: top, bottom (RGB rows).
    rgb: (Vec<u8>, Vec<u8>),
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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, (): ()) {
        if self.quit.load(Ordering::Relaxed) {
            event_loop.exit();
            return;
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
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
                if let Some(bit) = pad_bit(key) {
                    match event.state {
                        ElementState::Pressed => self.pad.fetch_or(bit, Ordering::Relaxed),
                        ElementState::Released => self.pad.fetch_and(!bit, Ordering::Relaxed),
                    };
                }
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
        let started = Instant::now();
        self.redraw();
        self.draw_time += started.elapsed();
    }

    fn redraw(&mut self) {
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
        let stopped = self.shared.lock().ok().and_then(|s| s.stopped.clone());
        let [new_top, new_bottom] = match self.frames.frames.lock() {
            Ok(mut f) => [f[0].take(), f[1].take()],
            Err(_) => return,
        };
        if let Some(raw) = new_top {
            self.rgb.0 = probe::raw_to_rgb(&raw, TOP.0);
        }
        if let Some(raw) = new_bottom {
            self.rgb.1 = probe::raw_to_rgb(&raw, BOTTOM.0);
        }
        let screens = [(&self.rgb.0, TOP, (0, 0)), (&self.rgb.1, BOTTOM, ((CANVAS.0 - BOTTOM.0) / 2, TOP.1))];
        for (rgb, (sw, sh), (cx, cy)) in screens {
            if rgb.len() != (sw * sh * 3) as usize {
                continue;
            }
            // each source row scaled once into a line, then copied to the
            // `scale` window rows it covers
            let left = x0 + cx * scale;
            let visible = (sw * scale).min(width.saturating_sub(left)) as usize;
            let mut line = Vec::with_capacity(visible);
            for sy in 0..sh {
                line.clear();
                let src = &rgb[(sy * sw * 3) as usize..((sy + 1) * sw * 3) as usize];
                for px in src.as_chunks::<3>().0 {
                    let value = u32::from(px[0]) << 16 | u32::from(px[1]) << 8 | u32::from(px[2]);
                    line.extend(std::iter::repeat_n(value, scale as usize));
                }
                line.truncate(visible);
                for dy in 0..scale {
                    let row = y0 + (cy + sy) * scale + dy;
                    if row >= height {
                        break;
                    }
                    let at = (row * width + left) as usize;
                    buffer[at..at + visible].copy_from_slice(&line);
                }
            }
        }
        if let Some(why) = &stopped {
            if !self.title_stopped {
                window.set_title(&format!("usum-recomp — {why}"));
                self.title_stopped = true;
            }
        }
        // USUM_PLAY_SNAPSHOT=path: every 60 draws, the window's pixels as
        // a PPM (a check of what is shown; game-derived, private)
        self.draws += 1;
        if let Some(path) = std::env::var_os("USUM_PLAY_SNAPSHOT").filter(|_| self.draws.is_multiple_of(60)) {
            let mut ppm = format!("P6\n{width} {height}\n255\n").into_bytes();
            ppm.extend(buffer.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
            let _ = std::fs::write(path, ppm);
        }
        let _ = buffer.present();
    }
}
