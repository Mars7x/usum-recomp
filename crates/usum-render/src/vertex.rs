//! PICA vertex-program translation. Generated WGSL stays in memory: programs
//! supplied by a game are private data, not project source.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Raw input registers and triangle indices for a hardware vertex-stage draw.
/// Uniform words: float registers 0..384, packed integer registers 384..388,
/// bools/mask/map-count/UV-route 388..392, seven output-map words 392..399.
#[derive(Clone, Copy)]
pub struct PicaVertexDraw<'a> {
    pub program: &'a VertexProgram,
    pub inputs: &'a [[[f32; 4]; 16]],
    pub indices: &'a [u32],
    pub uniforms: &'a [u32],
}

impl PicaVertexDraw<'_> {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.uniforms.len() < 399 || self.inputs.is_empty() || self.indices.is_empty() || !self.indices.len().is_multiple_of(3)
        {
            return Err("invalid hardware vertex draw dimensions".into());
        }
        if self.indices.iter().any(|&i| i as usize >= self.inputs.len()) {
            return Err("hardware vertex index is out of range".into());
        }
        self.program.validate_uniforms(self.uniforms[384..388].try_into().expect("integer uniforms"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    code: Vec<u32>,
    descriptors: Vec<u32>,
    entry: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cost {
    Fixed(u64),
    Sum(Vec<Cost>),
    Loop(usize, Box<Cost>),
}

impl Cost {
    fn bound(&self, integers: &[u32; 4]) -> u64 {
        match self {
            Self::Fixed(n) => *n,
            Self::Sum(parts) => parts.iter().map(|part| part.bound(integers)).fold(0, u64::saturating_add),
            Self::Loop(slot, body) => body.bound(integers).saturating_mul(u64::from((integers[*slot] & 255) + 1)),
        }
    }
}

/// An immutable translated program, independent of per-draw uniforms.
#[derive(Debug, PartialEq, Eq)]
pub struct VertexProgram {
    pub(crate) id: u64,
    pub(crate) source: String,
    cost: Cost,
}

impl VertexProgram {
    pub fn compile(code: &[u32], descriptors: &[u32], entry: u32) -> Result<Arc<Self>, String> {
        type Cache = HashMap<Key, Result<Arc<VertexProgram>, String>>;
        static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        if code.len() > 4096 || descriptors.len() != 128 || entry as usize >= code.len() {
            return Err("invalid PICA vertex program memory".into());
        }
        let key = Key { code: code.to_vec(), descriptors: descriptors.to_vec(), entry };
        let mut cache = CACHE.get_or_init(Mutex::default).lock().map_err(|_| "vertex compiler cache poisoned")?;
        if let Some(program) = cache.get(&key) {
            return program.clone();
        }
        let result = (|| {
            let mut compiler = Compiler { code, descriptors, emitted: 0, sequence: 0, active: Vec::new() };
            let (body, cost, terminated) = compiler.block(entry as usize, code.len(), 0)?;
            if !terminated {
                return Err("vertex program can fall out of code memory".into());
            }
            let source = format!("{HELPERS}\nfn pica_run(index: u32) -> array<vec4<f32>,16> {{\nvar s: PicaState;\nfor(var k=0u;k<16u;k++){{s.v[k]=vs_inputs.values[index*16u+k];}}\n{body}\n}}\n");
            Ok(Arc::new(Self { id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), source, cost }))
        })();
        cache.insert(key, result.clone());
        result
    }

    /// Conservative bound, including both sides of conditional branches.
    /// Reject pathological uniform-controlled loops before submitting a draw.
    pub fn validate_uniforms(&self, integers: &[u32; 4]) -> Result<(), String> {
        if self.cost.bound(integers) > 65_536 {
            Err("vertex program exceeds GPU instruction budget".into())
        } else {
            Ok(())
        }
    }
}

struct Compiler<'a> {
    code: &'a [u32],
    descriptors: &'a [u32],
    emitted: usize,
    sequence: usize,
    active: Vec<(usize, usize)>,
}

impl Compiler<'_> {
    fn block(&mut self, start: usize, end: usize, loop_depth: usize) -> Result<(String, Cost, bool), String> {
        if start > end || end > self.code.len() || self.active.len() >= 32 || self.active.contains(&(start, end)) {
            return Err("unstructured or recursive vertex control flow".into());
        }
        self.active.push((start, end));
        let result = self.block_inner(start, end, loop_depth);
        self.active.pop();
        result
    }

    fn block_inner(&mut self, mut pc: usize, end: usize, loop_depth: usize) -> Result<(String, Cost, bool), String> {
        let mut text = String::new();
        let mut costs = Vec::new();
        while pc < end {
            self.emitted += 1;
            if self.emitted > 8192 {
                return Err("expanded vertex program is too large".into());
            }
            let word = self.code[pc];
            let op = word >> 26;
            let target = ((word >> 10) & 0xFFF) as usize;
            let num = (word & 255) as usize;
            costs.push(Cost::Fixed(1));
            match op {
                0x00..=0x0F | 0x12 | 0x13 | 0x18..=0x1B | 0x2E..=0x3F => {
                    text.push_str(&self.arithmetic(word)?);
                }
                0x21 => {}
                0x22 => {
                    text.push_str("return s.o;\n");
                    return Ok((text, Cost::Sum(costs), true));
                }
                0x20 | 0x23 => {
                    if loop_depth == 0 {
                        return Err("vertex BREAK outside a structured loop".into());
                    }
                    if op == 0x20 {
                        text.push_str("break;\n");
                        return Ok((text, Cost::Sum(costs), true));
                    }
                    text.push_str(&format!("if({}){{break;}}\n", condition(word)));
                }
                0x24..=0x26 => {
                    let (body, cost, terminated) =
                        self.block(target, target.checked_add(num).ok_or("vertex call overflow")?, loop_depth)?;
                    costs.push(cost);
                    if op == 0x24 {
                        text.push_str(&format!("{{\n{body}}}\n"));
                        if terminated {
                            return Ok((text, Cost::Sum(costs), true));
                        }
                    } else {
                        let yes = if op == 0x25 { condition(word) } else { uniform_bool(word) };
                        text.push_str(&format!("if({yes}){{\n{body}}}\n"));
                    }
                }
                0x27 | 0x28 => {
                    let after = target.checked_add(num).ok_or("vertex IF overflow")?;
                    if target < pc + 1 || after > end {
                        return Err("vertex IF crosses its containing block".into());
                    }
                    let (yes_body, yes_cost, yes_end) = self.block(pc + 1, target, loop_depth)?;
                    let (no_body, no_cost, no_end) = self.block(target, after, loop_depth)?;
                    let yes = if op == 0x27 { uniform_bool(word) } else { condition(word) };
                    text.push_str(&format!("if({yes}){{\n{yes_body}}}else{{\n{no_body}}}\n"));
                    costs.extend([yes_cost, no_cost]);
                    pc = after;
                    if yes_end && no_end {
                        return Ok((text, Cost::Sum(costs), true));
                    }
                    continue;
                }
                0x29 => {
                    if target < pc || target >= end {
                        return Err("vertex LOOP crosses its containing block".into());
                    }
                    let (body, cost, terminated) = self.block(pc + 1, target + 1, loop_depth + 1)?;
                    let slot = ((word >> 22) & 3) as usize;
                    self.sequence += 1;
                    let n = self.sequence;
                    let advance = if terminated {
                        ""
                    } else {
                        "s.al=bitcast<i32>(bitcast<u32>(s.al)+bitcast<u32>((i32((integer>>16u)&255u)<<24u)>>24u));"
                    };
                    text.push_str(&format!("{{let integer=vs_params.words[{}u];s.al=i32((integer>>8u)&255u);\nfor(var iteration{n}=0u;iteration{n}<= (integer&255u);iteration{n}++){{\n{body}\n{advance}\n}}}}\n",384+slot));
                    costs.push(Cost::Loop(slot, Box::new(cost)));
                    pc = target + 1;
                    continue;
                }
                0x2C | 0x2D => {
                    if target <= pc || target > end {
                        return Err("backward or crossing vertex jump".into());
                    }
                    let (body, cost, _) = self.block(pc + 1, target, loop_depth)?;
                    let yes = if op == 0x2C {
                        condition(word)
                    } else if num & 1 == 0 {
                        uniform_bool(word)
                    } else {
                        format!("!({})", uniform_bool(word))
                    };
                    text.push_str(&format!("if(!({yes})){{\n{body}}}\n"));
                    costs.push(cost);
                    pc = target;
                    continue;
                }
                _ => return Err(format!("unsupported GPU vertex opcode {op:#x} at {pc:#x}")),
            }
            pc += 1;
        }
        Ok((text, Cost::Sum(costs), false))
    }

    fn arithmetic(&self, word: u32) -> Result<String, String> {
        let op = word >> 26;
        let descriptor = self.descriptors[(word & if op >= 0x30 { 31 } else { 127 }) as usize];
        let mut text = String::from("{\n");
        let mut destination = (word >> 21) & 31;
        let value = if op >= 0x30 {
            let inverted = op < 0x38;
            let (a, b, c, idx) = if inverted {
                ((word >> 17) & 31, (word >> 12) & 31, (word >> 5) & 127, (word >> 22) & 3)
            } else {
                ((word >> 17) & 31, (word >> 10) & 127, (word >> 5) & 31, (word >> 22) & 3)
            };
            let s1 = source(a, "0", descriptor, 0);
            let offset = address(idx);
            let s2 = source(b, if inverted { "0" } else { &offset }, descriptor, 1);
            let s3 = source(c, if inverted { &offset } else { "0" }, descriptor, 2);
            text.push_str(&format!("let a={s1};let b={s2};let c={s3};\n"));
            destination = (word >> 24) & 31;
            "pica_mul4(a,b)+c".to_string()
        } else {
            let inverted = (0x18..=0x1B).contains(&op);
            let (a, b, idx) = if inverted {
                ((word >> 14) & 31, (word >> 7) & 127, (word >> 19) & 3)
            } else {
                ((word >> 12) & 127, (word >> 7) & 31, (word >> 19) & 3)
            };
            let offset = address(idx);
            let a = source(a, if inverted { "0" } else { &offset }, descriptor, 0);
            let b = source(b, if inverted { &offset } else { "0" }, descriptor, 1);
            text.push_str(&format!("let a={a};let b={b};\n"));
            match op {
                0x00 => "a+b".into(),
                0x01 => "vec4<f32>(pica_dot3(a.xyz,b.xyz))".into(),
                0x02 => "vec4<f32>(pica_dot4(a,b))".into(),
                0x03 | 0x18 => "vec4<f32>(pica_dot4(vec4<f32>(a.xyz,1.0),b))".into(),
                0x04 | 0x19 => "vec4<f32>(1.0,pica_mul(a.y,b.y),a.z,b.w)".into(),
                0x05 => "vec4<f32>(exp2(a.x))".into(),
                0x06 => "vec4<f32>(log2(a.x))".into(),
                0x07 => {
                    text.push_str("s.cmp=vec2<bool>(a.x>=0.0,a.w>=0.0);\n");
                    "vec4<f32>(pica_max(a.x,0.0),clamp(a.y,-127.9961,127.9961),0.0,pica_max(a.w,0.0))".into()
                }
                0x08 => "pica_mul4(a,b)".into(),
                0x09 | 0x1A => "select(vec4<f32>(0.0),vec4<f32>(1.0),a>=b)".into(),
                0x0A | 0x1B => "select(vec4<f32>(0.0),vec4<f32>(1.0),a<b)".into(),
                0x0B => "floor(a)".into(),
                0x0C => "select(b,a,a>b)".into(),
                0x0D => "select(b,a,a<b)".into(),
                0x0E => "vec4<f32>(pica_rcp(a.x))".into(),
                0x0F => "vec4<f32>(pica_rsq(a.x))".into(),
                0x12 => {
                    if descriptor & 8 != 0 {
                        text.push_str("s.a0.x=pica_i32(a.x);\n");
                    }
                    if descriptor & 4 != 0 {
                        text.push_str("s.a0.y=pica_i32(a.y);\n");
                    }
                    text.push_str("}\n");
                    return Ok(text);
                }
                0x13 => "a".into(),
                0x2E | 0x2F => {
                    text.push_str(&format!(
                        "s.cmp=vec2<bool>({},{});\n}}\n",
                        comparison("a.x", "b.x", (word >> 24) & 7),
                        comparison("a.y", "b.y", (word >> 21) & 7)
                    ));
                    return Ok(text);
                }
                _ => return Err(format!("invalid vertex arithmetic opcode {op:#x}")),
            }
        };
        text.push_str(&format!("let value={value};\n"));
        let register = if destination < 16 { format!("s.o[{destination}]") } else { format!("s.r[{}]", destination - 16) };
        for (k, c) in ['x', 'y', 'z', 'w'].into_iter().enumerate() {
            if descriptor & (8 >> k) != 0 {
                text.push_str(&format!("{register}.{c}=value.{c};\n"));
            }
        }
        text.push_str("}\n");
        Ok(text)
    }
}

fn address(index: u32) -> String {
    match index {
        1 => "s.a0.x",
        2 => "s.a0.y",
        3 => "s.al",
        _ => "0",
    }
    .into()
}
fn uniform_bool(word: u32) -> String {
    format!("((vs_params.words[388u]>>{}u)&1u)!=0u", (word >> 22) & 15)
}
fn condition(word: u32) -> String {
    let x = format!("(s.cmp.x=={})", word >> 25 & 1 != 0);
    let y = format!("(s.cmp.y=={})", word >> 24 & 1 != 0);
    match word >> 22 & 3 {
        0 => format!("({x}||{y})"),
        1 => format!("({x}&&{y})"),
        2 => x,
        _ => y,
    }
}
fn comparison(a: &str, b: &str, op: u32) -> String {
    match op {
        0 => format!("{a}=={b}"),
        1 => format!("{a}!={b}"),
        2 => format!("{a}<{b}"),
        3 => format!("{a}<={b}"),
        4 => format!("{a}>{b}"),
        5 => format!("{a}>={b}"),
        _ => "true".into(),
    }
}
fn source(register: u32, offset: &str, descriptor: u32, operand: u32) -> String {
    let raw = if register < 16 {
        format!("s.v[{register}]")
    } else if register < 32 {
        format!("s.r[{}]", register - 16)
    } else {
        format!("pica_uniform({}u,{offset})", register - 32)
    };
    let (shift, sign) = match operand {
        0 => (5, 4),
        1 => (14, 13),
        _ => (23, 22),
    };
    let selector = descriptor >> shift & 255;
    let swizzle: String = (0..4).map(|k| ['x', 'y', 'z', 'w'][((selector >> (6 - 2 * k)) & 3) as usize]).collect();
    format!("{}({raw}).{swizzle}", if descriptor >> sign & 1 != 0 { "-" } else { "" })
}

pub(crate) const BINDINGS: &str = r#"
@group(1) @binding(0) var<storage,read> vs_inputs: PicaInputs;
@group(1) @binding(1) var<storage,read> vs_params: PicaWords;
"#;

const HELPERS: &str = r#"
struct PicaInputs { values:array<vec4<f32>>, }
struct PicaWords { words:array<u32>, }
struct PicaState { v:array<vec4<f32>,16>, r:array<vec4<f32>,16>, o:array<vec4<f32>,16>, a0:vec2<i32>, al:i32, cmp:vec2<bool>, }
fn pica_uniform(index:u32,relative:i32)->vec4<f32>{
 var offset=relative;if(offset < -128 || offset >127){offset=0;}
 let i=u32((i32(index)+offset)&127);if(i>=96u){return vec4<f32>(1.0);}
 let at=i*4u;return bitcast<vec4<f32>>(vec4<u32>(vs_params.words[at],vs_params.words[at+1u],vs_params.words[at+2u],vs_params.words[at+3u]));
}
fn pica_mul(a:f32,b:f32)->f32{if((a==0.0 && b==b)||(b==0.0 && a==a)){return 0.0;}return a*b;}
fn pica_mul4(a:vec4<f32>,b:vec4<f32>)->vec4<f32>{return vec4<f32>(pica_mul(a.x,b.x),pica_mul(a.y,b.y),pica_mul(a.z,b.z),pica_mul(a.w,b.w));}
fn pica_dot3(a:vec3<f32>,b:vec3<f32>)->f32{var v=0.0;v=v+pica_mul(a.x,b.x);v=v+pica_mul(a.y,b.y);v=v+pica_mul(a.z,b.z);return v;}
fn pica_dot4(a:vec4<f32>,b:vec4<f32>)->f32{var v=pica_dot3(a.xyz,b.xyz);v=v+pica_mul(a.w,b.w);return v;}
fn pica_max(a:f32,b:f32)->f32{return select(b,a,a>b);}
fn pica_rcp(a:f32)->f32{if(a==0.0){return bitcast<f32>(0x7f800000u);}return 1.0/a;}
fn pica_rsq(a:f32)->f32{if(a==0.0){return bitcast<f32>(0x7f800000u);}return 1.0/sqrt(a);}
fn pica_i32(a:f32)->i32{if(a!=a){return 0;}if(a>=2147483648.0){return 2147483647;}if(a<= -2147483648.0){return (-2147483647-1);}return i32(a);}
"#;

pub(crate) const VERTEX_ENTRY: &str = r#"
@vertex fn pica_vertex(@builtin(vertex_index) index:u32)->VertexOut {
 let registers=pica_run(index);var out:VertexOut;out.color=vec4<f32>(0.0,0.0,0.0,1.0);
 var slot=0u;let total=min(vs_params.words[390u],7u);
 for(var r=0u;r<16u;r++){
  if((vs_params.words[389u]&(1u<<r))==0u){continue;}if(slot>=total){break;}
  let mapping=vs_params.words[392u+slot];slot++;
  for(var c=0u;c<4u;c++){
   let semantic=(mapping>>(c*8u))&31u;let v=registers[r][c];
   if(semantic<4u){out.position[semantic]=v;}
   else if(semantic<8u){out.quaternion[semantic-4u]=v;}
   else if(semantic<12u){out.color[semantic-8u]=v;}
   else if(semantic==12u){out.texcoord0.x=v;}else if(semantic==13u){out.texcoord0.y=v;}
   else if(semantic==14u){out.texcoord1.x=v;}else if(semantic==15u){out.texcoord1.y=v;}
   else if(semantic>=18u && semantic<=20u){out.view[semantic-18u]=v;}
   else if(semantic==22u){out.texcoord2.x=v;}else if(semantic==23u){out.texcoord2.y=v;}
  }
 }
 out.position.z= -out.position.z;
 if(vs_params.words[391u]!=0u){out.texcoord2=out.texcoord1;}
 return out;
}
"#;

impl crate::Renderer {
    /// Diagnostic execution of the same translated vertex function in a
    /// compute harness. Gameplay uses the graphics vertex stage directly.
    pub fn evaluate_vertex_program(
        &self,
        program: &VertexProgram,
        uniforms: &[u32],
        inputs: &[[[f32; 4]; 16]],
    ) -> Result<Vec<[[f32; 4]; 16]>, String> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        if uniforms.len() < 399 {
            return Err("missing vertex uniforms".into());
        }
        program.validate_uniforms(uniforms[384..388].try_into().expect("integer uniforms"))?;
        let mut input_bytes = Vec::with_capacity(inputs.len() * 256);
        for input in inputs {
            for register in input {
                for value in register {
                    input_bytes.extend_from_slice(&value.to_ne_bytes());
                }
            }
        }
        let mut uniform_bytes = Vec::with_capacity(uniforms.len() * 4);
        for word in uniforms {
            uniform_bytes.extend_from_slice(&word.to_ne_bytes());
        }
        let errors = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let upload = |label, bytes: &[u8]| -> Result<wgpu::Buffer, String> {
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: true,
            });
            buffer.get_mapped_range_mut(..).map_err(|e| e.to_string())?.copy_from_slice(bytes);
            buffer.unmap();
            Ok(buffer)
        };
        let input = upload("vertex comparison inputs", &input_bytes)?;
        let uniform = upload("vertex comparison uniforms", &uniform_bytes)?;
        let size = input_bytes.len() as u64;
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertex comparison output"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertex comparison readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vertex comparison layout"),
            entries: &[0, 1, 2].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: binding < 2 },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vertex comparison program"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{}\n{COMPUTE_ENTRY}", program.source, BINDINGS.replace("@group(1)", "@group(0)")).into(),
            ),
        });
        let pipeline_layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vertex comparison pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("vertex comparison pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("evaluate"),
            compilation_options: Default::default(),
            cache: None,
        });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vertex comparison bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: input.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: output.as_entire_binding() },
            ],
        });
        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("vertex comparison commands") });
        {
            let mut pass = encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("vertex comparison"), timestamp_writes: None });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((inputs.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        let submitted = self.queue.submit([encoder.finish()]);
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
        self.device.poll(wgpu::PollType::Wait { submission_index: Some(submitted), timeout: None }).map_err(|e| e.to_string())?;
        if let Some(error) = pollster::block_on(errors.pop()) {
            return Err(format!("vertex comparison GPU validation: {error}"));
        }
        receive.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let mapped = readback.get_mapped_range(..).map_err(|e| e.to_string())?;
        let result = mapped
            .as_chunks::<256>()
            .0
            .iter()
            .map(|vertex| {
                let mut registers = [[0.0; 4]; 16];
                for (out, bytes) in registers.iter_mut().flatten().zip(vertex.as_chunks::<4>().0) {
                    *out = f32::from_ne_bytes(*bytes);
                }
                registers
            })
            .collect();
        drop(mapped);
        readback.unmap();
        Ok(result)
    }
}

const COMPUTE_ENTRY: &str = r#"
@group(0) @binding(2) var<storage,read_write> vs_outputs:PicaInputs;
@compute @workgroup_size(64) fn evaluate(@builtin(global_invocation_id) id:vec3<u32>){
 let index=id.x;if(index>=arrayLength(&vs_inputs.values)/16u){return;}
 let outputs=pica_run(index);for(var k=0u;k<16u;k++){vs_outputs.values[index*16u+k]=outputs[k];}
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    fn descriptors() -> Vec<u32> {
        vec![15 | 0x1B << 5 | 0x1B << 14 | 0x1B << 23; 128]
    }
    #[test]
    fn rejects_recursive_and_unbounded_programs() {
        assert!(VertexProgram::compile(&[0x24 << 26 | 1, 0x22 << 26], &descriptors(), 0).is_err());
        assert!(VertexProgram::compile(&[0x21 << 26], &descriptors(), 0).is_err());
        assert!(VertexProgram::compile(&[0x2A << 26, 0x22 << 26], &descriptors(), 0).is_err());
    }
    #[test]
    fn structured_loop_budget_and_program_cache() {
        let code = [0x29 << 26 | 3 << 10, 0x29 << 26 | 2 << 10, 0x13 << 26, 0x21 << 26, 0x22 << 26];
        let a = VertexProgram::compile(&code, &descriptors(), 0).unwrap();
        let b = VertexProgram::compile(&code, &descriptors(), 0).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.validate_uniforms(&[0; 4]).is_ok());
        assert!(a.validate_uniforms(&[255; 4]).is_err());
    }
}
