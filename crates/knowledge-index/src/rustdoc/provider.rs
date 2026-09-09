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
    /// Cargo binary resolved once at construction via
    /// [`crate::cargo::resolve_cargo`] (explicit `--cargo` beats
    /// $`RUST_KNOWLEDGE_CARGO`, which beats cargo on $PATH); None means
    /// plain `cargo` from $PATH.
    cargo: Option<String>,
}

impl GeneratedRustdocProvider {
    #[must_use]
    pub fn new(
        universe: &CargoUniverse,
        artifact_dir: PathBuf,
        toolchain: Option<String>,
        cargo: Option<&Path>,
    ) -> Self {
        Self::new_with_env(
            universe,
            artifact_dir,
            toolchain,
            cargo,
            crate::cargo::cargo_env_value(),
        )
    }

    /// [new] with the [`crate::cargo::CARGO_ENV_VAR`] value supplied by the
    /// caller. The process environment cannot be swapped in-process (and
    /// mutating it would race other tests), so this is the seam that lets
    /// tests pin the constructor's resolution — the same
    /// [`crate::cargo::resolve_cargo`] precedence the metadata spawn
    /// applies: explicit `--cargo` over the env var over $PATH.
    fn new_with_env(
        universe: &CargoUniverse,
        artifact_dir: PathBuf,
        toolchain: Option<String>,
        cargo: Option<&Path>,
        env_cargo: Option<String>,
    ) -> Self {
        GeneratedRustdocProvider {
            manifest_path: universe.workspace_root().join("Cargo.toml"),
            workspace_root: universe.workspace_root().to_path_buf(),
            target_directory: universe.target_directory().to_path_buf(),
            artifact_dir,
            toolchain,
            cargo: crate::cargo::resolve_cargo(cargo, env_cargo),
        }
    }

    /// The cargo invocation prefix. An explicit cargo binary (resolved by
    /// the constructor from --cargo or $`RUST_KNOWLEDGE_CARGO`) replaces
    /// the PATH lookup (and suppresses the +toolchain argument: the
    /// caller controls the toolchain, including the rustdoc on PATH).
    fn cargo_argv(&self) -> Vec<String> {
        match &self.cargo {
            Some(cargo) => vec![cargo.clone()],
            None => match &self.toolchain {
                Some(t) => vec!["cargo".into(), format!("+{t}")],
                None => vec!["cargo".into()],
            },
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
    #[must_use]
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cargo_metadata::Package;

    use super::{GeneratedRustdocProvider, RustdocProvider};
    use crate::cargo::CargoUniverse;
    use crate::error::IndexError;

    const BAD_CARGO: &str = "/definitely-not-a-cargo-binary-0123456789";

    /// A provider constructed through [GeneratedRustdocProvider::new_with_env]
    /// (the constructor's test seam). The caller owns the artifact-dir
    /// [tempfile::TempDir] so every run gets an isolated, self-cleaning
    /// directory (concurrent runs share nothing); the universe and paths
    /// only need to exist for the spawn, not on disk.
    fn provider(
        artifacts: &tempfile::TempDir,
        cargo: Option<&Path>,
        toolchain: Option<String>,
        env_cargo: Option<String>,
    ) -> GeneratedRustdocProvider {
        let universe = minimal_universe();
        GeneratedRustdocProvider::new_with_env(
            &universe,
            artifacts.path().to_path_buf(),
            toolchain,
            cargo,
            env_cargo,
        )
    }

    #[test]
    fn cargo_argv_uses_the_resolved_binary_and_suppresses_toolchain() {
        let artifacts = tempfile::tempdir().expect("tempdir for rustdoc artifacts");
        let provider = provider(
            &artifacts,
            Some(Path::new("/opt/cargo")),
            Some("nightly".to_string()),
            Some("/from-env".to_string()),
        );
        assert_eq!(
            provider.cargo_argv(),
            vec!["/opt/cargo".to_string()],
            "an explicit cargo binary replaces the PATH lookup and the \
             +toolchain argument (the caller controls the toolchain)"
        );
    }

    #[test]
    fn cargo_argv_applies_the_toolchain_only_for_path_cargo() {
        let artifacts = tempfile::tempdir().expect("tempdir for rustdoc artifacts");
        let with_toolchain = provider(&artifacts, None, Some("nightly".to_string()), None);
        assert_eq!(
            with_toolchain.cargo_argv(),
            vec!["cargo".to_string(), "+nightly".to_string()]
        );
        let plain = provider(&artifacts, None, None, None);
        assert_eq!(plain.cargo_argv(), vec!["cargo".to_string()]);
    }

    /// Pins the constructor's cargo resolution — the wiring
    /// [GeneratedRustdocProvider::new] performs — not just the argv the
    /// resolved value produces: an explicit binary beats the env var,
    /// the env var fills the gap (the same precedence the metadata
    /// spawn applies). A regression in new() (forgetting the env read,
    /// or bypassing resolve_cargo) keeps the spawn tests below green only
    /// if this one stays loud.
    #[test]
    fn constructor_resolves_cargo_explicit_over_env() {
        let artifacts = tempfile::tempdir().expect("tempdir for rustdoc artifacts");
        let explicit = provider(
            &artifacts,
            Some(Path::new("/explicit-cargo")),
            Some("nightly".to_string()),
            Some("/from-env".to_string()),
        );
        assert_eq!(
            explicit.cargo.as_deref(),
            Some("/explicit-cargo"),
            "an explicit --cargo must beat $RUST_KNOWLEDGE_CARGO at construction"
        );
        let via_env = provider(&artifacts, None, None, Some("/from-env".to_string()));
        assert_eq!(
            via_env.cargo.as_deref(),
            Some("/from-env"),
            "the env var must fill the gap when no explicit cargo was given"
        );
    }

    /// Pins the --cargo plumbing on the rustdoc-generation spawn: an
    /// explicit cargo binary must reach the spawn itself (a nonexistent
    /// path fails it) instead of silently falling back to cargo on
    /// $PATH. Constructed through the constructor seam so the flag's
    /// whole path is exercised: resolve_cargo at construction, then the
    /// spawn. Mirrors the metadata-spawn pin in
    /// [crate::pipeline::open_retriever_with].
    #[test]
    fn explicit_cargo_reaches_the_rustdoc_spawn() {
        let universe = minimal_universe();
        let artifacts = tempfile::tempdir().expect("tempdir for rustdoc artifacts");
        let provider = GeneratedRustdocProvider::new_with_env(
            &universe,
            artifacts.path().to_path_buf(),
            Some("nightly".to_string()),
            Some(Path::new(BAD_CARGO)),
            None,
        );
        let pkg = lib_package();
        let error = match provider.generate(&universe, &[&pkg]) {
            Err(error) => error,
            Ok(_) => panic!("a bad cargo path must fail the rustdoc spawn"),
        };
        match error {
            IndexError::RustdocSpawn { command, .. } => assert!(
                command.contains(BAD_CARGO),
                "the spawn error must name the explicit cargo binary, got: {command}"
            ),
            other => panic!("expected a spawn failure, got: {other:?}"),
        }
    }

    fn minimal_universe() -> CargoUniverse {
        let json = r#"{
            "packages": [],
            "workspace_members": [],
            "workspace_root": "/rustdoc-test",
            "target_directory": "/rustdoc-test/target",
            "version": 1
        }"#;
        CargoUniverse::from_metadata_json(json).expect("minimal metadata should parse")
    }

    /// A package with a lib target, so generation gets far enough to
    /// construct and run the cargo rustdoc spawn.
    fn lib_package() -> Package {
        serde_json::from_value(serde_json::json!({
            "name": "demo_lib",
            "version": "0.1.0",
            "id": "path+file:///rustdoc-test#demo_lib@0.1.0",
            "targets": [{
                "kind": ["lib"],
                "crate_types": ["lib"],
                "name": "demo_lib",
                "src_path": "/rustdoc-test/src/lib.rs",
                "edition": "2021",
                "doc": true,
                "doctest": true,
                "test": true
            }],
            "dependencies": [],
            "features": {},
            "manifest_path": "/rustdoc-test/Cargo.toml",
            "edition": "2021",
            "authors": [],
            "categories": [],
            "keywords": []
        }))
        .expect("package literal should parse")
    }
}
