//! usumctl: the development CLI. It inspects dumps with the same library
//! the packaged launcher will use.

mod analyze;
mod paths;
mod translate;

use std::path::PathBuf;
use std::process::{Command, ExitCode};

use usum_import::fixture::{self, Spec};
use usum_import::profile::Match;
use usum_import::{ncch, Inspection};

const USAGE: &str = "\
usage: usumctl inspect <dump> [--json]
       usumctl modules <dump> [--json]
       usumctl analyze <dump> [--detail | --leaves]
       usumctl compare <dump-a> <dump-b>
       usumctl translate <dump> [--out <dir>] [--unit <instructions>] [--allow-in-repo]
       usumctl extract-code <dump> <output>
       usumctl romfs <dump> [<offset>...]
       usumctl make-fixture <output> [--bare] [--encrypted]
       usumctl paths
       usumctl doctor

inspect       validate a dump (structure, every stored SHA-256 on the parts
              read) and say which recorded version profile it is, if any
modules       list the CRO modules (dynamically loaded code) in its RomFS
analyze       run code discovery on the executable (in memory, nothing is
              written) and report coverage and undecodable instructions;
              --detail adds the most frequent encodings of each class;
              --leaves lists pure leaf functions, candidates to verify alone
compare       match the executables' functions between two dumps
translate     translate the whole executable into a Cargo workspace of
              generated crates (default: the private cache; never into a
              git work tree)
extract-code  write the decompressed executable (text, rodata, data) to a
              file for disassembly with external tools; never into a git
              work tree (it is game data)
romfs         list the RomFS files by their offset in the RomFS image (the
              offsets the game reads at); with hex offsets, only the files
              holding them
make-fixture  write a synthetic test image (no game data) for trying the tools
paths         print where generated files, saves and settings will be kept
doctor        check for the tools later milestones need

Exit status: 0 for a recorded profile (inspect) or success, 1 otherwise.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("inspect") => inspect(&args[1..]),
        Some("modules") => modules(&args[1..]),
        Some("analyze") => {
            let detail = args.iter().any(|a| a == "--detail");
            let leaves = args.iter().any(|a| a == "--leaves");
            let rest: Vec<String> = args[1..].iter().filter(|a| *a != "--detail" && *a != "--leaves").cloned().collect();
            load(&rest).and_then(|(i, _)| if leaves { analyze::leaves(&i) } else { analyze::report(&i, detail) })
        }
        Some("compare") => compare(&args[1..]),
        Some("translate") => translate_cmd(&args[1..]),
        Some("extract-code") => extract_code(&args[1..]),
        Some("romfs") => romfs_files(&args[1..]),
        Some("make-fixture") => make_fixture(&args[1..]),
        Some("paths") => print_paths(),
        Some("doctor") => doctor(),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("usumctl: {message}");
            ExitCode::FAILURE
        }
    }
}

/// One positional path and the flags set, refusing anything else.
fn parse<'a>(args: &'a [String], flags: &[&str]) -> Result<(PathBuf, Vec<&'a str>), String> {
    let mut path = None;
    let mut set = Vec::new();
    for arg in args {
        if flags.contains(&arg.as_str()) {
            set.push(arg.as_str());
        } else if arg.starts_with("--") {
            return Err(format!("unknown option {arg}\n\n{USAGE}"));
        } else if path.replace(PathBuf::from(arg)).is_some() {
            return Err(format!("one path at a time\n\n{USAGE}"));
        }
    }
    Ok((path.ok_or(USAGE.to_owned())?, set))
}

fn load(args: &[String]) -> Result<(Inspection, bool), String> {
    let (dump, flags) = parse(args, &["--json"])?;
    let inspection = usum_import::inspect(&dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    Ok((inspection, flags.contains(&"--json")))
}

fn romfs_files(args: &[String]) -> Result<(), String> {
    let [dump, offsets @ ..] = args else { return Err(USAGE.to_owned()) };
    let offsets = offsets
        .iter()
        .map(|o| u64::from_str_radix(o.trim_start_matches("0x"), 16).map_err(|e| format!("{o}: {e}")))
        .collect::<Result<Vec<u64>, String>>()?;
    let i = usum_import::inspect(std::path::Path::new(dump)).map_err(|e| format!("{dump}: {e}"))?;
    let image = i.romfs_image.ok_or("no RomFS")?;
    println!("image        {:#012x} in the dump, {:#x} bytes", image.offset, image.size);
    let mut files: Vec<_> = i.romfs_listing.iter().map(|f| (f.offset - image.offset, f.size, &f.path)).collect();
    files.sort();
    for (at, size, path) in files {
        if offsets.is_empty() || offsets.iter().any(|&o| (at..at + size.max(1)).contains(&o)) {
            println!("{at:#012x} {size:#010x} {path}");
        }
    }
    Ok(())
}

fn extract_code(args: &[String]) -> Result<(), String> {
    let [dump, output] = args else { return Err(USAGE.to_owned()) };
    let output = PathBuf::from(output);
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let parent = parent.canonicalize().map_err(|e| format!("{}: {e}", parent.display()))?;
    if let Some(tree) = paths::enclosing_git_tree(&parent) {
        return Err(format!(
            "{} is inside the git work tree {}: game data must not be committable",
            output.display(),
            tree.display()
        ));
    }
    let i = usum_import::inspect(std::path::Path::new(dump)).map_err(|e| format!("{dump}: {e}"))?;
    std::fs::write(&output, &i.code.bytes).map_err(|e| format!("{}: {e}", output.display()))?;
    println!("wrote {} bytes; load address {:#010x}", i.code.bytes.len(), i.exheader.text.address);
    Ok(())
}

fn profile_line(m: &Match) -> String {
    match m {
        Match::Profile { profile } => format!("{} — recorded profile {} ({})", profile.game.name(), profile.id, profile.release),
        Match::UnknownRevision { game } => format!(
            "{}, but this code matches no recorded profile (another region or revision, or modified): unsupported",
            game.name()
        ),
        Match::OtherTitle => "not Pokémon Ultra Sun or Ultra Moon: unsupported".to_owned(),
    }
}

fn inspect(args: &[String]) -> Result<(), String> {
    let (i, json) = load(args)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&i).map_err(|e| e.to_string())?);
    } else {
        let e = &i.exheader;
        println!("format          {:?}, {} bytes", i.format, i.file_size);
        if let Some(ncsd) = &i.ncsd {
            for (index, p) in ncsd.partitions.iter().enumerate().filter(|(_, p)| p.size != 0) {
                println!("partition {index}     {:#012x} + {:#x} ({})", p.offset, p.size, ncch::partition_role(index));
            }
        }
        println!("program id      {}", i.program_id());
        println!("product code    {}", i.ncch.product_code);
        println!("ncch version    {}", i.ncch.version);
        println!("process         {} (remaster version {})", e.process_name, e.remaster_version);
        println!("text            {:#010x} {:#x} bytes in {} pages", e.text.address, e.text.size, e.text.pages);
        println!("rodata          {:#010x} {:#x} bytes in {} pages", e.rodata.address, e.rodata.size, e.rodata.pages);
        println!(
            "data            {:#010x} {:#x} bytes in {} pages, bss {:#x}",
            e.data.address, e.data.size, e.data.pages, e.bss_size
        );
        println!("stack           {:#x} bytes", e.stack_size);
        println!(
            "code            {} bytes ({} stored{}), sha256 {}",
            i.code.size,
            i.code.stored_size,
            if e.compressed_code { ", compressed" } else { "" },
            i.code.sha256
        );
        println!("save data size  {:#x}", e.save_data_size);
        println!("services        {}", e.services.join(" "));
        println!("dependencies    {} system modules", e.dependencies.len());
        println!("system mode     Old 3DS {}, New 3DS {}", e.old3ds_system_mode, e.new3ds_system_mode);
        if let Some((major, minor)) = e.kernel_version {
            println!("kernel version  {major}.{minor}");
        }
        println!("romfs           {} files, {} code modules", i.romfs_files, i.modules.len());
        println!("integrity       exheader, ExeFS header, .code and RomFS superblock SHA-256 all match");
        for note in &i.notes {
            println!("note            {note}");
        }
        println!("profile         {}", profile_line(&i.profile));
    }
    match i.profile {
        Match::Profile { .. } => Ok(()),
        _ => Err("no recorded profile matches this dump".to_owned()),
    }
}

fn modules(args: &[String]) -> Result<(), String> {
    let (i, json) = load(args)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&i.modules).map_err(|e| e.to_string())?);
    } else {
        for m in &i.modules {
            println!("{:<32} {:>9} {}{}", m.path, m.size, m.sha256, if m.cro_magic { "" } else { "  (no CRO0 magic)" });
        }
    }
    Ok(())
}

fn compare(args: &[String]) -> Result<(), String> {
    let [a, b] = args else { return Err(USAGE.to_owned()) };
    let load = |p: &String| usum_import::inspect(std::path::Path::new(p)).map_err(|e| format!("{p}: {e}"));
    analyze::compare(&load(a)?, &load(b)?)
}

fn translate_cmd(args: &[String]) -> Result<(), String> {
    let (mut dump, mut out, mut unit, mut allow) = (None, None, translate::DEFAULT_UNIT, false);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--out" => {
                i += 1;
                out = Some(PathBuf::from(args.get(i).ok_or("--out needs a directory")?));
            }
            "--unit" => {
                i += 1;
                unit = args.get(i).and_then(|v| v.parse().ok()).filter(|&n| n > 0).ok_or("--unit needs a positive number")?;
            }
            "--allow-in-repo" => allow = true,
            a if a.starts_with("--") => return Err(format!("unknown option {a}\n\n{USAGE}")),
            a if dump.is_none() => dump = Some(PathBuf::from(a)),
            _ => return Err(USAGE.to_owned()),
        }
        i += 1;
    }
    let dump = dump.ok_or(USAGE.to_owned())?;
    let inspection = usum_import::inspect(&dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    translate::translate(&inspection, &dump, out, unit, allow).map(|_| ())
}

fn make_fixture(args: &[String]) -> Result<(), String> {
    let (out, flags) = parse(args, &["--bare", "--encrypted"])?;
    let spec = Spec { cartridge: !flags.contains(&"--bare"), encrypted: flags.contains(&"--encrypted"), ..Spec::default() };
    std::fs::write(&out, fixture::build(&spec)).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote a synthetic {} to {}", if spec.cartridge { "cartridge image" } else { "executable" }, out.display());
    Ok(())
}

fn print_paths() -> Result<(), String> {
    let show = |p: Option<PathBuf>| p.map_or("(unset: neither XDG nor HOME is set)".to_owned(), |p| p.display().to_string());
    println!("generated  {}", show(paths::cache_dir()));
    println!("saves      {}", show(paths::data_dir()));
    println!("settings   {}", show(paths::config_dir()));
    Ok(())
}

/// A tool's version line, or None when it does not run.
fn tool(name: &str) -> Option<String> {
    let output = Command::new(name).arg("--version").output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).lines().next().unwrap_or("").to_owned())
}

fn doctor() -> Result<(), String> {
    let sandbox = std::env::var("FLATPAK_ID").map_or("none (host or toolbox)".to_owned(), |id| format!("Flatpak {id}"));
    println!("sandbox  {sandbox}");
    // Local Rust generation (milestone 3 on) needs a Rust toolchain; under
    // Flatpak it comes from the rust-stable SDK extension (ADR-0003).
    let mut missing = 0;
    for name in ["rustc", "cargo"] {
        match tool(name) {
            Some(version) => println!("ok       {version}"),
            None => {
                missing += 1;
                println!("missing  {name} (needed from milestone 3 on)");
            }
        }
    }
    if missing == 0 {
        Ok(())
    } else {
        Err(format!("{missing} tools missing"))
    }
}
