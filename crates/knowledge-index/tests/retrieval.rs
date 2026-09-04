//! Stage 3 acceptance: identifier-heavy and conceptual queries over the
//! committed fixture corpus, plus filters, get-by-id round trips, symbol
//! lookup, and snippet truncation.
//!
//! The index is built once from the fixture workspace using prebuilt rustdoc
//! artifacts (no nightly needed), into a fresh temp dir per test run.

use std::path::{Path, PathBuf};

use knowledge_core::{KnowledgeRetriever, SearchQuery, SourceKind, SymbolQuery};
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;
use knowledge_index::store::IndexMeta;
use knowledge_index::tantivy_index::{TantivyRetriever, build_index};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

fn build_temp_index(tag: &str) -> (tempfile::TempDir, TantivyRetriever) {
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
    let retriever = TantivyRetriever::open(dir.path()).expect("open index");
    // Leak the TempDir: tests read it for their whole life.
    std::mem::forget(dir);
    let _ = tag;
    (tempfile::TempDir::new().expect("unused"), retriever)
}

fn search(retriever: &TantivyRetriever, query: &str) -> Vec<knowledge_core::SearchHit> {
    retriever
        .search(&SearchQuery::new(query))
        .expect("search succeeds")
}

#[test]
fn identifier_query_finds_the_symbol() {
    let (_dir, retriever) = build_temp_index("id");
    let hits = search(&retriever, "write_all");
    assert!(!hits.is_empty());
    let top = &hits[0];
    assert_eq!(
        top.symbol_path.as_deref(),
        Some("demo_core::writer::Writer::write_all"),
        "top hit: {} {:?}",
        top.title,
        top.symbol_path
    );
    assert_eq!(top.package_name, "demo-core");
    assert_eq!(top.source_kind, SourceKind::RustdocItem);
}

#[test]
fn conceptual_query_finds_offload_guidance() {
    let (_dir, retriever) = build_temp_index("concept");
    let hits = search(
        &retriever,
        "how should CPU intensive work be handled in the async runtime",
    );
    let top3: Vec<String> = hits
        .iter()
        .take(3)
        .map(|h| {
            h.symbol_path
                .clone()
                .unwrap_or_else(|| h.section_path.join(" > "))
        })
        .collect();
    let relevant = top3.iter().any(|ctx| {
        ctx.contains("offload_blocking")
            || ctx.contains("runtime")
            || ctx.contains("CPU-bound work")
    });
    assert!(
        relevant,
        "expected runtime/offload guidance in top 3, got {top3:?}"
    );
}

#[test]
fn conceptual_query_finds_buffering_guidance() {
    let (_dir, retriever) = build_temp_index("buffer");
    let hits = search(&retriever, "assembling payloads in memory before flushing");
    assert!(hits.len() >= 2, "want a few candidates, got {}", hits.len());
    let top4: Vec<String> = hits
        .iter()
        .take(4)
        .map(|h| {
            h.symbol_path
                .clone()
                .unwrap_or_else(|| h.section_path.join(" > "))
        })
        .collect();
    assert!(
        top4.iter()
            .any(|c| c.contains("Writer") || c.contains("Buffering")),
        "expected writer/buffering docs in top 4, got {top4:?}"
    );
}

#[test]
fn package_filter_restricts_results() {
    let (_dir, retriever) = build_temp_index("filter");
    let query = SearchQuery {
        packages: vec!["base64".into()],
        ..SearchQuery::new("engine encode")
    };
    let hits = retriever.search(&query).expect("search");
    assert!(!hits.is_empty());
    for hit in &hits {
        assert_eq!(hit.package_name, "base64", "filtered results only base64");
    }

    // name@version form picks exactly one resolved version.
    let query = SearchQuery {
        packages: vec!["base64@0.21.7".into()],
        ..SearchQuery::new("engine encode")
    };
    let hits = retriever.search(&query).expect("search");
    assert!(!hits.is_empty());
    for hit in &hits {
        assert_eq!(hit.package_version, "0.21.7");
    }
}

#[test]
fn source_kind_filter_restricts_results() {
    let (_dir, retriever) = build_temp_index("kind-filter");
    let query = SearchQuery {
        source_kinds: vec![SourceKind::CrateReadme],
        ..SearchQuery::new("runtime writer")
    };
    let hits = retriever.search(&query).expect("search");
    assert!(!hits.is_empty());
    for hit in &hits {
        assert_eq!(hit.source_kind, SourceKind::CrateReadme);
    }
}

#[test]
fn item_kind_filter_restricts_results() {
    let (_dir, retriever) = build_temp_index("item-filter");
    let query = SearchQuery {
        item_kinds: vec!["function".into()],
        ..SearchQuery::new("write")
    };
    let hits = retriever.search(&query).expect("search");
    assert!(!hits.is_empty());
    for hit in &hits {
        let doc = retriever.get(&hit.id).expect("get");
        assert_eq!(doc.item_kind.as_deref(), Some("function"));
    }
}

#[test]
fn get_round_trips_the_document() {
    let (_dir, retriever) = build_temp_index("get");
    let hits = search(&retriever, "offload_blocking");
    let hit = hits
        .iter()
        .find(|h| h.symbol_path.as_deref() == Some("demo_core::runtime::offload_blocking"))
        .expect("found offload_blocking");
    let doc = retriever.get(&hit.id).expect("get");
    assert_eq!(doc.id, hit.id);
    assert_eq!(
        doc.symbol_path.as_deref(),
        Some("demo_core::runtime::offload_blocking")
    );
    assert_eq!(doc.item_kind.as_deref(), Some("function"));
    assert_eq!(doc.package.name, "demo-core");
    assert!(doc.text.contains("Runs the provided closure"));
    assert!(
        doc.signature
            .as_deref()
            .unwrap()
            .contains("pub fn offload_blocking<F, R>")
    );
    assert!(doc.source_span.is_some());
    assert!(
        doc.source_path
            .as_ref()
            .unwrap()
            .ends_with("crates/demo-core/src/lib.rs")
    );
}

#[test]
fn missing_document_id_is_a_typed_error() {
    let (_dir, retriever) = build_temp_index("missing");
    let err = retriever
        .get(&knowledge_core::DocumentId::from_raw("a".repeat(32)).unwrap())
        .expect_err("not found");
    assert!(matches!(
        err,
        knowledge_core::KnowledgeError::DocumentNotFound(_)
    ));
}

#[test]
fn symbol_lookup_exact_and_bare() {
    let (_dir, retriever) = build_temp_index("symbol");
    let exact = retriever
        .symbol_lookup(&SymbolQuery::new("demo_core::writer::Writer::write_all"))
        .expect("lookup");
    assert_eq!(exact[0].symbol_path, "demo_core::writer::Writer::write_all");
    assert_eq!(exact[0].kind, "function");
    assert!(exact[0].signature.as_deref().unwrap().contains("write_all"));

    let bare = retriever
        .symbol_lookup(&SymbolQuery::new("write_all"))
        .expect("lookup");
    assert_eq!(bare[0].symbol_path, "demo_core::writer::Writer::write_all");

    // Partial path (no crate prefix).
    let partial = retriever
        .symbol_lookup(&SymbolQuery::new("Writer::write_all"))
        .expect("lookup");
    assert_eq!(
        partial[0].symbol_path,
        "demo_core::writer::Writer::write_all"
    );
}

#[test]
fn symbol_lookup_separates_crate_versions() {
    let (_dir, retriever) = build_temp_index("symbol-versions");
    let infos = retriever
        .symbol_lookup(&SymbolQuery {
            symbol: "Engine::encode".into(),
            packages: Vec::new(),
            limit: 10,
        })
        .expect("lookup");
    // The exact path is found once per resolved version; either the
    // canonical path or its re-export is an acceptable form, but the top
    // results must lead and cover both versions.
    let exact: Vec<&knowledge_core::SymbolInfo> = infos.iter().take(2).collect();
    assert_eq!(exact.len(), 2);
    for info in &exact {
        assert!(
            info.symbol_path == "base64::engine::Engine::encode"
                || info.symbol_path == "base64::Engine::encode",
            "unexpected path {}",
            info.symbol_path
        );
    }
    let mut versions: Vec<&str> = exact.iter().map(|i| i.package_version.as_str()).collect();
    versions.sort();
    assert_eq!(versions, vec!["0.21.7", "0.22.1"]);
    for info in &infos {
        assert_eq!(info.package_name, "base64");
    }
}

#[test]
fn snippets_are_compact() {
    let (_dir, retriever) = build_temp_index("snippets");
    for query in [
        "runtime offload blocking CPU work",
        "writer buffering",
        "base64 engine",
    ] {
        let hits = search(&retriever, query);
        assert!(!hits.is_empty(), "query {query:?} should hit");
        for hit in &hits {
            assert!(
                hit.snippet.len() <= 320,
                "snippet for {} is {} chars",
                hit.title,
                hit.snippet.len()
            );
        }
    }
}

#[test]
fn search_does_not_expose_query_syntax() {
    // Tantivy syntax like field:term or +must must be treated as text.
    let (_dir, retriever) = build_temp_index("syntax");
    let hits = search(&retriever, "title:write_all");
    // It must not error, and results (if any) must be real documents.
    for hit in &hits {
        assert!(!hit.id.as_str().is_empty());
    }
}
