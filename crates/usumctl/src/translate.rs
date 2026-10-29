//! `usumctl translate`: translates a dump's whole executable into a Cargo
//! workspace of generated crates (one per unit of about `--unit`
//! instructions, plus a top crate holding the function and entry tables).
//!
//! The output is derived from the user's dump. It goes to the private XDG
//! cache by default and is refused inside any git work tree.

use std::path::{Path, PathBuf};

use usum_import::cro::{self, ImportRef, SegmentOffset};
use usum_import::profile::Match;
use usum_import::Inspection;
use usum_recomp::{analyze, analyze_with, emit, Analysis, Image};

use crate::paths;

/// Instructions per generated crate: a unit this size compiles in roughly
/// 1 GB (RESEARCH.md R-022).
pub const DEFAULT_UNIT: usize = 20_000;

/// Writes `contents` unless the file already holds exactly that, so an
/// unchanged unit keeps its timestamp and Cargo does not rebuild it.
fn write(path: &Path, contents: &str) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|old| old == contents.as_bytes()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

fn unit_name(i: usize) -> String {
    format!("usum_game_u{i:03}")
}

fn module_unit_name(i: usize) -> String {
    format!("usum_mods_u{i:03}")
}

/// Where module k (in RomFS path order, the CRS left out) is translated:
/// virtual bases 16 MB apart from 0x40000000 (ADR-0007). They only have
/// to be distinct; at run time `cpu.module_delta` maps them to wherever
/// RO loaded the module.
pub const MODULE_SPACE: u32 = 0x4000_0000;
pub const MODULE_STRIDE: u32 = 0x0100_0000;

/// One translated module: its name, the SHA-256 of its file (checked
/// against what the game loads), its virtual base.
struct ModuleRow {
    name: String,
    sha256: String,
    base: u32,
}

/// Discovery over every CRO module, each at its virtual base, seeded from
/// its own tables (`Cro::code_seeds`) and from other modules' anonymous
/// imports of it, never from its unrelocated data words.
/// The dump's modules as stored: the CROs (SHA-256, bytes, parsed), the
/// CRS, and what anyone imports anonymously, by exporting module name.
struct Modules {
    cros: Vec<(String, Vec<u8>, cro::Cro)>,
    crs: Option<cro::Cro>,
    anonymous: std::collections::HashMap<String, Vec<SegmentOffset>>,
}

impl Modules {
    /// Code the modules say the executable has: the CRS's exports in
    /// text, and the anonymous imports of it. Nothing in the executable
    /// itself need point there (R-030).
    fn executable_seeds(&self) -> Vec<u32> {
        let Some(crs) = &self.crs else { return Vec::new() };
        let mut seeds = crs.code_seeds();
        let text = crs.text().map(|t| t.0);
        for at in self.anonymous.get(&crs.name).into_iter().flatten().filter(|at| Some(usize::from(at.segment)) == text) {
            seeds.push(crs.segments[usize::from(at.segment)].offset.wrapping_add(at.offset));
        }
        seeds
    }
}

fn read_modules(inspection: &Inspection, dump: &Path) -> Result<Modules, String> {
    use std::os::unix::fs::FileExt;
    let file = std::fs::File::open(dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    let mut modules = Vec::new();
    let mut crs = None;
    // what anyone imports anonymously, the CRS (the executable) included
    let mut anonymous: std::collections::HashMap<String, Vec<SegmentOffset>> = std::collections::HashMap::new();
    for m in &inspection.modules {
        let mut bytes = vec![0; m.size as usize];
        file.read_exact_at(&mut bytes, m.offset).map_err(|e| format!("{}: {e}", m.path))?;
        let c = cro::parse(&bytes).map_err(|e| format!("{}: {e}", m.path))?;
        for i in &c.imports {
            if let ImportRef::Anonymous { module, at } = &i.what {
                anonymous.entry(module.clone()).or_default().push(*at);
            }
        }
        if c.is_crs {
            crs = Some(c);
        } else {
            modules.push((m.sha256.clone(), bytes, c));
        }
    }
    Ok(Modules { cros: modules, crs, anonymous })
}

fn analyze_modules(all: &Modules) -> Result<(Analysis, Vec<ModuleRow>), String> {
    let (modules, anonymous) = (&all.cros, &all.anonymous);
    let mut merged = Analysis::default();
    let mut rows = Vec::new();
    for (k, (sha256, bytes, c)) in modules.iter().enumerate() {
        let base = MODULE_SPACE + k as u32 * MODULE_STRIDE;
        if bytes.len() as u64 > u64::from(MODULE_STRIDE) || u64::from(base) + u64::from(MODULE_STRIDE) > 1 << 32 {
            return Err(format!("{}: does not fit the module space", c.name));
        }
        rows.push(ModuleRow { name: c.name.clone(), sha256: sha256.clone(), base });
        let Some((text, start, size)) = c.text() else { continue };
        let mut seeds: Vec<u32> = c.code_seeds().into_iter().map(|s| base + s).collect();
        seeds.extend(
            anonymous
                .get(&c.name)
                .into_iter()
                .flatten()
                .filter(|at| usize::from(at.segment) == text)
                .map(|at| base + start + at.offset),
        );
        let image = Image { base, bytes, code: base + start..base + start + size };
        let a = analyze_with(&image, &seeds, false);
        merged.from_entries += a.from_entries;
        merged.from_calls += a.from_calls;
        merged.from_pointers += a.from_pointers;
        merged.functions.extend(a.functions);
    }
    Ok((merged, rows))
}

pub fn translate(
    inspection: &Inspection,
    dump: &Path,
    out: Option<PathBuf>,
    unit: usize,
    allow_in_repo: bool,
) -> Result<PathBuf, String> {
    let Match::Profile { profile } = &inspection.profile else {
        return Err("only recorded version profiles can be translated (see `usumctl inspect`)".to_owned());
    };
    let out = match out {
        Some(dir) => dir,
        None => paths::cache_dir().ok_or("no cache directory: set XDG_CACHE_HOME or HOME")?.join("translations").join(profile.id),
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let out = out.canonicalize().map_err(|e| format!("{}: {e}", out.display()))?;
    if let Some(tree) = paths::enclosing_git_tree(&out) {
        if !allow_in_repo {
            return Err(format!(
                "{} is inside the git work tree {}: code generated from a dump must not be committable",
                out.display(),
                tree.display()
            ));
        }
    }
    // the generated crates build against this checkout's runtime crates
    let crate_dir = |name: &str| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name).canonicalize().map_err(|e| format!("{name}: {e}"))
    };
    let (guest, runtime, import) = (crate_dir("usum-guest")?, crate_dir("usum-runtime")?, crate_dir("usum-import")?);

    let e = &inspection.exheader;
    let base = e.text.address;
    let image = Image { base, bytes: &inspection.code.bytes, code: base..base + e.text.size };
    let modules = read_modules(inspection, dump)?;
    let mut entries = vec![base];
    entries.extend(modules.executable_seeds());
    let analysis = analyze(&image, &entries);
    let split = emit::split(&analysis, unit, &unit_name);
    let (module_analysis, module_rows) = analyze_modules(&modules)?;
    let module_split = emit::split_with(&module_analysis, unit, &module_unit_name, analysis.functions.len(), true);

    let header = "#![forbid(unsafe_code)]\n#![allow(unused, unreachable_code, clippy::all)]\n";
    let mut members = Vec::new();
    for (i, u) in split.units.iter().enumerate() {
        let name = unit_name(i);
        write(&out.join(&name).join("Cargo.toml"), &format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nusum-guest.workspace = true\n"))?;
        write(&out.join(&name).join("src/lib.rs"), &format!("{header}{}", u.source))?;
        members.push(name);
    }
    for (i, u) in module_split.units.iter().enumerate() {
        let name = module_unit_name(i);
        write(&out.join(&name).join("Cargo.toml"), &format!("[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nusum-guest.workspace = true\n"))?;
        write(&out.join(&name).join("src/lib.rs"), &format!("{header}{}", u.source))?;
        members.push(name);
    }
    // units from an earlier translation that this one no longer has
    if let Ok(entries) = std::fs::read_dir(&out) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("usum_game_u") || name.starts_with("usum_mods_u")) && !members.contains(&name) {
                std::fs::remove_dir_all(entry.path()).map_err(|e| format!("{}: {e}", entry.path().display()))?;
            }
        }
    }
    let deps: String = members.iter().map(|m| format!("{m} = {{ path = \"../{m}\" }}\n")).collect();
    write(&out.join("usum_game/Cargo.toml"), &format!("[package]\nname = \"usum_game\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nusum-guest.workspace = true\n{deps}"))?;
    // modules: their function and entry tables (virtual keys), and which
    // module is translated at which virtual base
    let rows: String = module_rows.iter().map(|m| format!("    ({:?}, {:?}, {:#010x}),\n", m.name, m.sha256, m.base)).collect();
    let modules = format!(
        "pub mod modules {{\n{}\n/// (module name, SHA-256 of its file, virtual base)\npub static MODULES: &[(&str, &str, u32)] = &[\n{rows}];\n}}\n",
        module_split.top
    );
    write(&out.join("usum_game/src/lib.rs"), &format!("{header}{}{modules}", split.top))?;
    members.push("usum_game".to_owned());
    // the probe: runs the translation from the entry point to the first SVC
    write(&out.join("probe/Cargo.toml"), "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nusum-runtime.workspace = true\nusum_game = { path = \"../usum_game\" }\n")?;
    write(
        &out.join("probe/src/main.rs"),
        "#![forbid(unsafe_code)]\nfn main() -> std::process::ExitCode {\n    let modules = usum_runtime::probe::Modules { functions: usum_game::modules::FUNCTIONS, entries: usum_game::modules::ENTRIES, translated: usum_game::modules::MODULES };\n    usum_runtime::probe::main(usum_game::FUNCTIONS, usum_game::ENTRIES, modules)\n}\n",
    )?;
    members.push("probe".to_owned());
    // play: the same code in a window with live input (usum-runtime's
    // window and Vulkan features)
    write(
        &out.join("play/Cargo.toml"),
        "[package]\nname = \"play\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nusum-runtime = { workspace = true, features = [\"window\", \"vulkan-compare\"] }\nusum_game = { path = \"../usum_game\" }\n",
    )?;
    write(
        &out.join("play/src/main.rs"),
        "#![forbid(unsafe_code)]\nfn main() -> std::process::ExitCode {\n    let modules = usum_runtime::probe::Modules { functions: usum_game::modules::FUNCTIONS, entries: usum_game::modules::ENTRIES, translated: usum_game::modules::MODULES };\n    usum_runtime::play::main(usum_game::FUNCTIONS, usum_game::ENTRIES, modules)\n}\n",
    )?;
    members.push("play".to_owned());
    let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    write(
        &out.join("Cargo.toml"),
        &format!(
            "# Generated by usumctl translate from {} ({}). Derived from the user's dump: never share or commit.\n[workspace]\nresolver = \"2\"\nmembers = [{}]\n\n[workspace.dependencies]\nusum-guest = {{ path = \"{}\" }}\nusum-runtime = {{ path = \"{}\" }}\nusum-import = {{ path = \"{}\" }}\n\n[profile.release]\ndebug = 0\n",
            profile.id,
            inspection.program_id(),
            list.join(", "),
            guest.display(),
            runtime.display(),
            import.display()
        ),
    )?;
    let instructions: usize = split.units.iter().map(|u| u.instructions).sum();
    println!("translated     {} functions, {instructions} instructions", analysis.functions.len());
    let module_instructions: usize = module_split.units.iter().map(|u| u.instructions).sum();
    println!(
        "modules        {} modules, {} functions, {module_instructions} instructions",
        module_rows.len(),
        module_analysis.functions.len()
    );
    println!(
        "units          {} + {} crates of up to {unit} instructions, plus usum_game",
        split.units.len(),
        module_split.units.len()
    );
    println!("output         {}", out.display());
    Ok(out)
}
