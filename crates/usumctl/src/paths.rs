//! Where the project will keep what it derives from a user's dump.
//!
//! Everything goes under the XDG base directories. Inside a Flatpak these
//! resolve to the app's private ~/.var/app/<app id>/ directories, so the
//! same code needs no home-directory access there. Original dumps are only
//! ever read, never copied or modified.

use std::path::PathBuf;

const APP_DIR: &str = "usum-recomp";

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        // the XDG spec says relative values are invalid and must be ignored
        if path.is_absolute() {
            return Some(path);
        }
    }
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|home| PathBuf::from(home).join(fallback))
}

/// Generated code and build output: deletable, rebuildable from the dump.
pub fn cache_dir() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache").map(|p| p.join(APP_DIR))
}

/// Saves: not rebuildable, kept apart from everything generated.
pub fn data_dir() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share").map(|p| p.join(APP_DIR))
}

pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config").map(|p| p.join(APP_DIR))
}

/// The nearest ancestor of `path` that is the top of a git work tree, if
/// any. Code generated from a dump must never land in one, where it could
/// be committed by accident.
pub fn enclosing_git_tree(path: &std::path::Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_owned());
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_an_enclosing_git_tree() {
        let root = std::env::temp_dir().join(format!("usumctl-paths-{}", std::process::id()));
        let nested = root.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_ne!(enclosing_git_tree(&nested), Some(root.clone()), "no .git yet");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(enclosing_git_tree(&nested), Some(root.clone()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
