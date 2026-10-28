//! Tests against the user's own decrypted dumps. They are ignored by
//! default, since no dump is (or may be) in the repository. Run them with
//!
//!   USUM_DUMP_US=/path/to/ultra-sun.3ds USUM_DUMP_UM=/path/to/ultra-moon.3ds \
//!       cargo test -p usum-import --test real_dumps -- --ignored
//!
//! A missing variable fails the test rather than skipping it, so "passed"
//! always means both dumps were read.

use std::path::PathBuf;

use usum_import::profile::Match;
use usum_import::{inspect, Game, Inspection};

fn load(var: &str) -> Inspection {
    let path = std::env::var_os(var).unwrap_or_else(|| panic!("set {var} to a decrypted dump to run this test"));
    inspect(&PathBuf::from(path)).expect("the dump should inspect cleanly")
}

fn expect_profile(i: &Inspection, game: Game, id: &str) {
    match &i.profile {
        Match::Profile { profile } => {
            assert_eq!((profile.game, profile.id), (game, id));
            assert_eq!(profile.code_size, i.code.size);
            assert_eq!(profile.product_code, i.ncch.product_code);
            assert_eq!(profile.ncch_version, i.ncch.version);
        }
        other => panic!("expected profile {id}, got {other:?}"),
    }
    assert!(i.notes.is_empty(), "unexpected notes: {:?}", i.notes);
}

#[test]
#[ignore = "needs USUM_DUMP_US, a decrypted Ultra Sun dump"]
fn ultra_sun_matches_its_profile() {
    expect_profile(&load("USUM_DUMP_US"), Game::UltraSun, "ultra-sun-usa-cart");
}

#[test]
#[ignore = "needs USUM_DUMP_UM, a decrypted Ultra Moon dump"]
fn ultra_moon_matches_its_profile() {
    expect_profile(&load("USUM_DUMP_UM"), Game::UltraMoon, "ultra-moon-usa-cart");
}

/// The measured split that decides how much translation output the games
/// can share (docs/RESEARCH.md, R-005): same memory layout and services,
/// same module set, 123 of 132 CROs byte-identical, executables and
/// static.crs different.
#[test]
#[ignore = "needs USUM_DUMP_US and USUM_DUMP_UM"]
fn the_games_share_layout_and_most_modules() {
    let (us, um) = (load("USUM_DUMP_US"), load("USUM_DUMP_UM"));
    assert_eq!(us.exheader.services, um.exheader.services);
    assert_eq!(us.exheader.dependencies, um.exheader.dependencies);
    assert_eq!((us.exheader.rodata, us.exheader.data), (um.exheader.rodata, um.exheader.data));
    assert_eq!(us.exheader.text.address, um.exheader.text.address);
    assert_ne!(us.code.sha256, um.code.sha256);

    let paths = |i: &Inspection| i.modules.iter().map(|m| m.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&us), paths(&um));
    let cro = |i: &Inspection| i.modules.iter().filter(|m| m.path.ends_with(".cro")).cloned().collect::<Vec<_>>();
    let (us_cro, um_cro) = (cro(&us), cro(&um));
    let identical = us_cro.iter().zip(&um_cro).filter(|(a, b)| a.sha256 == b.sha256).count();
    assert_eq!((us_cro.len(), identical), (132, 123));
    assert!(us.modules.iter().all(|m| m.cro_magic));
}

/// Every module parses: tables inside the file, every relocation of a
/// known type inside its segment, import lists terminated.
fn modules_parse(var: &str) {
    let i = load(var);
    let path = PathBuf::from(std::env::var_os(var).unwrap());
    let file = std::fs::File::open(path).unwrap();
    let (mut crs, mut relocations) = (0, 0);
    for m in &i.modules {
        let mut bytes = vec![0; m.size as usize];
        std::os::unix::fs::FileExt::read_exact_at(&file, &mut bytes, m.offset).unwrap();
        let cro = usum_import::cro::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", m.path));
        crs += usize::from(cro.is_crs);
        relocations += cro.internal.len() + cro.imports.iter().map(|i| i.relocations.len()).sum::<usize>();
        let texts = cro.segments.iter().filter(|s| s.kind == usum_import::cro::SegmentKind::Text && s.size > 0).count();
        assert!(texts <= 1, "{}: {texts} text segments", m.path);
        if cro.is_crs {
            eprintln!("{} {:x?}", m.path, cro.segments);
        }
    }
    assert_eq!(crs, 1, "one CRS");
    eprintln!("{} modules, {relocations} relocations", i.modules.len());
}

#[test]
#[ignore = "needs USUM_DUMP_US, a decrypted Ultra Sun dump"]
fn ultra_sun_modules_parse() {
    modules_parse("USUM_DUMP_US");
}

#[test]
#[ignore = "needs USUM_DUMP_UM, a decrypted Ultra Moon dump"]
fn ultra_moon_modules_parse() {
    modules_parse("USUM_DUMP_UM");
}

#[test]
#[ignore = "needs USUM_DUMP_US, a decrypted Ultra Sun dump"]
fn relocation_kinds() {
    let i = load("USUM_DUMP_US");
    let path = PathBuf::from(std::env::var_os("USUM_DUMP_US").unwrap());
    let file = std::fs::File::open(path).unwrap();
    let mut kinds = std::collections::BTreeMap::new();
    for m in &i.modules {
        let mut bytes = vec![0; m.size as usize];
        std::os::unix::fs::FileExt::read_exact_at(&file, &mut bytes, m.offset).unwrap();
        let cro = usum_import::cro::parse(&bytes).unwrap();
        for r in cro.imports.iter().flat_map(|i| &i.relocations) {
            let kind = cro.segments[usize::from(r.target.segment)].kind;
            *kinds.entry((cro.is_crs, r.kind, format!("import into {kind:?}"))).or_insert(0usize) += 1;
        }
        for r in &cro.internal {
            let kind = cro.segments[usize::from(r.target.segment)].kind;
            let to = cro.segments.get(usize::from(r.segment_or_last)).map(|s| s.kind);
            *kinds.entry((cro.is_crs, r.kind, format!("internal into {kind:?} to {to:?}"))).or_insert(0usize) += 1;
        }
    }
    for ((crs, kind, segment), n) in kinds {
        eprintln!("crs={crs} type {kind:2} into {segment:6}: {n}");
    }
}
