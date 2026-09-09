//! The resolved Cargo package universe.
//!
//! cargo metadata (via the maintained `cargo_metadata` crate) is the
//! authoritative representation of the dependency universe: every package is
//! identified by Cargo's opaque `PackageId`, located by its manifest path, and
//! classified by source. Nothing in this prototype ever walks the registry
//! directory by hand.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, Package, PackageId};
use knowledge_core::PackageIdentity;
use sha2::{Digest, Sha256};
use tracing::{info, info_span, warn};

use crate::error::IndexError;

/// The env var that names the cargo binary the engine spawns when the
/// frontends' `--cargo` flag is absent. Mirrors the figue env alias of
/// [`WorkspaceConfig::cargo`](crate::config::WorkspaceConfig::cargo), which
/// sits above it in the layering.
pub const CARGO_ENV_VAR: &str = "RUST_KNOWLEDGE_CARGO";

/// Resolves which cargo binary a spawn invokes: an explicit binary (the
/// frontends' `--cargo` flag, already merged with [`CARGO_ENV_VAR`] by
/// figue) beats a raw [`CARGO_ENV_VAR`] from the environment, which beats
/// plain `cargo` on `$PATH`. Returns the binary to invoke, or None for
/// `cargo` from `$PATH`.
///
/// Empty or whitespace-only values are treated as unset, with a warning:
/// silently skipping one would let `--cargo ""` fall to a different
/// binary than the user asked for. The one implementation is shared by
/// the cargo-metadata spawn and the rustdoc-generation spawn so both
/// resolve identically — the duplicated inline versions used to disagree
/// for `--cargo ""` (one fell through to the env var, the other to
/// `$PATH`, so metadata and rustdoc could run under different binaries).
///
/// The resolved binary is made absolute (a relative value resolves
/// against the process's working directory): the rustdoc spawn runs in
/// the workspace root while the metadata spawn runs at the process's
/// directory, so a relative value would otherwise resolve — and
/// possibly diverge — per spawn.
///
/// Resolution is idempotent, so passing an already-resolved value as the
/// `explicit` argument selects the same binary.
pub fn resolve_cargo(explicit: Option<&Path>, env_cargo: Option<String>) -> Option<String> {
    if let Some(path) = explicit {
        let lossy = path.to_string_lossy();
        let trimmed = lossy.trim();
        if !trimmed.is_empty() {
            return Some(absolute_bin(trimmed));
        }
        warn!(
            path = %path.display(),
            "explicit cargo binary (--cargo or ${CARGO_ENV_VAR}) is empty; \
             falling back to ${CARGO_ENV_VAR}, then to cargo on $PATH"
        );
    }
    match env_cargo.map(|cargo| cargo.trim().to_owned()) {
        Some(cargo) if !cargo.is_empty() => Some(absolute_bin(&cargo)),
        Some(_) => {
            warn!("${CARGO_ENV_VAR} is empty; using cargo from $PATH");
            None
        }
        None => None,
    }
}

/// Makes a configured cargo binary absolute: both spawn paths must invoke
/// the same file, but the rustdoc spawn changes its working directory to
/// the workspace root while the metadata spawn runs at the process's,
/// so a relative value would resolve differently per spawn. Fails
/// soft (returns the input) if the path cannot be made absolute.
fn absolute_bin(cargo: &str) -> String {
    std::path::absolute(cargo)
        .map_or_else(|_| cargo.to_owned(), |p| p.to_string_lossy().into_owned())
}

/// Reads [`CARGO_ENV_VAR`] the way [`resolve_cargo`] expects it. figue's env
/// layer has already merged the variable into the frontends' `--cargo`
/// flag when it applies, so this read serves callers below that layer
/// (and is idempotent under it). A value that is not valid UTF-8 cannot
/// become a CLI-layer string, so it warns and yields None — the same
/// loud fall-to-$PATH contract [`resolve_cargo`] applies to an empty
/// value, instead of the silent one `std::env::var(...).ok()` gives
/// (its `NotUnicode` error would vanish).
pub(crate) fn cargo_env_value() -> Option<String> {
    match std::env::var_os(CARGO_ENV_VAR) {
        Some(value) => {
            if let Some(text) = value.to_str() {
                Some(text.to_owned())
            } else {
                warn!("${CARGO_ENV_VAR} is not valid UTF-8; using cargo from $PATH");
                None
            }
        }
        None => None,
    }
}

/// How a package entered the resolved graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A workspace member.
    Workspace,
    /// A path dependency that is not a workspace member.
    Path,
    /// A git dependency.
    Git,
    /// A registry (e.g. crates.io) dependency.
    Registry,
}

impl Origin {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::Workspace => "workspace",
            Origin::Path => "path",
            Origin::Git => "git",
            Origin::Registry => "registry",
        }
    }
}

/// The packages Cargo actually resolved for a workspace.
pub struct CargoUniverse {
    metadata: Metadata,
    /// Cargo's opaque package id to index into metadata.packages.
    by_id: HashMap<PackageId, usize>,
    /// Workspace member ids (`metadata.workspace_members`).
    member_ids: Vec<PackageId>,
}

impl CargoUniverse {
    /// Runs cargo metadata for the given manifest (or the current
    /// directory's ancestor workspace when `manifest_path` is None).
    ///
    /// # Errors
    ///
    /// Returns an error when Cargo cannot resolve workspace metadata.
    pub fn load(manifest_path: Option<&Path>) -> Result<Self, IndexError> {
        Self::load_with(manifest_path, None)
    }

    /// Like [load](Self::load), with an explicit cargo binary. A None
    /// cargo falls back to $`RUST_KNOWLEDGE_CARGO`, then to cargo on $PATH.
    ///
    /// # Errors
    ///
    /// Returns an error when the selected Cargo binary cannot resolve
    /// workspace metadata.
    pub fn load_with(
        manifest_path: Option<&Path>,
        cargo: Option<&Path>,
    ) -> Result<Self, IndexError> {
        let span = info_span!("cargo_metadata");
        let _enter = span.enter();

        let mut cmd = MetadataCommand::new();
        // Escape hatch for environments where cargo is not on PATH. Shared
        // with the rustdoc-generation spawn so both spawn paths resolve
        // the binary identically (explicit beats env beats PATH).
        if let Some(cargo) = resolve_cargo(cargo, cargo_env_value()) {
            cmd.cargo_path(cargo);
        }
        if let Some(path) = manifest_path {
            cmd.manifest_path(path);
        }
        // Resolve the full dependency graph with all features unified.
        cmd.features(CargoOpt::AllFeatures);

        let metadata = cmd.exec().map_err(IndexError::cargo_metadata)?;

        let by_id = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        let member_ids = metadata.workspace_members.clone();

        info!(
            packages = metadata.packages.len(),
            workspace_members = member_ids.len(),
            "resolved cargo universe"
        );

        Ok(CargoUniverse {
            metadata,
            by_id,
            member_ids,
        })
    }

    /// Parses an existing cargo metadata JSON blob (format version 1).
    /// Used by tests to exercise universe logic without spawning cargo.
    ///
    /// # Errors
    ///
    /// Returns an error when `json` is not valid Cargo metadata.
    pub fn from_metadata_json(json: &str) -> Result<Self, IndexError> {
        let metadata: Metadata = serde_json::from_str(json).map_err(IndexError::cargo_metadata)?;
        let by_id = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        let member_ids = metadata.workspace_members.clone();
        Ok(CargoUniverse {
            metadata,
            by_id,
            member_ids,
        })
    }

    /// All resolved packages.
    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.metadata.packages.iter()
    }

    #[must_use]
    pub fn package_count(&self) -> usize {
        self.metadata.packages.len()
    }

    /// Workspace root directory.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        self.metadata.workspace_root.as_std_path()
    }

    /// Workspace target directory.
    #[must_use]
    pub fn target_directory(&self) -> &Path {
        self.metadata.target_directory.as_std_path()
    }

    /// Looks a package up by Cargo's opaque id.
    #[must_use]
    pub fn get(&self, id: &PackageId) -> Option<&Package> {
        self.by_id
            .get(id)
            .and_then(|&i| self.metadata.packages.get(i))
    }

    /// Deterministically locates a package by name@version or bare name.
    ///
    /// Bare names must resolve uniquely; ambiguous names (two versions of
    /// the same crate in one graph) require the version suffix. This is the
    /// lookup contract the whole system uses: no name-based guessing beyond
    /// this point.
    ///
    /// # Errors
    ///
    /// Returns an error when the package is absent or a bare name is
    /// ambiguous.
    pub fn resolve_spec(&self, spec: &str) -> Result<&Package, IndexError> {
        if let Some((name, version)) = spec.split_once('@') {
            return self
                .packages()
                .find(|p| p.name.as_str() == name && p.version.to_string() == version)
                .ok_or_else(|| IndexError::PackageNotFound {
                    spec: spec.to_string(),
                });
        }
        let matches: Vec<&Package> = self
            .packages()
            .filter(|p| p.name.as_str() == spec)
            .collect();
        match matches.len() {
            0 => Err(IndexError::PackageNotFound {
                spec: spec.to_string(),
            }),
            1 => matches
                .into_iter()
                .next()
                .ok_or_else(|| IndexError::PackageNotFound {
                    spec: spec.to_string(),
                }),
            _ => {
                let versions = matches
                    .iter()
                    .map(|p| p.version.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(IndexError::PackageNotFound {
                    spec: format!(
                        "{spec} is ambiguous (versions in graph: {versions}); use name@version"
                    ),
                })
            }
        }
    }

    /// All versions of a package name present in the resolved graph.
    #[must_use]
    pub fn versions_of(&self, name: &str) -> Vec<&Package> {
        let mut found: Vec<&Package> = self
            .packages()
            .filter(|p| p.name.as_str() == name)
            .collect();
        found.sort_by(|a, b| a.version.cmp(&b.version));
        found
    }

    pub fn workspace_members(&self) -> impl Iterator<Item = &Package> {
        self.member_ids.iter().filter_map(|id| self.get(id))
    }

    #[must_use]
    pub fn is_workspace_member(&self, id: &PackageId) -> bool {
        self.member_ids.contains(id)
    }

    /// Classifies how the package entered the graph.
    #[must_use]
    pub fn origin(&self, pkg: &Package) -> Origin {
        if self.is_workspace_member(&pkg.id) {
            return Origin::Workspace;
        }
        match pkg.source.as_ref().map(|s| s.repr.as_str()) {
            None => Origin::Path,
            Some(repr) if repr.starts_with("git+") => Origin::Git,
            Some(_) => Origin::Registry,
        }
    }

    /// The normalized identity of a package.
    #[must_use]
    pub fn identity(&self, pkg: &Package) -> PackageIdentity {
        identity_from_package(pkg)
    }

    /// Resolved-graph node for a package: direct dependencies (with rename
    /// info) and enabled features, or None when cargo ran with --no-deps.
    #[must_use]
    pub fn node(&self, id: &PackageId) -> Option<&cargo_metadata::Node> {
        self.metadata
            .resolve
            .as_ref()?
            .nodes
            .iter()
            .find(|n| &n.id == id)
    }

    /// Enabled features of a package in this graph (resolved view). Empty
    /// when the metadata carries no resolve graph (cargo ran with
    /// --no-deps), which is indistinguishable from zero enabled features.
    #[must_use]
    pub fn enabled_features(&self, id: &PackageId) -> Vec<String> {
        self.node(id)
            .map(|n| {
                n.features
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// SHA-256 of the workspace Cargo.lock, for index metadata.
    #[must_use]
    pub fn lock_hash(&self) -> String {
        hash_file(
            self.metadata
                .workspace_root
                .join("Cargo.lock")
                .as_std_path(),
        )
    }

    /// Stable fingerprint of the resolved universe itself: package ids and
    /// workspace root. Two fingerprints match only if the same universe was
    /// resolved.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.metadata.workspace_root.as_str().as_bytes());
        let mut ids: Vec<&str> = self.packages().map(|p| p.id.repr.as_str()).collect();
        ids.sort_unstable();
        for id in ids {
            hasher.update(id.as_bytes());
            hasher.update([0x1f]);
        }
        let digest = hasher.finalize();
        hex(digest.get(..16).unwrap_or_default())
    }

    /// Path of the workspace Cargo.lock, if present.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.metadata
            .workspace_root
            .join("Cargo.lock")
            .into_std_path_buf()
    }
}

/// Converts a `cargo_metadata` package into the normalized identity.
#[must_use]
pub fn identity_from_package(pkg: &Package) -> PackageIdentity {
    PackageIdentity {
        package_id: pkg.id.repr.clone(),
        name: pkg.name.to_string(),
        version: pkg.version.to_string(),
        source: pkg.source.as_ref().map(|s| s.repr.clone()),
        manifest_path: pkg.manifest_path.clone().into_std_path_buf(),
    }
}

/// SHA-256 of a file (truncated to 16 bytes of hex), or an empty string
/// when the file cannot be read — with a warning, because the empty hash
/// silently degrades any staleness comparison made against it.
fn hash_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex(hasher.finalize().get(..16).unwrap_or_default())
        }
        Err(error) => {
            warn!(
                path = %path.display(),
                error = %error,
                "failed to hash file; staleness checks against this hash are degraded"
            );
            String::new()
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("writing hexadecimal digits to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{CARGO_ENV_VAR, CargoUniverse, resolve_cargo};

    /// An explicit binary beats the env var and $PATH, and is trimmed.
    #[test]
    fn explicit_cargo_beats_env_and_path() {
        assert_eq!(
            resolve_cargo(
                Some(std::path::Path::new("  /explicit/cargo  ")),
                Some("/from-env".to_string())
            ),
            Some("/explicit/cargo".to_string())
        );
    }

    /// The empty-string cases the two duplicated inline implementations
    /// disagreed on: an explicit-but-empty value must fall through to the
    /// env var (and warn), never jump straight to $PATH.
    #[test]
    fn empty_explicit_cargo_falls_through_to_env() {
        for empty in ["", "  \t"] {
            assert_eq!(
                resolve_cargo(
                    Some(std::path::Path::new(empty)),
                    Some("/from-env".to_string())
                ),
                Some("/from-env".to_string()),
                "an empty explicit cargo must defer to ${CARGO_ENV_VAR}"
            );
        }
    }

    /// An empty env value falls to plain cargo on $PATH, as does no value
    /// at all.
    #[test]
    fn empty_or_absent_env_cargo_falls_to_path() {
        assert_eq!(resolve_cargo(None, Some("  ".to_string())), None);
        assert_eq!(resolve_cargo(None, None), None);
    }

    /// The env var fills the gap when no explicit binary was given.
    #[test]
    fn env_cargo_fills_the_gap() {
        assert_eq!(
            resolve_cargo(None, Some("  /from-env  ".to_string())),
            Some("/from-env".to_string())
        );
    }

    /// A relative binary is made absolute against the process cwd, for
    /// both the explicit flag and the env var: the rustdoc spawn runs in
    /// the workspace root while the metadata spawn runs at the process
    /// cwd, so a relative value would otherwise resolve to a different
    /// binary (or none) per spawn.
    #[test]
    fn relative_cargo_is_absolutized() {
        let cwd = std::env::current_dir().expect("the test process has a cwd");
        let expected = cwd.join("cargo-shim").to_string_lossy().into_owned();
        assert_eq!(
            resolve_cargo(Some(std::path::Path::new("cargo-shim")), None),
            Some(expected.clone()),
            "a relative --cargo must resolve against the process cwd"
        );
        assert_eq!(
            resolve_cargo(None, Some("cargo-shim".to_string())),
            Some(expected),
            "a relative ${CARGO_ENV_VAR} must resolve against the process cwd"
        );
    }

    /// A workspace with a Cargo.lock hashes it; one without (or
    /// unreadable) records an empty hash instead of failing the index
    /// build — that contract, and the degradation it implies, is what
    /// [super::CargoUniverse::lock_hash] documents.
    #[test]
    fn lock_hash_is_empty_without_a_lockfile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let universe = universe_rooted_at(dir.path());
        assert_eq!(
            universe.lock_hash(),
            "",
            "a missing Cargo.lock must not fail: the hash is recorded empty"
        );

        std::fs::write(dir.path().join("Cargo.lock"), b"version = 3").expect("write lockfile");
        let hash = universe.lock_hash();
        assert_eq!(
            hash.len(),
            32,
            "a present lockfile hashes to 32 hex chars (16 bytes), got {hash:?}"
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "the lock hash is hex, got {hash:?}"
        );
    }

    fn universe_rooted_at(root: &std::path::Path) -> CargoUniverse {
        let json = format!(
            r#"{{
                "packages": [],
                "workspace_members": [],
                "workspace_root": "{}",
                "target_directory": "{}",
                "version": 1
            }}"#,
            root.display(),
            root.join("target").display()
        );
        CargoUniverse::from_metadata_json(&json).expect("minimal metadata should parse")
    }
}
