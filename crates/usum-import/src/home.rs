//! The one folder the project writes to.
//!
//! Everything derived from a user's dump or play (generated translations,
//! recordings, saves) lives under a single root: `$USUM_HOME` if set to an
//! absolute path, else `data/` in the project checkout these tools were built
//! from. That folder is ignored by git (`/data/` in `.gitignore`), so nothing
//! in it can be committed by accident. Original dumps are only ever read.

use std::path::PathBuf;

/// Overrides the root, for example inside a Flatpak.
pub const ENV: &str = "USUM_HOME";

/// The root folder.
pub fn root() -> PathBuf {
    if let Some(value) = std::env::var_os(ENV).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return path;
        }
    }
    // crates/usum-import -> the checkout's data/
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().and_then(|crates| crates.parent()).unwrap_or(manifest).join("data")
}

/// Generated workspaces, one per version profile: deletable, rebuildable.
pub fn translations() -> PathBuf {
    root().join("translations")
}

/// `play` sessions: input scripts and the saves they started from.
pub fn recordings() -> PathBuf {
    root().join("recordings")
}

/// Game saves, by program ID: not rebuildable.
pub fn saves() -> PathBuf {
    root().join("saves")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_checkouts_data_folder() {
        if std::env::var_os(ENV).is_none() {
            let root = root();
            assert!(root.ends_with("data"));
            assert!(root.parent().unwrap().join("Cargo.toml").exists(), "{}", root.display());
        }
        assert!(translations().starts_with(root()) && saves().starts_with(root()) && recordings().starts_with(root()));
    }
}
