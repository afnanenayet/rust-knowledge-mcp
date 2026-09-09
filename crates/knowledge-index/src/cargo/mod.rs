//! The resolved Cargo package universe.
//!
//! cargo metadata (via the maintained cargo_metadata crate) is the
//! authoritative representation of the dependency universe: every package is
//! identified by Cargo's opaque PackageId, located by its manifest path, and
//! classified by source. Nothing in this prototype ever walks the registry
//! directory by hand.

pub mod discovery;

pub use discovery::{CachedCargo, resolve};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, Package, PackageId};
use knowledge_core::PackageIdentity;
use sha2::{Digest, Sha256};
use tracing::{info, info_span};

use crate::error::IndexError;

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
    /// Workspace member ids (metadata.workspace_members).
    member_ids: Vec<PackageId>,
}

impl CargoUniverse {
    /// Runs cargo metadata for the given manifest (or the current
    /// directory's ancestor workspace when manifest_path is None).
    pub fn load(manifest_path: Option<&Path>) -> Result<Self, IndexError> {
        let span = info_span!("cargo_metadata");
        let _enter = span.enter();

        let cargo = resolve(None);
        let mut cmd = MetadataCommand::new();
        // The shared resolver picks the cargo binary (see `discovery`);
        // metadata never requests a toolchain, so the resolved invocation
        // carries no pre-arguments and cargo_path captures the full choice.
        // The invocation's PATH prepend (rustup tier) is irrelevant here:
        // `cargo metadata` never invokes rustc, and MetadataCommand has no
        // way to inject a child environment.
        cmd.cargo_path(&cargo.resolved().program);
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

    pub fn package_count(&self) -> usize {
        self.metadata.packages.len()
    }

    /// Workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        self.metadata.workspace_root.as_std_path()
    }

    /// Workspace target directory.
    pub fn target_directory(&self) -> &Path {
        self.metadata.target_directory.as_std_path()
    }

    /// Looks a package up by Cargo's opaque id.
    pub fn get(&self, id: &PackageId) -> Option<&Package> {
        self.by_id.get(id).and_then(|&i| self.metadata.packages.get(i))
    }

    /// Deterministically locates a package by name@version or bare name.
    ///
    /// Bare names must resolve uniquely; ambiguous names (two versions of
    /// the same crate in one graph) require the version suffix. This is the
    /// lookup contract the whole system uses: no name-based guessing beyond
    /// this point.
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
            1 => Ok(matches.into_iter().next().expect("exactly one match")),
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

    pub fn is_workspace_member(&self, id: &PackageId) -> bool {
        self.member_ids.contains(id)
    }

    /// Classifies how the package entered the graph.
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
    pub fn identity(&self, pkg: &Package) -> PackageIdentity {
        identity_from_package(pkg)
    }

    /// Resolved-graph node for a package: direct dependencies (with rename
    /// info) and enabled features, or None when cargo ran with --no-deps.
    pub fn node(&self, id: &PackageId) -> Option<&cargo_metadata::Node> {
        self.metadata
            .resolve
            .as_ref()?
            .nodes
            .iter()
            .find(|n| &n.id == id)
    }

    /// Enabled features of a package in this graph (resolved view).
    pub fn enabled_features(&self, id: &PackageId) -> Vec<String> {
        self.node(id)
            .map(|n| n.features.iter().map(|f| f.to_string()).collect())
            .unwrap_or_default()
    }

    /// SHA-256 of the workspace Cargo.lock, for index metadata.
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
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.metadata.workspace_root.as_str().as_bytes());
        let mut ids: Vec<&str> = self.packages().map(|p| p.id.repr.as_str()).collect();
        ids.sort();
        for id in ids {
            hasher.update(id.as_bytes());
            hasher.update([0x1f]);
        }
        let digest = hasher.finalize();
        hex(digest.get(..16).expect("sha256 digest is 32 bytes"))
    }

    /// Path of the workspace Cargo.lock, if present.
    pub fn lock_path(&self) -> PathBuf {
        self.metadata
            .workspace_root
            .join("Cargo.lock")
            .into_std_path_buf()
    }
}

/// Converts a cargo_metadata package into the normalized identity.
pub fn identity_from_package(pkg: &Package) -> PackageIdentity {
    PackageIdentity {
        package_id: pkg.id.repr.clone(),
        name: pkg.name.to_string(),
        version: pkg.version.to_string(),
        source: pkg.source.as_ref().map(|s| s.repr.clone()),
        manifest_path: pkg.manifest_path.clone().into_std_path_buf(),
    }
}

fn hash_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex(hasher.finalize().get(..16).expect("sha256 digest is 32 bytes"))
        }
        Err(_) => String::new(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
