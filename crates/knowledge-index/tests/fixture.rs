//! End-to-end integration tests against the committed fixture workspace
//! (fixtures/demo-workspace). These run the real `cargo metadata`
//! command, the same one the CLI runs in production.
//!
//! Acceptance (Stage 1): given a package spec, we deterministically locate
//! its Cargo package — no `find`, no `grep`, no registry traversal.

use std::path::{Path, PathBuf};

use knowledge_index::CargoUniverse;
use knowledge_index::cargo::Origin;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

fn load_universe() -> CargoUniverse {
    let manifest = fixture_dir().join("Cargo.toml");
    CargoUniverse::load(Some(&manifest)).expect("cargo metadata on fixture")
}

#[test]
fn loads_the_fixture_universe() {
    let u = load_universe();
    assert_eq!(u.package_count(), 6);
    assert_eq!(u.workspace_members().count(), 2);
    assert_eq!(
        u.versions_of("base64").len(),
        2,
        "fixture graph must contain two base64 versions"
    );
}

#[test]
fn locates_packages_deterministically() {
    let u = load_universe();
    // A workspace member.
    let app = u.resolve_spec("demo-app").unwrap();
    // A path dependency that is not a workspace member.
    let core = u.resolve_spec("demo-core").unwrap();
    assert_eq!(u.origin(app), Origin::Workspace);
    assert_eq!(u.origin(core), Origin::Path);

    // The acceptance criterion: from identity to real source location.
    let lib = u.identity(core).root().join("src/lib.rs");
    assert!(
        lib.is_file(),
        "demo-core lib must exist at {}",
        lib.display()
    );

    // And a registry package, located by manifest path, without any registry
    // directory traversal.
    let base64 = u.resolve_spec("base64@0.22.1").unwrap();
    let manifest = &u.identity(base64).manifest_path;
    assert!(
        manifest.is_file(),
        "registry manifest must exist at {}",
        manifest.display()
    );
    assert!(
        manifest.starts_with(home_cargo_registry()),
        "expected the registry checkout cargo metadata reports"
    );
}

fn home_cargo_registry() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap()).join(".cargo/registry")
}

#[test]
fn enabled_features_come_from_the_resolve_graph() {
    let u = load_universe();
    let anyhow = u.resolve_spec("anyhow").unwrap();
    assert!(
        u.enabled_features(&anyhow.id).contains(&"std".to_string()),
        "anyhow default features should include std"
    );
}
