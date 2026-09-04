//! Stage 2 acceptance: for a resolved crate, enumerate its natural-language
//! docs and structured documented API items with package/version provenance.
//!
//! Uses the committed fixture workspace with its prebuilt rustdoc JSON
//! artifacts (generated with the pinned nightly that emitted format
//! version 61), so the test is stable and needs no toolchain at test time.

use std::path::{Path, PathBuf};

use knowledge_core::{KnowledgeDocument, SourceKind};
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, CorpusReport, RustdocScope, build_corpus};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

fn build() -> (CargoUniverse, Vec<KnowledgeDocument>, CorpusReport) {
    let universe = CargoUniverse::load(Some(&fixture_dir().join("Cargo.toml")))
        .expect("cargo metadata on fixture");
    let provider = PrebuiltRustdocProvider {
        dir: fixture_dir().join("prebuilt-rustdoc"),
    };
    let (docs, report) = build_corpus(
        &universe,
        &provider,
        &CorpusOptions {
            rustdoc_scope: RustdocScope::All,
        },
    )
    .expect("corpus build");
    (universe, docs, report)
}

fn find(
    docs: &[KnowledgeDocument],
    pred: impl Fn(&KnowledgeDocument) -> bool,
) -> KnowledgeDocument {
    docs.iter()
        .find(|d| pred(d))
        .cloned()
        .unwrap_or_else(|| panic!("no document matched"))
}

#[test]
fn rustdoc_items_carry_symbol_provenance() {
    let (_universe, docs, _report) = build();
    let doc = find(&docs, |d| {
        d.symbol_path.as_deref() == Some("demo_core::runtime::offload_blocking")
    });

    assert_eq!(doc.source_kind, SourceKind::RustdocItem);
    assert_eq!(doc.item_kind.as_deref(), Some("function"));
    assert_eq!(doc.package.name, "demo-core");
    assert_eq!(doc.package.version, "0.1.0");
    assert!(
        doc.signature
            .as_deref()
            .unwrap()
            .starts_with("pub fn offload_blocking<F, R>(work: F) -> R"),
        "signature: {:?}",
        doc.signature
    );
    assert!(doc.text.contains("Runs the provided closure"));
    let span = doc.source_span.expect("span present");
    let path = doc.source_path.as_ref().expect("source path");
    assert!(path.ends_with("crates/demo-core/src/lib.rs"), "{path:?}");
    assert_eq!(span.start_line, 41, "span covers the documented item");
}

#[test]
fn crate_and_module_docs_become_rustdoc_modules() {
    let (_universe, docs, _report) = build();
    let root = find(&docs, |d| d.symbol_path.as_deref() == Some("demo_core"));
    assert_eq!(root.source_kind, SourceKind::RustdocModule);
    assert!(root.text.contains("miniature async runtime library"));

    let runtime = find(&docs, |d| {
        d.symbol_path.as_deref() == Some("demo_core::runtime")
    });
    assert_eq!(runtime.source_kind, SourceKind::RustdocModule);
    assert!(runtime.text.contains("cooperative task runtime"));
}

#[test]
fn impl_methods_and_trait_items_are_indexed() {
    let (_universe, docs, _report) = build();
    // Inherent impl method, reachable with its full path.
    let write_all = find(&docs, |d| {
        d.symbol_path.as_deref() == Some("demo_core::writer::Writer::write_all")
    });
    assert_eq!(write_all.item_kind.as_deref(), Some("function"));
    assert!(write_all.text.contains("never fails"));

    // Trait method with its natural path.
    find(&docs, |d| {
        d.symbol_path.as_deref() == Some("demo_core::writer::AsyncWriter::write_async")
    });

    // Implementation of a local trait for a local type.
    let batching = find(&docs, |d| {
        d.symbol_path.as_deref() == Some("demo_core::writer::BatchingWriter::write_async")
    });
    assert_eq!(batching.item_kind.as_deref(), Some("function"));
}

#[test]
fn foreign_trait_impls_are_not_indexed() {
    let (_universe, docs, _report) = build();
    // Debug/From/TryFrom impls on local types are compiler noise.
    let noise = [
        "demo_core::writer::Writer::fmt",
        "demo_core::writer::Writer::borrow",
        "demo_core::writer::Writer::from",
        "demo_core::writer::Writer::try_into",
    ];
    for path in noise {
        assert!(
            docs.iter().all(|d| d.symbol_path.as_deref() != Some(path)),
            "{path} should not be indexed"
        );
    }
}

#[test]
fn markdown_chunks_retain_heading_ancestry() {
    let (_universe, docs, _report) = build();
    let section = find(&docs, |d| {
        d.source_kind == SourceKind::CrateReadme
            && d.section_path == vec!["demo-core", "Runtime", "CPU-bound work"]
    });
    assert!(section.text.contains("offload"));
    assert!(section.source_path.as_ref().unwrap().ends_with("README.md"));

    let guide = find(&docs, |d| {
        d.source_kind == SourceKind::MarkdownDocument && d.title == "When to use Writer"
    });
    assert_eq!(
        guide.section_path,
        vec!["demo-core guide", "Choosing a writer", "When to use Writer"]
    );
    assert!(
        guide
            .source_path
            .as_ref()
            .unwrap()
            .ends_with("docs/guide.md")
    );
}

#[test]
fn package_provenance_separates_crate_versions() {
    let (_universe, docs, _report) = build();
    let v21: Vec<_> = docs
        .iter()
        .filter(|d| d.package.name == "base64" && d.package.version == "0.21.7")
        .collect();
    let v22: Vec<_> = docs
        .iter()
        .filter(|d| d.package.name == "base64" && d.package.version == "0.22.1")
        .collect();
    assert!(
        !v21.is_empty() && !v22.is_empty(),
        "both versions documented"
    );

    // Never merged: identities differ even where content overlaps.
    let ids21: std::collections::HashSet<_> = v21.iter().map(|d| d.id.clone()).collect();
    for d in &v22 {
        assert!(
            !ids21.contains(&d.id),
            "version-specific ids must never collide"
        );
    }
    assert_ne!(v21[0].package.package_id, v22[0].package.package_id);
}

#[test]
fn report_records_provenance() {
    let (_universe, _docs, report) = build();
    assert_eq!(
        report.rustdoc_format_version,
        Some(rustdoc_types::FORMAT_VERSION)
    );
    assert!(report.rustdoc_packages >= 6);
    assert!(report.markdown_files >= 8);
    assert!(
        report.skipped.is_empty(),
        "prebuilt covers everything: {:?}",
        report.skipped
    );
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
}

#[test]
fn document_ids_are_deterministic_across_rebuilds() {
    let (_u1, docs1, _r1) = build();
    let (_u2, docs2, _r2) = build();
    let ids1: Vec<_> = docs1.iter().map(|d| d.id.to_string()).collect();
    let ids2: Vec<_> = docs2.iter().map(|d| d.id.to_string()).collect();
    assert_eq!(ids1, ids2, "rebuilding the same corpus reproduces ids");
}
