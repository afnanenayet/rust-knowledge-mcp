//! Pins the concurrent-servers guarantee: multiple MCP sessions may serve
//! the same workspace index at the same time, which only holds because the
//! open path is readers-only (`Index::open_in_dir` + `index.reader()`; a
//! tantivy writer lock exists only under `index.writer()`). Two
//! retrievers over one index directory must both open and search while
//! both are alive.

use std::path::{Path, PathBuf};

use knowledge_core::{KnowledgeRetriever, SearchQuery};
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;
use knowledge_index::store::IndexMeta;
use knowledge_index::tantivy_index::{TantivyRetriever, build_index};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

/// Builds the fixture index into a temp dir whose path is kept for the
/// test's lifetime (the dir itself is never cleaned up).
fn build_index_dir() -> PathBuf {
    let universe =
        CargoUniverse::load(Some(&fixture_dir().join("Cargo.toml"))).expect("cargo metadata");
    let provider = PrebuiltRustdocProvider {
        dir: fixture_dir().join("prebuilt-rustdoc"),
    };
    let (documents, report) = build_corpus(
        &universe,
        &provider,
        &CorpusOptions {
            rustdoc_scope: RustdocScope::All,
        },
    )
    .expect("corpus");
    let dir = tempfile::tempdir().expect("tempdir");
    let meta = IndexMeta {
        schema_version: IndexMeta::supported_schema(),
        workspace_root: universe.workspace_root().to_path_buf(),
        lock_hash: universe.lock_hash(),
        metadata_fingerprint: universe.fingerprint(),
        cargo_version: report.cargo_version.clone(),
        toolchain: None,
        rustdoc_format_version: report.rustdoc_format_version,
        rustdoc_scope: "All".into(),
        package_count: report.packages,
        document_count: documents.len(),
        built_at: "test".into(),
        skipped: Vec::new(),
        warnings: Vec::new(),
    };
    build_index(dir.path(), &documents, &meta).expect("build index");
    let path = dir.path().to_path_buf();
    #[expect(clippy::mem_forget, reason = "leak the TempDir for the test's lifetime")]
    std::mem::forget(dir);
    path
}

fn hit_count(retriever: &TantivyRetriever, query: &str) -> usize {
    retriever
        .search(&SearchQuery::new(query))
        .expect("search succeeds")
        .len()
}

#[test]
fn two_servers_can_open_the_same_index_concurrently() {
    let dir = build_index_dir();
    let first = TantivyRetriever::open(&dir).expect("first open");
    // The pin: this must succeed while the first retriever is still open.
    let second = TantivyRetriever::open(&dir).expect("second open while first is alive");

    let first_hits = hit_count(&first, "write_all");
    let second_hits = hit_count(&second, "write_all");
    assert!(first_hits > 0, "first reader must search");
    assert_eq!(
        second_hits,
        first_hits,
        "both concurrent readers must see the same corpus"
    );
}
