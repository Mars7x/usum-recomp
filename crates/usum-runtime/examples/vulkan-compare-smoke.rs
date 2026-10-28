use usum_render::Renderer;
use usum_runtime::pica::draw::Vertex;
use usum_runtime::pica::vulkan_compare::{CaptureTarget, DrawSnapshot, TargetSnapshot};
use usum_runtime::pica::Pica;

fn main() -> Result<(), String> {
    let mut pica = Pica::default();
    pica.regs[0x41] = 0x430000; // float24 16.0: half of a 32-pixel target
    pica.regs[0x43] = 0x430000;
    pica.regs[0x107] = 1 | 5 << 4 | 0xF << 8 | 1 << 12;
    pica.regs[0x113] = 1;
    pica.regs[0x115] = 2;
    let shaded = [
        Vertex { position: [-0.75, -0.75, -0.5, 1.0], color: [1.0, 0.0, 0.0, 1.0], ..Vertex::default() },
        Vertex { position: [0.75, -0.75, -0.5, 1.0], color: [0.0, 1.0, 0.0, 1.0], ..Vertex::default() },
        Vertex { position: [0.0, 0.75, -0.5, 1.0], color: [0.0, 0.0, 1.0, 1.0], ..Vertex::default() },
    ];
    let target = TargetSnapshot { width: 32, height: 32, color: vec![0; 32 * 32 * 4], depth: vec![1.0; 32 * 32] };
    let target_info =
        CaptureTarget { color_address: 0x2000_0000, depth_address: 0, width: 32, height: 32, color_format: 0, depth_format: 0 };
    let snapshot = DrawSnapshot::from_shaded_vertices(&pica, &shaded, &[[0, 1, 2]], target_info, target, [None, None, None])?;
    let renderer = Renderer::new()?;
    let image = snapshot.render(&renderer)?;
    let pixel = &image[(16 * 32 + 16) * 4..][..4];
    if pixel[3] != 255 || pixel[..3].iter().all(|&channel| channel == 0) {
        return Err(format!("runtime Vulkan handoff returned centre pixel {pixel:?}"));
    }
    println!("adapter  {}", renderer.adapter_info().name);
    println!("handoff  immutable runtime draw snapshot rendered centre pixel {pixel:?}");
    Ok(())
}
