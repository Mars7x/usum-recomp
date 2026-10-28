//! `usumctl analyze`: runs code discovery on a dump's executable in memory
//! and reports coverage. Nothing derived from the dump is written.

use std::collections::BTreeMap;
use std::time::Instant;

use usum_arm::{unsupported_class, Op};
use usum_import::Inspection;
use usum_recomp::{analyze, Image, Origin};

/// Unsupported instruction sites per class, split by whether the function
/// was reached by control flow (entry or call) or only by a pointer seed,
/// which may be data that merely looks like a code address.
#[derive(Default)]
struct Classes {
    reached: BTreeMap<&'static str, usize>,
    seeded: BTreeMap<&'static str, usize>,
    /// For `--detail`: encodings seen per class at reached sites.
    encodings: BTreeMap<&'static str, BTreeMap<u32, usize>>,
}

/// How many of the most frequent encodings `--detail` shows per class;
/// `USUMCTL_DETAIL_ROWS` overrides it.
const DETAIL_ROWS: usize = 8;

fn detail_rows() -> usize {
    std::env::var("USUMCTL_DETAIL_ROWS").ok().and_then(|v| v.parse().ok()).unwrap_or(DETAIL_ROWS)
}

/// Leaf functions that are pure enough to verify in isolation: ARM, no
/// calls, no SVCs, no exits from the image, every instruction supported,
/// and memory only through SP or PC-relative literals.
fn pure_leaves(analysis: &usum_recomp::Analysis) -> Vec<(u32, usize, bool, bool)> {
    use usum_arm::{Op, PC};
    const SP: u8 = 13;
    let mut out = Vec::new();
    for f in analysis.functions.values() {
        if f.thumb || !f.calls.is_empty() || !f.exits.is_empty() || f.origin == Origin::Pointer {
            continue;
        }
        let mut pure = true;
        let (mut looping, mut vfp) = (false, false);
        for (&address, instr) in &f.instrs {
            match instr.op {
                Op::Unsupported | Op::Svc { .. } | Op::Exclusive { .. } | Op::ReadTls { .. } => pure = false,
                Op::Mem { rn, .. } | Op::Block { rn, .. } | Op::VfpBlock { rn, .. } if rn != SP && rn != PC => pure = false,
                Op::VfpMem { rn, .. } if rn != SP && rn != PC => pure = false,
                Op::Bx { link: true, .. } => pure = false,
                Op::Branch { target, link: false, .. } if target <= address => looping = true,
                Op::VfpData { .. } => vfp = true,
                _ => {}
            }
        }
        if pure {
            out.push((f.start, f.instrs.len(), looping, vfp));
        }
    }
    out
}

pub fn leaves(inspection: &Inspection) -> Result<(), String> {
    let e = &inspection.exheader;
    let base = e.text.address;
    let image = Image { base, bytes: &inspection.code.bytes, code: base..base + e.text.size };
    let analysis = analyze(&image, &[base]);
    let mut list = pure_leaves(&analysis);
    list.sort_by_key(|&(start, n, looping, _)| (std::cmp::Reverse(looping), std::cmp::Reverse(n), start));
    println!("{} pure leaf functions (ARM, reached by calls, memory only via SP or literals)", list.len());
    println!("address     instructions  loop  vfp");
    for (start, n, looping, vfp) in list.iter().take(40) {
        println!("{start:#010x}  {n:>12}  {:>4}  {:>3}", if *looping { "yes" } else { "" }, if *vfp { "yes" } else { "" });
    }
    Ok(())
}

pub fn report(inspection: &Inspection, detail: bool) -> Result<(), String> {
    let e = &inspection.exheader;
    let base = e.text.address;
    let code = base..base.checked_add(e.text.size).ok_or("text segment wraps the address space")?;
    let image = Image { base, bytes: &inspection.code.bytes, code: code.clone() };

    let start = Instant::now();
    // the entry point of a 3DS application is the start of its text segment
    let analysis = analyze(&image, &[base]);
    let took = start.elapsed();

    let mut classes = Classes::default();
    let (mut arm, mut thumb, mut labels, mut exits) = (0usize, 0usize, 0usize, 0usize);
    let mut by_origin: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut covered = std::collections::BTreeSet::new();
    for f in analysis.functions.values() {
        let origin = match f.origin {
            Origin::Entry => "entry",
            Origin::Call => "call",
            Origin::Pointer => "pointer",
        };
        let slot = by_origin.entry(origin).or_default();
        if f.thumb {
            slot.1 += 1;
        } else {
            slot.0 += 1;
        }
        labels += f.labels.len();
        exits += f.exits.len();
        for (&address, instr) in &f.instrs {
            if !covered.insert(address) {
                continue; // code shared by several functions counts once
            }
            if instr.thumb {
                thumb += 1;
            } else {
                arm += 1;
            }
            if instr.op == Op::Unsupported {
                let class = unsupported_class(instr);
                if f.origin == Origin::Pointer {
                    *classes.seeded.entry(class).or_default() += 1;
                } else {
                    *classes.reached.entry(class).or_default() += 1;
                    *classes.encodings.entry(class).or_default().entry(instr.raw).or_default() += 1;
                }
            }
        }
    }
    let covered_bytes: u64 = covered.len() as u64 * 4; // upper bound; Thumb instructions are 2 bytes
    let thumb_adjust = thumb as u64 * 2;
    let text = u64::from(e.text.size);

    println!("executable      {} ({}), text {:#010x}..{:#010x}", inspection.program_id(), e.process_name, code.start, code.end);
    println!("analysis        {:.2} s", took.as_secs_f64());
    for (origin, (a, t)) in &by_origin {
        println!("functions       {a:>6} ARM, {t:>5} Thumb   from {origin}");
    }
    println!("functions       {:>6} total", analysis.functions.len());
    println!("instructions    {arm} ARM + {thumb} Thumb, distinct addresses");
    println!(
        "text covered    {:.1} % of {} bytes decoded as code",
        100.0 * (covered_bytes - thumb_adjust) as f64 / text as f64,
        text
    );
    println!("labels          {labels} (block starts and entries)");
    println!("exits           {exits} control-flow edges leave the image");
    for (title, map) in [("reached by control flow", &classes.reached), ("only in pointer-seeded functions", &classes.seeded)] {
        let total: usize = map.values().sum();
        println!("unsupported     {total} sites {title}");
        let mut rows: Vec<_> = map.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (class, n) in rows {
            println!("    {n:>7}  {class}");
            if detail && std::ptr::eq(map, &classes.reached) {
                let mut top: Vec<_> = classes.encodings[class].iter().collect();
                top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
                for (raw, count) in top.into_iter().take(detail_rows()) {
                    println!("             {count:>6} x {raw:08x}");
                }
            }
        }
    }
    Ok(())
}

/// The bytes a function's instructions occupy, in address order.
fn function_bytes(image: &Image, f: &usum_recomp::Function) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (&address, instr) in &f.instrs {
        let offset = (address - image.base) as usize;
        bytes.extend_from_slice(&image.bytes[offset..offset + usize::from(instr.size)]);
    }
    bytes
}

/// Compares the functions two dumps' executables contain: which are
/// byte-identical at the same address, which are identical elsewhere
/// (moved), and which have no identical counterpart. Functions found only
/// through pointer seeds are left out, since many are data.
pub fn compare(a: &Inspection, b: &Inspection) -> Result<(), String> {
    use std::collections::HashMap;
    let analyzed = |i: &Inspection| {
        let e = &i.exheader;
        let base = e.text.address;
        let image = Image { base, bytes: &i.code.bytes, code: base..base + e.text.size };
        let analysis = analyze(&image, &[base]);
        let functions: Vec<(u32, Vec<u8>)> = analysis
            .functions
            .values()
            .filter(|f| f.origin != Origin::Pointer)
            .map(|f| (f.key(), function_bytes(&image, f)))
            .collect();
        functions
    };
    let (fa, fb) = (analyzed(a), analyzed(b));
    let by_key: HashMap<u32, &Vec<u8>> = fb.iter().map(|(k, v)| (*k, v)).collect();
    let mut by_content: HashMap<&Vec<u8>, usize> = HashMap::new();
    for (_, v) in &fb {
        *by_content.entry(v).or_default() += 1;
    }
    let (mut same, mut moved, mut differ) = (0usize, 0usize, 0usize);
    let (mut same_instr, mut moved_instr, mut differ_instr) = (0usize, 0usize, 0usize);
    for (key, bytes) in &fa {
        let n = bytes.len();
        if by_key.get(key).is_some_and(|other| *other == bytes) {
            same += 1;
            same_instr += n;
        } else if by_content.contains_key(bytes) {
            moved += 1;
            moved_instr += n;
        } else {
            differ += 1;
            differ_instr += n;
        }
    }
    let total = (same_instr + moved_instr + differ_instr).max(1) as f64;
    println!(
        "{} ({} functions) against {} ({} functions), control-flow-reached functions only",
        a.program_id(),
        fa.len(),
        b.program_id(),
        fb.len()
    );
    println!("identical at the same address  {same:>6} functions, {:>5.1} % of code bytes", 100.0 * same_instr as f64 / total);
    println!("identical at another address   {moved:>6} functions, {:>5.1} % of code bytes", 100.0 * moved_instr as f64 / total);
    println!(
        "no identical counterpart       {differ:>6} functions, {:>5.1} % of code bytes",
        100.0 * differ_instr as f64 / total
    );
    Ok(())
}
