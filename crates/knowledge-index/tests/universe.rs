//! Unit tests for the Cargo universe model, using a committed, real
//! `cargo metadata` blob captured from the fixture workspace
//! (fixtures/demo-workspace). Parsing this JSON exercises exactly the code
//! path used in production without spawning cargo.

use knowledge_index::CargoUniverse;
use knowledge_index::cargo::Origin;

fn universe() -> CargoUniverse {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/demo_metadata.json");
    let json = std::fs::read_to_string(path).expect("test data present");
    CargoUniverse::from_metadata_json(&json).expect("parse metadata")
}

#[test]
fn discovers_the_full_universe() {
    let u = universe();
    assert_eq!(u.package_count(), 6);
    assert_eq!(u.workspace_members().count(), 2);
    let names: Vec<String> = u.packages().map(|p| p.name.to_string()).collect();
    for expected in ["demo-app", "demo-tools", "demo-core", "anyhow", "base64"] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}");
    }
}

#[test]
fn two_versions_of_the_same_crate_are_distinct_identities() {
    let u = universe();
    let versions = u.versions_of("base64");
    assert_eq!(versions.len(), 2, "fixture must resolve base64 twice");

    // Bare names are ambiguous and must be rejected with a helpful error.
    let err = u.resolve_spec("base64").expect_err("ambiguous");
    assert!(err.to_string().contains("ambiguous"), "got: {err}");

    // Fully specified specs are deterministic.
    let v21 = u
        .resolve_spec("base64@0.21.7")
        .expect("exact spec resolves");
    let v22 = u
        .resolve_spec("base64@0.22.1")
        .expect("exact spec resolves");
    assert_ne!(v21.id, v22.id, "distinct PackageIds, never keyed by name");

    let a = u.identity(v21);
    let b = u.identity(v22);
    assert_eq!(a.name, "base64");
    assert_eq!(a.version, "0.21.7");
    assert_eq!(b.version, "0.22.1");
    assert_ne!(a.package_id, b.package_id);
}

#[test]
fn classifies_origins() {
    let u = universe();
    let demo_app = u.resolve_spec("demo-app").unwrap();
    let demo_core = u.resolve_spec("demo-core").unwrap();
    let base64 = u.resolve_spec("base64@0.22.1").unwrap();
    let anyhow = u.resolve_spec("anyhow").unwrap();
    assert_eq!(u.origin(demo_app), Origin::Workspace);
    assert_eq!(
        u.origin(demo_core),
        Origin::Path,
        "demo-core is not a member"
    );
    assert_eq!(u.origin(base64), Origin::Registry);
    assert_eq!(u.origin(anyhow), Origin::Registry);
}

/// The committed metadata blob carries absolute paths captured on the machine
/// that recorded it (a foreign `$HOME` and project layout), so identities built
/// from it cannot be checked against this filesystem — that on-disk sweep runs
/// against live `cargo metadata` in tests/fixture.rs instead. What a blob can
/// pin is the derivation: the identity must carry over the manifest path cargo
/// reported, and its root must be that manifest's parent directory.
#[test]
fn identity_preserves_the_reported_manifest_location() {
    let u = universe();
    for pkg in u.packages() {
        let identity = u.identity(pkg);
        assert_eq!(
            identity.manifest_path,
            pkg.manifest_path.as_std_path(),
            "manifest path for {} must match what cargo reported",
            identity.display()
        );
        assert_eq!(
            identity.root(),
            identity.manifest_path.parent().expect("manifest has a parent"),
            "package root for {} must be the manifest's directory",
            identity.display()
        );
    }
}

#[test]
fn package_id_is_cargo_opaque_id() {
    let u = universe();
    let base64 = u.resolve_spec("base64@0.21.7").unwrap();
    let identity = u.identity(base64);
    assert!(identity.package_id.contains("base64@0.21.7"));
    assert!(identity.source.as_deref().unwrap().starts_with("registry+"));
}

#[test]
fn fingerprint_is_stable_across_parses() {
    let a = universe();
    let b = universe();
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert!(!a.fingerprint().is_empty());
}
