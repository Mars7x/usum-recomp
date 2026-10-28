//! Every program's recompiled output must equal QEMU's, byte for byte.

use usum_difftest::{execute, Program, PROGRAMS};

/// Explains a difference. The first 56 bytes are the harness's register
/// dump; random programs then hold one record per case (r0-r12 and the
/// flags, then for VFP s0-s31 and FPSCR), and the case's instruction and
/// every differing word of its record are shown.
fn describe(program: &Program, actual: &[u8]) -> String {
    let (name, expected) = (program.name, program.expected);
    let Some(first) = expected.iter().zip(actual).position(|(a, b)| a != b) else {
        return format!("{name}: lengths differ: expected {}, got {}", expected.len(), actual.len());
    };
    let word = |bytes: &[u8], at: usize| match bytes.get(at..at + 4) {
        Some(w) => format!("{:#010x}", u32::from_le_bytes(w.try_into().unwrap())),
        None => "(missing)".to_owned(),
    };
    let at = first & !3;
    if at < 56 {
        return format!(
            "{name}: harness register dump, word {}: expected {}, got {}",
            at / 4,
            word(expected, at),
            word(actual, at)
        );
    }
    if program.record == 0 {
        return format!("{name}: output byte {:#x}: expected {}, got {}", at - 56, word(expected, at), word(actual, at));
    }
    let size = program.record * 4;
    let case = (at - 56) / size;
    let start = 56 + case * size;
    let text = program.cases.lines().nth(case).unwrap_or("?");
    let name_of = |i: usize| match i {
        0..=12 => format!("r{i}"),
        13 => "flags".to_owned(),
        46 => "fpscr".to_owned(),
        _ => format!("s{}", i - 14),
    };
    let diffs: Vec<String> = (0..program.record)
        .filter_map(|i| {
            let (e, a) = (word(expected, start + 4 * i), word(actual, start + 4 * i));
            (e != a).then(|| format!("{}: expected {}, got {}", name_of(i), e, a))
        })
        .collect();
    format!("{name}: case {case} `{text}`: {}", diffs.join("; "))
}

#[test]
fn recompiled_programs_match_qemu() {
    assert!(PROGRAMS.len() >= 2, "the corpus is missing");
    let mut failures = Vec::new();
    for program in PROGRAMS {
        match execute(program) {
            Ok(output) if output == program.expected => {}
            Ok(output) => failures.push(describe(program, &output)),
            Err(failure) => failures.push(format!("{}: {failure:?}", program.name)),
        }
    }
    let total: usize = PROGRAMS.iter().map(|p| p.instructions).sum();
    eprintln!("{} programs, {total} translated instructions, {} failed", PROGRAMS.len(), failures.len());
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
