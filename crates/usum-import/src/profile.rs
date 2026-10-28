//! Version profiles: the exact executables this project has recorded.
//!
//! A dump is matched by program ID *and* SHA-256 of its decompressed code,
//! never by file name. Each profile was measured from a real dump; the
//! evidence (date, tool, dump description) is in docs/COMPATIBILITY.md.
//! Being listed here means "identified", not "runs": run status is tracked
//! separately in COMPATIBILITY.md.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Game {
    UltraSun,
    UltraMoon,
}

impl Game {
    pub fn name(self) -> &'static str {
        match self {
            Game::UltraSun => "Pokémon Ultra Sun",
            Game::UltraMoon => "Pokémon Ultra Moon",
        }
    }

    /// Filesystem-safe name for per-game directories (saves, generated
    /// code), so the two games never share state.
    pub fn slug(self) -> &'static str {
        match self {
            Game::UltraSun => "ultra-sun",
            Game::UltraMoon => "ultra-moon",
        }
    }
}

/// Program IDs from the NCCH headers of the measured dumps.
pub const TITLES: &[(u64, Game)] = &[(0x0004_0000_001B_5000, Game::UltraSun), (0x0004_0000_001B_5100, Game::UltraMoon)];

/// One exact executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Profile {
    /// Stable identifier used in paths and logs.
    pub id: &'static str,
    pub game: Game,
    pub program_id: u64,
    /// Where the dump came from, as recorded when it was measured.
    pub release: &'static str,
    pub product_code: &'static str,
    /// The release's region (from where the dump came from).
    pub region: Region,
    pub ncch_version: u16,
    /// SHA-256 of the decompressed ExeFS `.code`.
    pub code_sha256: &'static str,
    pub code_size: usize,
}

/// Console regions, numbered as cfg's SecureInfoGetRegion reports them
/// (3dbrew "CFG:SecureInfoGetRegion"; libctru `CFG_REGION_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Region {
    Usa = 1,
}

pub const PROFILES: &[Profile] = &[
    // Measured 2026-10-25 from the user's USA cartridge dumps.
    // The code hashes agree with an independent decompressor (3dsrecomp
    // 9b2f217's rom::Title::code), see docs/RESEARCH.md.
    Profile {
        id: "ultra-sun-usa-cart",
        game: Game::UltraSun,
        program_id: 0x0004_0000_001B_5000,
        release: "USA cartridge",
        product_code: "CTR-P-A2AA",
        region: Region::Usa,
        ncch_version: 2,
        code_sha256: "b5388f7500d91be01499a99ca007c98212068608ed7c83c43952e1d5148e9e09",
        code_size: 5_914_624,
    },
    Profile {
        id: "ultra-moon-usa-cart",
        game: Game::UltraMoon,
        program_id: 0x0004_0000_001B_5100,
        release: "USA cartridge",
        product_code: "CTR-P-A2BA",
        region: Region::Usa,
        ncch_version: 2,
        code_sha256: "0aa12056d403a7d747d9c299e1dfbd3c0d5e4ce147675bc8300a8d61e999919b",
        code_size: 5_914_624,
    },
];

/// How a dump relates to the recorded profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Match {
    /// Exactly a recorded executable.
    Profile { profile: Profile },
    /// One of the two games, but code no profile records: another region
    /// or revision, or modified. Not supported until it gets a profile.
    UnknownRevision { game: Game },
    /// Not Ultra Sun or Ultra Moon.
    OtherTitle,
}

pub fn find(program_id: u64, code_sha256: &str) -> Match {
    let Some(&(_, game)) = TITLES.iter().find(|(id, _)| *id == program_id) else {
        return Match::OtherTitle;
    };
    match PROFILES.iter().find(|p| p.program_id == program_id && p.code_sha256 == code_sha256) {
        Some(profile) => Match::Profile { profile: *profile },
        None => Match::UnknownRevision { game },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_are_consistent() {
        for (i, p) in PROFILES.iter().enumerate() {
            assert!(TITLES.contains(&(p.program_id, p.game)), "{}", p.id);
            assert_eq!(p.code_sha256.len(), 64);
            assert!(p.code_sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "lowercase hex");
            assert!(PROFILES[i + 1..].iter().all(|q| q.id != p.id && q.code_sha256 != p.code_sha256));
        }
    }

    #[test]
    fn matching_needs_both_id_and_hash() {
        let us = &PROFILES[0];
        assert_eq!(find(us.program_id, us.code_sha256), Match::Profile { profile: *us });
        assert_eq!(find(us.program_id, &"0".repeat(64)), Match::UnknownRevision { game: Game::UltraSun });
        // Ultra Moon's code under Ultra Sun's ID is not a profile
        assert_eq!(find(us.program_id, PROFILES[1].code_sha256), Match::UnknownRevision { game: Game::UltraSun });
        assert_eq!(find(0x0004_0000_0016_4800, us.code_sha256), Match::OtherTitle);
    }

    #[test]
    fn games_have_distinct_slugs() {
        assert_ne!(Game::UltraSun.slug(), Game::UltraMoon.slug());
    }
}
