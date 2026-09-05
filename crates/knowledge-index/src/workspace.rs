//! Workspace resolution from a starting directory.
//!
//! A client that registers the MCP server once, globally, cannot pass a
//! per-repo manifest path; the server instead identifies the workspace from
//! its working directory: the nearest ancestor directory containing a
//! `Cargo.toml`. The walk is lexical — paths are used exactly as given,
//! never canonicalized — so identity is stable for symlinked checkouts.
//! Cargo metadata stays the authority for what a found manifest resolves
//! to: a member manifest still resolves to its owning workspace.

use std::path::{Path, PathBuf};

/// The manifest file that marks a directory as part of a cargo workspace.
const MANIFEST_FILE: &str = "Cargo.toml";

/// Finds the nearest `Cargo.toml` at or above `start_dir`.
///
/// Walks up over `start_dir` and its ancestors as given (no
/// canonicalization; `is_file()` follows symlinks) and returns the first
/// manifest that exists. The result is the manifest path, not the directory,
/// so callers can hand it straight to cargo metadata.
///
/// A found manifest may belong to a workspace *member* rather than the
/// workspace root; cargo metadata resolves it to the owning workspace,
/// which is exactly the semantics this inference relies on.
pub fn nearest_manifest(start_dir: &Path) -> Option<PathBuf> {
    start_dir
        .ancestors()
        .map(|dir| dir.join(MANIFEST_FILE))
        .find(|manifest| manifest.is_file())
}

#[cfg(test)]
mod tests {
    use super::nearest_manifest;

    #[test]
    fn finds_manifest_in_start_dir() {
        let root = tempfile::tempdir().expect("tempdir");
        let manifest = root.path().join("Cargo.toml");
        std::fs::write(&manifest, "[workspace]").expect("write manifest");
        assert_eq!(nearest_manifest(root.path()), Some(manifest));
    }

    #[test]
    fn finds_manifest_from_deep_subdirectory() {
        let root = tempfile::tempdir().expect("tempdir");
        let manifest = root.path().join("Cargo.toml");
        std::fs::write(&manifest, "[workspace]").expect("write manifest");
        let deep = root.path().join("crates").join("demo").join("src");
        assert_eq!(nearest_manifest(&deep), Some(manifest));
    }

    #[test]
    fn none_found_without_manifest_anywhere_above() {
        // A tempdir has no Cargo.toml at or above it (the OS temp tree is
        // not a cargo workspace), so the walk exhausts the ancestors.
        let start = tempfile::tempdir().expect("tempdir");
        assert_eq!(nearest_manifest(start.path()), None);
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_start_dir_keeps_lexical_identity() {
        let parent = tempfile::tempdir().expect("tempdir");
        let real = parent.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");
        std::fs::write(real.join("Cargo.toml"), "[workspace]").expect("manifest");
        let link = parent.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        // The walk must report the manifest through the symlink as given,
        // not the canonicalized target: identity must not depend on how the
        // checkout was reached.
        assert_eq!(
            nearest_manifest(&link),
            Some(link.join("Cargo.toml")),
            "walk-up must not canonicalize away the symlinked path"
        );
        assert_eq!(
            nearest_manifest(&link),
            Some(parent.path().join("link").join("Cargo.toml"))
        );
    }
}
