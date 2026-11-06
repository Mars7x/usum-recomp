//! Where generated output may go: under `usum_import::home`, never into a
//! git work tree except the checkout's ignored `data/` folder.

use std::path::PathBuf;

/// The nearest ancestor of `path` that is the top of a git work tree, if
/// any. Code generated from a dump must never land in one, where it could
/// be committed by accident.
pub fn enclosing_git_tree(path: &std::path::Path) -> Option<PathBuf> {
    if path.starts_with(usum_import::home::root()) {
        return None; // the ignored data folder
    }
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
