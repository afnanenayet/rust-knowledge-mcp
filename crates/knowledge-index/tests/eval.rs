//! Stage 5: the committed eval set (../../evals/queries.toml) run against a
//! fixture index. Retrieval quality is asserted, not assumed: every case must
//! produce a useful hit within its max_rank.

use std::path::{Path, PathBuf};

use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::eval::{EvalCase, run_eval, summarize};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;
use knowledge_index::store::IndexMeta;
use knowledge_index::tantivy_index::{TantivyRetriever, build_index};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

fn build_fixture_retriever() -> TantivyRetriever {
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
    #[expect(
        clippy::mem_forget,
        reason = "leak the TempDir for the test's lifetime"
    )]
    std::mem::forget(dir);
    retriever
}

fn eval_cases() -> Vec<EvalCase> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/queries.toml");
    let raw = std::fs::read_to_string(&path).expect("eval set present");
    knowledge_index::eval::parse_cases(&raw).expect("eval set parses")
}

#[test]
fn eval_set_is_wellformed_and_sized() {
    let cases = eval_cases();
    assert!(cases.len() >= 20, "want ~20 cases, got {}", cases.len());
    let categories: std::collections::HashSet<_> =
        cases.iter().map(|c| c.category.clone()).collect();
    for expected in [
        "known_symbol",
        "api_discovery",
        "conceptual",
        "cross_package",
        "version_sensitive",
    ] {
        assert!(
            categories.contains(expected),
            "eval set must cover {expected}, has {categories:?}"
        );
    }
}

#[test]
fn retrieval_quality_meets_the_eval_set() {
    let retriever = build_fixture_retriever();
    let cases = eval_cases();
    let outcomes = run_eval(&retriever, cases);
    let summary = summarize(&outcomes);

    for outcome in &outcomes {
        if outcome.passed() {
            println!(
                "PASS rank {:>2} {:>16} {:?}",
                outcome.rank.unwrap_or(0),
                outcome.case.category,
                outcome.case.text
            );
        } else {
            println!(
                "FAIL rank {:>2} {:>16} {:?} (expected one of {:?}, top: {:?})",
                outcome.rank.unwrap_or(0),
                outcome.case.category,
                outcome.case.text,
                outcome.case.expect_any,
                outcome.top
            );
        }
    }

    assert_eq!(
        summary.passed,
        summary.total,
        "{} of {} eval cases failed; MRR {:.3}. Failures: {:?}",
        summary.total - summary.passed,
        summary.total,
        summary.mrr,
        summary
            .failures
            .iter()
            .map(|f| (f.case.text.clone(), f.rank, f.top.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        summary.mrr >= 0.6,
        "reciprocal rank collapsed: {:.3}",
        summary.mrr
    );
}
