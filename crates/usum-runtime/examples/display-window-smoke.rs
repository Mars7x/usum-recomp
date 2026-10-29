//! Bounded, synthetic window/resize test. No game data or manual input required.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use usum_render::{DisplayFrame, DisplayScreen, RasterVertex, Renderer, WindowPresenter};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

struct App {
    renderer: &'static Renderer,
    window: Option<Arc<Window>>,
    presenter: Option<WindowPresenter>,
    frame: DisplayFrame,
    draws: u32,
    sizes: Vec<(u32, u32)>,
    started: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("USUM synthetic Vulkan resize check")
                        .with_inner_size(winit::dpi::PhysicalSize::new(800, 960)),
                )
                .unwrap(),
        );
        self.presenter = Some(self.renderer.create_window_presenter(window.clone()).unwrap());
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        assert!(self.started.elapsed() < Duration::from_secs(15), "window/resize test timed out");
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(10)));
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => panic!("synthetic window check was closed before completion"),
            WindowEvent::RedrawRequested => {
                let window = self.window.as_ref().unwrap();
                let size = window.inner_size();
                let screens = [DisplayScreen { frame: &self.frame, rect: [0, 0, size.width, size.height] }];
                if !self.presenter.as_mut().unwrap().present(size.width, size.height, &screens).unwrap() {
                    return;
                }
                assert!(!self.frame.ready(), "real surface presentation must not materialize RGB");
                let dims = (size.width, size.height);
                if !self.sizes.contains(&dims) {
                    self.sizes.push(dims);
                }
                self.draws += 1;
                if self.draws.is_multiple_of(8) {
                    let dims = [(400, 480), (1024, 768), (129, 77), (825, 998), (800, 960)];
                    let (w, h) = dims[(self.draws / 8 - 1) as usize % dims.len()];
                    let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                }
                if self.draws >= 48 {
                    assert!(self.sizes.len() >= 4, "resize requests were not exercised: {:?}", self.sizes);
                    eprintln!("Vulkan window: {} presentations at {:?}; no source materialization", self.draws, self.sizes);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let renderer = Box::leak(Box::new(Renderer::new().unwrap()));
    let rgb: Vec<_> = (0..37 * 29 * 3).map(|n| (n * 73 + 19) as u8).collect();
    let frame = renderer.upload_display_rgb(37, 29, &rgb).unwrap();
    let quit = Arc::new(AtomicBool::new(false));
    let worker = {
        let quit = Arc::clone(&quit);
        let renderer: &'static Renderer = renderer;
        std::thread::spawn(move || {
            let target = renderer.create_color_target(64, 64).unwrap();
            let mut draws = 0;
            while !quit.load(Ordering::Relaxed) {
                renderer
                    .draw_vertices(
                        &target,
                        &[
                            RasterVertex::new([-0.7, -0.7, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
                            RasterVertex::new([0.7, -0.7, -0.5, 1.0], [0.0, 1.0, 0.0, 1.0]),
                            RasterVertex::new([0.0, 0.7, -0.5, 1.0], [0.0, 0.0, 1.0, 1.0]),
                        ],
                    )
                    .unwrap();
                draws += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            draws
        })
    };
    let mut app = App { renderer, window: None, presenter: None, frame, draws: 0, sizes: Vec::new(), started: Instant::now() };
    EventLoop::new().unwrap().run_app(&mut app).unwrap();
    quit.store(true, Ordering::Relaxed);
    let draws = worker.join().unwrap();
    assert!(draws > 0);
    eprintln!("concurrent raster submissions: {draws}");
}
