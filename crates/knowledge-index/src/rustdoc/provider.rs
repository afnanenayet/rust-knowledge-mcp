//! Rustdoc JSON generation, isolated behind a trait so the nightly/unstable
//! command construction does not leak through the codebase.
//!
//! Empirically verified command shape (see docs/design.md):
//! cargo rustdoc -p name@version --lib -- -Zunstable-options --output-format json
//! Artifacts appear at <target-dir>/doc/<crate-name>.json; two versions of
//! the same crate overwrite each other there, so artifacts are copied to a
//! version-named cache location immediately after each generation.

use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_metadata::{Package, TargetKind};
use knowledge_core::PackageIdentity;
use tracing::{info, info_span};

use crate::cargo::CargoUniverse;
use crate::error::IndexError;

/// A rustdoc JSON artifact for one resolved package.
#[derive(Clone, Debug)]
pub struct RustdocArtifact {
    pub package: PackageIdentity,
    pub path: PathBuf,
    /// Output of the cargo version query (e.g. "cargo 1.100.0-nightly (..)").
    pub cargo_version: Option<String>,
}

/// Result of a generation run over a set of packages.
#[derive(Debug, Default)]
pub struct GeneratedRustdocs {
    pub artifacts: Vec<RustdocArtifact>,
    /// (package spec, reason) for every package where generation was
    /// attempted and failed.
    pub skipped: Vec<(String, String)>,
    /// Packages that structurally cannot have rustdoc JSON (e.g. binary-only
    /// crates). Benign: they may still have README documentation.
    pub unsupported: Vec<String>,
}

/// Generates or locates rustdoc JSON artifacts for packages.
pub trait RustdocProvider {
    fn generate(
        &self,
        universe: &CargoUniverse,
        packages: &[&Package],
    ) -> Result<GeneratedRustdocs, IndexError>;
}

/// Generates rustdoc JSON by invoking cargo (nightly toolchain required;
/// rustdoc JSON output is an unstable rustdoc feature).
pub struct GeneratedRustdocProvider {
    manifest_path: PathBuf,
    workspace_root: PathBuf,
    target_directory: PathBuf,
    /// Versioned artifact cache dir, e.g. <index>/cache/rustdoc.
    artifact_dir: PathBuf,
    /// Toolchain passed to cargo (e.g. "nightly"); None = plain cargo.
    toolchain: Option<String>,
    /// Explicit cargo binary resolved by the caller's config layer; None
    /// falls back to $RUST_KNOWLEDGE_CARGO, then to cargo on $PATH.
    cargo: Option<PathBuf>,
}

impl GeneratedRustdocProvider {
    pub fn new(
        universe: &CargoUniverse,
        artifact_dir: PathBuf,
        toolchain: Option<String>,
        cargo: Option<PathBuf>,
    ) -> Self {
        GeneratedRustdocProvider {
            manifest_path: universe.workspace_root().join("Cargo.toml"),
            workspace_root: universe.workspace_root().to_path_buf(),
            target_directory: universe.target_directory().to_path_buf(),
            artifact_dir,
            toolchain,
            cargo,
        }
    }

    /// The cargo invocation prefix. An explicit cargo binary (from the
    /// constructor or $RUST_KNOWLEDGE_CARGO) replaces the PATH lookup (and
    /// suppresses the +toolchain argument: the caller controls the
    /// toolchain, including the rustdoc on PATH).
    fn cargo_argv(&self) -> Vec<String> {
        let explicit = self
            .cargo
            .as_ref()
            .map(|path| path.to_string_lossy().trim().to_string())
            .or_else(|| {
                std::env::var("RUST_KNOWLEDGE_CARGO")
                    .ok()
                    .map(|cargo| cargo.trim().to_string())
            })
            .filter(|explicit| !explicit.is_empty());
        if let Some(explicit) = explicit {
            return vec![explicit];
        }
        match &self.toolchain {
            Some(t) => vec!["cargo".into(), format!("+{t}")],
            None => vec!["cargo".into()],
        }
    }

    fn command(&self, args: &[String]) -> Command {
        let prefix = self.cargo_argv();
        let mut cmd = Command::new(prefix.first().expect("cargo argv is never empty"));
        for arg in prefix.iter().skip(1) {
            cmd.arg(arg);
        }
        for arg in args {
            cmd.arg(arg);
        }
        cmd.current_dir(&self.workspace_root);
        cmd
    }

    fn query_cargo_version(&self) -> Option<String> {
        self.command(&["--version".into()])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }
}

impl RustdocProvider for GeneratedRustdocProvider {
    fn generate(
        &self,
        universe: &CargoUniverse,
        packages: &[&Package],
    ) -> Result<GeneratedRustdocs, IndexError> {
        let span = info_span!("rustdoc_generation");
        let _enter = span.enter();
        std::fs::create_dir_all(&self.artifact_dir)
            .map_err(|e| IndexError::io(&self.artifact_dir, e))?;

        let cargo_version = self.query_cargo_version();
        let mut out = GeneratedRustdocs::default();

        for pkg in packages {
            let identity = universe.identity(pkg);
            let spec = format!("{}@{}", pkg.name, pkg.version);

            let Some(lib_name) = lib_crate_name(pkg) else {
                out.unsupported.push(spec.clone());
                continue;
            };

            let args: Vec<String> = [
                "rustdoc",
                "-p",
                &spec,
                "--lib",
                "--",
                "-Zunstable-options",
                "--output-format",
                "json",
            ]
            .into_iter()
            .map(String::from)
            .collect();
            let display_cmd = format!(
                "{} rustdoc -p {spec} --lib -- -Zunstable-options --output-format json",
                self.cargo_argv().join(" ")
            );
            let output = self
                .command(&args)
                .output()
                .map_err(|e| IndexError::RustdocSpawn {
                    command: display_cmd,
                    cause: e.to_string(),
                })?;
            if !output.status.success() {
                out.skipped.push((
                    spec.clone(),
                    format!(
                        "cargo rustdoc failed ({}); stderr: {}",
                        output.status.code().unwrap_or(-1),
                        stderr_tail(&output.stderr, 4000)
                    ),
                ));
                continue;
            }

            // Move the artifact out of target/doc before the next package:
            // same-name crates at different versions collide there.
            let src = self
                .target_directory
                .join("doc")
                .join(format!("{lib_name}.json"));
            if !src.is_file() {
                out.skipped.push((
                    spec.clone(),
                    format!(
                        "rustdoc JSON not produced at {}; this may be a toolchain without rustdoc JSON support",
                        src.display()
                    ),
                ));
                continue;
            }
            let dest = self
                .artifact_dir
                .join(format!("{}-{}.json", pkg.name, pkg.version));
            std::fs::copy(&src, &dest).map_err(|e| IndexError::io(&dest, e))?;

            info!(package = %spec, artifact = %dest.display(), "generated rustdoc JSON");
            out.artifacts.push(RustdocArtifact {
                package: identity,
                path: dest,
                cargo_version: cargo_version.clone(),
            });
        }

        Ok(out)
    }
}

/// Reads pre-existing rustdoc JSON artifacts named <name>-<version>.json from
/// a directory. Used by tests (stable, no nightly needed) and by users who
/// generate rustdoc JSON with a pinned toolchain out of band.
pub struct PrebuiltRustdocProvider {
    pub dir: PathBuf,
}

impl RustdocProvider for PrebuiltRustdocProvider {
    fn generate(
        &self,
        universe: &CargoUniverse,
        packages: &[&Package],
    ) -> Result<GeneratedRustdocs, IndexError> {
        let mut out = GeneratedRustdocs::default();
        for pkg in packages {
            let identity = universe.identity(pkg);
            let spec = format!("{}@{}", pkg.name, pkg.version);
            let path = self.dir.join(format!("{}-{}.json", pkg.name, pkg.version));
            if path.is_file() {
                out.artifacts.push(RustdocArtifact {
                    package: identity,
                    path,
                    cargo_version: None,
                });
            } else {
                out.skipped
                    .push((spec, format!("no prebuilt artifact at {}", path.display())));
            }
        }
        Ok(out)
    }
}

/// The lib target's crate name (rustdoc output filename), if the package
/// has a library-like target at all.
fn lib_crate_name(pkg: &Package) -> Option<String> {
    const LIB_KINDS: [TargetKind; 6] = [
        TargetKind::Lib,
        TargetKind::RLib,
        TargetKind::DyLib,
        TargetKind::CDyLib,
        TargetKind::StaticLib,
        TargetKind::ProcMacro,
    ];
    pkg.targets
        .iter()
        .find(|t| t.kind.iter().any(|k| LIB_KINDS.contains(k)))
        .map(|t| t.name.clone())
}

fn stderr_tail(stderr: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(stderr);
    if text.len() <= max {
        text.trim().to_string()
    } else {
        let start = text.floor_char_boundary(text.len() - max);
        let tail = text.get(start..).unwrap_or_default().trim();
        format!("...{tail}")
    }
}

// manifest_path is retained for diagnostics in future work; keep the field
// exercised to avoid dead-code warnings.
impl GeneratedRustdocProvider {
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }
}
