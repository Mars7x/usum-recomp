//! Hardware check: frozen staging tickets survive reuse and destruction of the
//! target, agree with synchronous readback, and support concurrent cached reads.
use std::sync::Arc;
use usum_render::{RasterState, RasterVertex, Renderer, Viewport};

fn main() -> Result<(), String> {
    let renderer = Renderer::new()?;
    let target = renderer.create_color_target(31, 19)?; // exercises padded rows
    let triangle = |color| {
        [
            RasterVertex::new([-0.8, -0.8, -0.5, 1.0], color),
            RasterVertex::new([0.8, -0.8, -0.5, 1.0], color),
            RasterVertex::new([0.0, 0.8, -0.5, 1.0], color),
        ]
    };
    let mut batch = renderer.begin_batch(&target);
    batch.draw(&triangle([1.0, 0.0, 0.0, 1.0]), Viewport::full(&target), RasterState::default())?;
    let first = Arc::new(batch.submit_readback()?);
    let first_reference = renderer.read_color_and_depth_target(&target)?;
    renderer.upload_target(&target, &vec![0; 31 * 19 * 4], &vec![1.0; 31 * 19])?;
    let mut batch = renderer.begin_batch(&target);
    batch.draw(&triangle([0.0, 1.0, 0.0, 1.0]), Viewport::full(&target), RasterState::default())?;
    let second = batch.submit_readback()?;
    let second_reference = renderer.read_color_and_depth_target(&target)?;
    drop(target);
    // Materialize out of submission order and only after the target is gone.
    let second = second.read()?;
    assert_eq!(second.color, second_reference.0);
    assert_eq!(second.depth, second_reference.1);
    std::thread::scope(|s| {
        let mut jobs = Vec::new();
        for _ in 0..4 {
            jobs.push(s.spawn(|| {
                let read = first.read().unwrap();
                assert_eq!(read.color, first_reference.0);
                assert_eq!(read.depth, first_reference.1);
            }));
        }
        for job in jobs {
            job.join().unwrap();
        }
    });
    assert_ne!(first.read()?.color, second.color);
    println!("adapter {}", renderer.adapter_info().name);
    println!("frozen color/depth snapshots match synchronous readback; target reuse, padded rows, destruction and concurrent reads pass");
    Ok(())
}
