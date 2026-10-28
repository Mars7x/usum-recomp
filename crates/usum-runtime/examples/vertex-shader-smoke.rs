//! Original synthetic PICA programs; never uses a game dump.
use usum_render::{FragmentInputs, PicaVertexDraw, RasterState, RasterVertex, Renderer, VertexProgram, Viewport};
use usum_runtime::pica::shader::{self, Unit};

fn unit(code: &[u32]) -> Unit {
    let mut unit = Unit::default();
    unit.write(0x25, 0);
    unit.write(0x26, 15 | 0x1B << 5 | 0x1B << 14 | 0x1B << 23);
    unit.write(0x1B, 0);
    for &word in code {
        unit.write(0x1C, word);
    }
    unit
}
fn uniforms(unit: &Unit) -> Vec<u32> {
    let mut words: Vec<u32> = unit.floats.iter().flatten().map(|f| f.to_bits()).collect();
    words.extend(unit.ints.map(u32::from_le_bytes));
    words.extend([u32::from(unit.bools), 3, 2, 0, 0x03020100, 0x0b0a0908, 0, 0, 0, 0, 0]);
    words
}
fn compare(renderer: &Renderer, label: &str, unit: &Unit, inputs: &[[[f32; 4]; 16]]) -> Result<(), String> {
    let program = VertexProgram::compile(&unit.code, &unit.descs, unit.entry)?;
    let actual = renderer.evaluate_vertex_program(&program, &uniforms(unit), inputs)?;
    for (index, (actual, input)) in actual.iter().zip(inputs).enumerate() {
        let expected = shader::run(unit, input)?;
        for (component, (a, e)) in actual.iter().flatten().zip(expected.iter().flatten()).enumerate() {
            let equal = if e.is_nan() {
                a.is_nan()
            } else if e.is_infinite() {
                a.to_bits() == e.to_bits()
            } else {
                (*a - *e).abs() <= 1e-5 * e.abs().max(1.0)
            };
            if !equal {
                return Err(format!("{label}: vertex {index} component {component}: GPU {a} CPU {e}"));
            }
        }
    }
    println!("{label}: {} vertices match CPU", inputs.len());
    Ok(())
}
fn main() -> Result<(), String> {
    let renderer = Renderer::new()?;
    println!("{}", renderer.adapter_info().name);
    let mut inputs = vec![[[0.0; 4]; 16]; 129];
    for (i, input) in inputs.iter_mut().enumerate() {
        input[0] = [1.0 + i as f32 * 0.01, 2.0, 3.0, 1.0];
        input[1] = [2.0, 0.5, 1.0, 1.0];
    }
    let mut matrix = unit(&[0x02 << 26 | 0x20 << 12, 0x22 << 26]);
    matrix.floats[0] = [1.0, 2.0, 3.0, 4.0];
    compare(&renderer, "uniform DP4", &matrix, &inputs)?;
    let mut relative = unit(&[
        0x12 << 26 | 1 << 12,
        0x13 << 26 | 0x10 << 21 | 0x20 << 12 | 1 << 19,
        0x38 << 26 | 1 << 24 | 0x10 << 10,
        0x22 << 26,
    ]);
    relative.floats[2] = [2.0; 4];
    compare(&renderer, "MOVA, relative MOV and MAD", &relative, &inputs)?;
    let mut looping = unit(&[0x29 << 26 | 1 << 10, 0x10 << 21 | 0x10 << 12, 0x13 << 26 | 0x10 << 12, 0x22 << 26]);
    looping.ints[0] = [3, 0, 1, 0];
    compare(&renderer, "LOOP", &looping, &inputs)?;
    let mut branch = unit(&[0x27 << 26 | 2 << 10 | 1, 0x13 << 26, 0x13 << 26 | 1 << 12, 0x22 << 26]);
    compare(&renderer, "IFU false", &branch, &inputs)?;
    branch.bools = 1;
    compare(&renderer, "IFU true", &branch, &inputs)?;
    let call = unit(&[0x24 << 26 | 2 << 10 | 1, 0x22 << 26, 0x13 << 26]);
    compare(&renderer, "CALL", &call, &inputs)?;
    let mut breaking = unit(&[0x29 << 26 | 2 << 10, 0x13 << 26, 0x20 << 26, 0x22 << 26]);
    breaking.ints[0] = [200, 0, 1, 0];
    compare(&renderer, "BREAK", &breaking, &inputs)?;
    for opcode in
        [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x13, 0x18, 0x19, 0x1a, 0x1b]
    {
        let program = unit(&[opcode << 26 | 1 << 7, 0x22 << 26]);
        compare(&renderer, &format!("arithmetic {opcode:#x}"), &program, &inputs)?;
    }
    let program = unit(&[0x13 << 26, 0x13 << 26 | 1 << 21 | 1 << 12, 0x22 << 26]);
    let compiled = VertexProgram::compile(&program.code, &program.descs, 0)?;
    let vertices = [
        RasterVertex::new([-0.75, -0.75, -0.25, 1.0], [1.0, 0.0, 0.0, 1.0]),
        RasterVertex::new([0.75, -0.75, -0.25, 1.0], [0.0, 1.0, 0.0, 1.0]),
        RasterVertex::new([0.0, 0.75, -0.25, 1.0], [0.0, 0.0, 1.0, 1.0]),
    ];
    let raw: Vec<_> = vertices
        .iter()
        .map(|vertex| {
            let mut raw = [[0.0; 4]; 16];
            raw[0] = vertex.position;
            raw[1] = vertex.color;
            raw
        })
        .collect();
    let expected = renderer.render_vertices(&vertices, 32, 32)?;
    let target = renderer.create_color_target(32, 32)?;
    let mut batch = renderer.begin_batch(&target);
    batch.draw_pica_vertices(
        PicaVertexDraw { program: &compiled, inputs: &raw, indices: &[0, 1, 2], uniforms: &uniforms(&program) },
        Viewport::full(&target),
        RasterState::default(),
        FragmentInputs::default(),
    )?;
    let (actual, _) = batch.finish_and_read_color_and_depth()?;
    if actual != expected {
        return Err("indexed hardware vertex raster differs from CPU-fed raster".into());
    }
    println!("indexed hardware vertex raster: byte-identical");
    Ok(())
}
