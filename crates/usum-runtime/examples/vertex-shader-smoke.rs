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
    for comparison in 0..6 {
        let cmp = ((0x2Eu32 << 26) & !(7 << 24)) | comparison << 24 | 1 << 7;
        let conditional =
            unit(&[cmp, 0x28 << 26 | 1 << 25 | 2 << 22 | 3 << 10 | 1, 0x13 << 26, 0x13 << 26 | 1 << 12, 0x22 << 26]);
        compare(&renderer, &format!("CMP {comparison} / IFC"), &conditional, &inputs)?;
    }
    let cmp = 0x2E << 26 | 1 << 7;
    let conditional_call =
        unit(&[cmp, 0x25 << 26 | 1 << 25 | 2 << 22 | 4 << 10 | 1, 0x13 << 26 | 2 << 12, 0x22 << 26, 0x13 << 26 | 1 << 21]);
    compare(&renderer, "conditional CALL", &conditional_call, &inputs)?;
    let jump = unit(&[cmp, 0x2C << 26 | 1 << 25 | 2 << 22 | 3 << 10, 0x13 << 26, 0x22 << 26]);
    compare(&renderer, "forward conditional jump", &jump, &inputs)?;
    let mut nested = unit(&[
        0x29 << 26 | 3 << 10,
        0x29 << 26 | 1 << 22 | 2 << 10,
        0x10 << 21 | 0x20 << 12 | 0x10 << 7 | 3 << 19,
        0x21 << 26,
        0x13 << 26 | 0x10 << 12,
        0x22 << 26,
    ]);
    nested.ints[0] = [2, 0, 1, 0];
    nested.ints[1] = [1, 2, 255, 0];
    for (i, v) in nested.floats.iter_mut().enumerate() {
        *v = [i as f32; 4];
    }
    compare(&renderer, "nested LOOP and signed address steps", &nested, &inputs)?;
    let mut madi = unit(&[0x12 << 26 | 1 << 12, 0x30 << 26 | 1 << 22 | 1 << 12 | 0x20 << 5, 0x22 << 26]);
    madi.floats[2] = [2.0; 4];
    compare(&renderer, "MADI relative third operand", &madi, &inputs)?;
    for opcode in [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x13, 0x18, 0x19, 0x1a,
        0x1b,
    ] {
        let program = unit(&[opcode << 26 | 1 << 7, 0x22 << 26]);
        compare(&renderer, &format!("arithmetic {opcode:#x}"), &program, &inputs)?;
    }
    let mut special = [[[0.0; 4]; 16]; 2];
    special[0][0] = [0.0, -0.0, f32::INFINITY, f32::NAN];
    special[0][1] = [f32::INFINITY, f32::INFINITY, 0.0, 0.0];
    special[1][0] = [-0.25, 0.0, 1.0, f32::NAN];
    special[1][1] = [2.0, -0.0, f32::NAN, 1.0];
    for opcode in [0x08, 0x0c, 0x0d, 0x0e, 0x0f] {
        compare(&renderer, &format!("special floats {opcode:#x}"), &unit(&[opcode << 26 | 1 << 7, 0x22 << 26]), &special)?;
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
    let mapping_program = unit(&[
        0x13 << 26 | 3 << 21,
        0x13 << 26 | 7 << 21 | 1 << 12,
        0x13 << 26 | 12 << 21 | 2 << 12,
        0x13 << 26 | 14 << 21 | 3 << 12,
        0x22 << 26,
    ]);
    let mapped = VertexProgram::compile(&mapping_program.code, &mapping_program.descs, 0)?;
    let texture = renderer.create_rgba8_texture(
        2,
        2,
        &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255],
        usum_render::SamplerState::default(),
    )?;
    let mut state = RasterState::default();
    state.tev.stages[0] = usum_render::TevStage::replace(5);
    let mut words = uniforms(&mapping_program);
    words[389] = (1 << 3) | (1 << 7) | (1 << 12) | (1 << 14);
    words[390] = 4;
    words[392] = 0x03020100;
    words[393] = 0x0b0a0908;
    words[394] = 0x0f0e0d0c;
    words[395] = 0x1f1f1716;
    let mut raw = raw;
    for input in &mut raw {
        input[2] = [0.25, 0.25, 0.75, 0.75];
        input[3] = [0.25, 0.25, 0.0, 0.0];
    }
    for route in [0, 1] {
        words[391] = route;
        let expected_vertices =
            vertices.map(|v| v.with_texcoord1([0.75, 0.75]).with_texcoord2(if route == 0 { [0.25, 0.25] } else { [0.75, 0.75] }));
        let expected_target = renderer.create_color_target(32, 32)?;
        renderer.draw_vertices_with_inputs(
            &expected_target,
            &expected_vertices,
            Viewport::full(&expected_target),
            state,
            FragmentInputs { texture2: Some(&texture), ..FragmentInputs::default() },
        )?;
        let target = renderer.create_color_target(32, 32)?;
        let mut batch = renderer.begin_batch(&target);
        batch.draw_pica_vertices(
            PicaVertexDraw { program: &mapped, inputs: &raw, indices: &[0, 1, 2], uniforms: &words },
            Viewport::full(&target),
            state,
            FragmentInputs { texture2: Some(&texture), ..FragmentInputs::default() },
        )?;
        let (actual, _) = batch.finish_and_read_color_and_depth()?;
        if actual != renderer.read_color_target(&expected_target)? {
            return Err(format!("sparse output map / UV route {route} differs"));
        }
    }
    println!("sparse output maps and both UV routes: byte-identical");
    Ok(())
}
