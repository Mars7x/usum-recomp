//! Builds the differential test corpus.
//!
//! For each `programs/*.s` (and each generated random program), this:
//! 1. assembles and links it with the harness, using GNU binutils for ARM;
//! 2. runs the ELF under `qemu-arm -cpu arm11mpcore`, the independent
//!    reference, and keeps its stdout as the expected output;
//! 3. translates the same flat binary with `usum-recomp` into Rust, which
//!    the test crate compiles and runs (src/lib.rs).
//!
//! The tools are external test dependencies only (docs/DEPENDENCIES.md).
//! If any is missing the build fails with instructions; it never skips.

#[path = "build/random.rs"]
mod random;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BASE: u32 = 0x0010_0000;
/// The program translated in split mode (several units, table calls), and
/// its unit size in instructions. Only one: the table is process-global.
const SPLIT_PROGRAM: &str = "split";
const SPLIT_UNIT: usize = 8;
/// Where game.ld puts the harness in game mode.
const GAME_TEXT: u32 = 0x0080_0000;
/// Random cases in the default run; `USUM_DIFFTEST_CASES` overrides it.
const DEFAULT_CASES: usize = 20_000;
/// Random load/store cases; `USUM_DIFFTEST_MEM_CASES` overrides it.
const DEFAULT_MEM_CASES: usize = 10_000;
/// Random Thumb cases; `USUM_DIFFTEST_THUMB_CASES` overrides it.
const DEFAULT_THUMB_CASES: usize = 10_000;
/// Random VFP cases; `USUM_DIFFTEST_VFP_CASES` overrides it.
const DEFAULT_VFP_CASES: usize = 10_000;
/// Random media/DSP cases; `USUM_DIFFTEST_MEDIA_CASES` overrides it.
const DEFAULT_MEDIA_CASES: usize = 10_000;
const QEMU_TIMEOUT: Duration = Duration::from_secs(60);

fn tool(var: &str, default: &str) -> String {
    let name = std::env::var(var).unwrap_or_else(|_| default.to_owned());
    let ok = Command::new(&name).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    if !ok {
        panic!(
            "usum-difftest needs `{name}` (set {var} to override). On Ubuntu/Debian: \
             sudo apt install qemu-user binutils-arm-none-eabi. On Fedora: \
             sudo dnf install qemu-user binutils-arm-linux-gnu (and set the variables)."
        );
    }
    name
}

struct Tools {
    r#as: String,
    ld: String,
    objcopy: String,
    qemu: String,
}

fn run(command: &mut Command, what: &str) {
    let output = command.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    if !output.status.success() {
        panic!("{what} failed:\n{}", String::from_utf8_lossy(&output.stderr));
    }
}

/// Runs the ELF under QEMU with a deadline and returns its stdout, or why
/// the reference run failed.
fn reference(tools: &Tools, elf: &Path, out: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::create(out).expect("create expected output");
    let mut child = Command::new(&tools.qemu)
        .args(["-cpu", "arm11mpcore"])
        .arg(elf)
        .stdout(file)
        .stderr(Stdio::piped())
        // core dumps of a crashing reference land in OUT_DIR, not the source tree
        .current_dir(out.parent().expect("output directory"))
        .spawn()
        .expect("start qemu-arm");
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait for qemu-arm") {
            if !status.success() {
                return Err(format!("qemu-arm failed on {}: {status}", elf.display()));
            }
            break;
        }
        if start.elapsed() > QEMU_TIMEOUT {
            let _ = child.kill();
            return Err(format!("qemu-arm timed out on {}", elf.display()));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(std::fs::read(out).expect("read expected output"))
}

/// The instruction text of each case in a generated random program: the
/// lines between a call to the state loader and the call to the saver.
fn case_texts(asm: &str) -> Vec<String> {
    let mut cases = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in asm.lines().map(str::trim) {
        match line {
            "bl load_state" | "blx load_state" | "bl vload_state" => current = Some(Vec::new()),
            "bl save_state" | "blx save_state" | "bl vsave_state" => {
                if let Some(lines) = current.take() {
                    cases.push(lines.join("; "));
                }
            }
            _ => {
                if let Some(lines) = current.as_mut() {
                    lines.push(line);
                }
            }
        }
    }
    cases
}

struct Built {
    name: String,
    split: bool,
    entry: u32,
    /// Words per output record for random programs (0 for hand-written ones).
    record: usize,
    functions: usize,
    from_pointers: usize,
    instructions: usize,
}

/// How a program is linked: the script, extra objects (the game code in
/// game mode), and where its harness `.text` (and so its entry) starts.
struct Layout {
    script: PathBuf,
    extra: Vec<PathBuf>,
    text_start: u32,
}

fn build(tools: &Tools, out_dir: &Path, layout: &Layout, harness: &Path, name: &str, source: &Path) -> Result<Built, String> {
    let asm = std::fs::read_to_string(source).expect("read source");
    let object = out_dir.join(format!("{name}.o"));
    let elf = out_dir.join(format!("{name}.elf"));
    let bin = out_dir.join(format!("{name}.bin"));
    run(
        Command::new(&tools.r#as).args(["-mcpu=mpcore", "-mfpu=vfpv2", "-o"]).arg(&object).arg(source),
        &format!("assembling {name}"),
    );
    run(
        Command::new(&tools.ld)
            .arg("-T")
            .arg(&layout.script)
            .args(["-z", "max-page-size=0x1000", "-o"])
            .arg(&elf)
            .arg(harness)
            .arg(&object)
            .args(&layout.extra),
        &format!("linking {name}"),
    );
    run(Command::new(&tools.objcopy).args(["-O", "binary"]).arg(&elf).arg(&bin), &format!("objcopy {name}"));
    reference(tools, &elf, &out_dir.join(format!("{name}.expected")))?;

    // the code range is the harness and program .text; in game mode only
    // the called game functions are explored, never the whole game image
    let text = out_dir.join(format!("{name}.text"));
    run(
        Command::new(&tools.objcopy).args(["-O", "binary", "-j", ".text"]).arg(&elf).arg(&text),
        &format!("objcopy .text {name}"),
    );
    let entry = layout.text_start;
    let code_end = entry + std::fs::metadata(&text).expect("text size").len() as u32;
    let image = std::fs::read(&bin).expect("read image");
    let image = usum_recomp::Image { base: BASE, bytes: &image, code: entry..code_end };
    let analysis = usum_recomp::analyze(&image, &[entry]);
    let split = name == SPLIT_PROGRAM;
    let source = if split {
        // tiny units, as modules of one file, so calls cross units
        let s = usum_recomp::emit::split(&analysis, SPLIT_UNIT, &|i| format!("u{i:03}"));
        let mut text = String::new();
        for (i, unit) in s.units.iter().enumerate() {
            text += &format!("pub mod u{i:03} {{\n{}}}\n", unit.source);
        }
        text + &s.top
    } else {
        usum_recomp::emit::module(&analysis)
    };
    std::fs::write(out_dir.join(format!("p_{name}.rs")), source).expect("write translation");
    let cases = case_texts(&asm);
    std::fs::write(out_dir.join(format!("{name}.cases")), cases.join("\n")).expect("write case list");
    let record = if cases.is_empty() {
        0
    } else if asm.contains("bl vload_state") {
        47
    } else {
        14
    };
    Ok(Built {
        name: name.to_owned(),
        split,
        entry,
        record,
        functions: analysis.functions.len(),
        from_pointers: analysis.from_pointers,
        instructions: analysis.functions.values().map(|f| f.instrs.len()).sum(),
    })
}

/// Game mode: verifies real game functions when `USUM_DIFFTEST_DUMP` names
/// the user's decrypted dump and `USUM_DIFFTEST_FUNCTIONS` lists leaf
/// function addresses (hex, comma-separated; see `usumctl analyze
/// --leaves`). The decompressed code is written only to OUT_DIR (under
/// target/, never committed) and linked at its real address.
fn game_mode(tools: &Tools, out_dir: &Path, harness: &Path) -> Vec<Built> {
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_DUMP");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_FUNCTIONS");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_GAME_CASES");
    let Some(dump) = std::env::var_os("USUM_DIFFTEST_DUMP") else { return Vec::new() };
    let dump = PathBuf::from(dump);
    println!("cargo:rerun-if-changed={}", dump.display());
    let inspection = usum_import::inspect(&dump).unwrap_or_else(|e| panic!("{}: {e}", dump.display()));
    let usum_import::profile::Match::Profile { profile } = inspection.profile else {
        panic!("{} matches no recorded profile; game mode only runs on recorded executables", dump.display());
    };
    let functions: Vec<u32> = std::env::var("USUM_DIFFTEST_FUNCTIONS")
        .expect("game mode needs USUM_DIFFTEST_FUNCTIONS (hex addresses, comma-separated)")
        .split(',')
        .map(|a| u32::from_str_radix(a.trim().trim_start_matches("0x"), 16).expect("hex function address"))
        .collect();
    let cases = std::env::var("USUM_DIFFTEST_GAME_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    let blob = out_dir.join("game-code.bin");
    std::fs::write(&blob, &inspection.code.bytes).expect("write game code");
    let blob_source = out_dir.join("game-code.s");
    std::fs::write(&blob_source, format!("    .section .game, \"ax\"\n    .incbin \"{}\"\n", blob.display()))
        .expect("write blob source");
    let blob_object = out_dir.join("game-code.o");
    run(Command::new(&tools.r#as).arg("-o").arg(&blob_object).arg(&blob_source), "assembling the game code blob");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let layout = Layout { script: manifest.join("game.ld"), extra: vec![blob_object], text_start: GAME_TEXT };
    functions
        .iter()
        .filter_map(|&address| {
            let name = format!("game_{}_{address:08x}", profile.id.replace('-', "_"));
            let path = out_dir.join(format!("{name}.s"));
            std::fs::write(&path, random::game_program(address, cases)).expect("write game program");
            // a candidate whose reference run fails (it hangs or crashes on
            // random inputs) cannot be compared: report it and go on
            match build(tools, out_dir, &layout, harness, &name, &path) {
                Ok(built) => Some(built),
                Err(why) => {
                    println!("cargo:warning=game mode skips {address:#010x}: {why}");
                    None
                }
            }
        })
        .collect()
}

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let programs = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("programs");
    println!("cargo:rerun-if-changed=programs");
    println!("cargo:rerun-if-changed=build/random.rs");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_CASES");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_MEM_CASES");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_THUMB_CASES");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_VFP_CASES");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_MEDIA_CASES");
    println!("cargo:rerun-if-env-changed=USUM_DIFFTEST_SEED");
    let tools = Tools {
        r#as: tool("USUM_ARM_AS", "arm-none-eabi-as"),
        ld: tool("USUM_ARM_LD", "arm-none-eabi-ld"),
        objcopy: tool("USUM_ARM_OBJCOPY", "arm-none-eabi-objcopy"),
        qemu: tool("USUM_QEMU_ARM", "qemu-arm"),
    };
    let harness = out_dir.join("harness.o");
    run(
        Command::new(&tools.r#as).args(["-mcpu=mpcore", "-mfpu=vfpv2", "-o"]).arg(&harness).arg(programs.join("harness.s")),
        "assembling the harness",
    );

    let mut sources: Vec<(String, PathBuf)> = std::fs::read_dir(&programs)
        .expect("programs directory")
        .map(|e| e.expect("directory entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "s") && p.file_stem().is_some_and(|s| s != "harness"))
        .map(|p| (p.file_stem().unwrap().to_string_lossy().into_owned(), p))
        .collect();
    sources.sort();

    let cases = std::env::var("USUM_DIFFTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_CASES);
    let mem_cases = std::env::var("USUM_DIFFTEST_MEM_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MEM_CASES);
    let media_cases = std::env::var("USUM_DIFFTEST_MEDIA_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MEDIA_CASES);
    let vfp_cases = std::env::var("USUM_DIFFTEST_VFP_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_VFP_CASES);
    let thumb_cases = std::env::var("USUM_DIFFTEST_THUMB_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_THUMB_CASES);
    let random = random::programs(cases)
        .into_iter()
        .chain(random::mem_programs(mem_cases))
        .chain(random::thumb_programs(thumb_cases))
        .chain(random::vfp_programs(vfp_cases))
        .chain(random::media_programs(media_cases));
    for (name, text) in random {
        let path = out_dir.join(format!("{name}.s"));
        std::fs::write(&path, text).expect("write random program");
        sources.push((name, path));
    }

    let plain = Layout { script: programs.join("link.ld"), extra: Vec::new(), text_start: BASE };
    // the hand-written and random corpora must all run under the reference
    let mut built: Vec<Built> = sources
        .iter()
        .map(|(name, path)| build(&tools, &out_dir, &plain, &harness, name, path).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    built.extend(game_mode(&tools, &out_dir, &harness));
    let mut generated = String::new();
    for b in &built {
        generated += &format!(
            "#[allow(unused, unreachable_code, clippy::all)]\npub mod p_{0} {{ include!(concat!(env!(\"OUT_DIR\"), \"/p_{0}.rs\")); }}\n",
            b.name
        );
    }
    generated += "pub static PROGRAMS: &[Program] = &[\n";
    for b in &built {
        generated += &format!(
            "    Program {{ name: \"{0}\", image: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{0}.bin\")), \
             expected: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{0}.expected\")), entries: p_{0}::ENTRIES, entry: {5}, table: {6}, \
             functions: {1}, from_pointers: {2}, instructions: {3}, record: {4}, \
             cases: include_str!(concat!(env!(\"OUT_DIR\"), \"/{0}.cases\")) }},\n",
            b.name,
            b.functions,
            b.from_pointers,
            b.instructions,
            b.record,
            b.entry,
            if b.split { format!("Some(p_{}::FUNCTIONS)", b.name) } else { "None".to_owned() }
        );
    }
    generated += "];\n";
    std::fs::write(out_dir.join("generated.rs"), generated).expect("write generated.rs");
}
