fn main() -> Result<(), String> {
    let renderer = usum_render::Renderer::new()?;
    let info = renderer.adapter_info();
    println!("adapter  {}", info.name);
    println!("backend  {:?}", info.backend);
    println!("device   {:?}", info.device_type);
    println!("driver   {} {}", info.driver, info.driver_info);
    renderer.smoke_triangle()?;
    println!(
        "draw     batching, snapshot upload, persistence, viewport, culling, rasterization, interpolation, depth, color masks, alpha test, blending, 3 textures, TEV, lighting LUTs and readback validated"
    );
    Ok(())
}
