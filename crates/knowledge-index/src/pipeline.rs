//! The end-to-end indexing pipeline:
//! cargo metadata -> corpus (rustdoc + markdown) -> Tantivy index + metadata.
//! Called by the CLI and by tests; the MCP server only reads the result.

use std::path::{Path, PathBuf};

use tracing::info;

use crate::cargo::CargoUniverse;
use crate::corpus::{CorpusOptions, CorpusReport, RustdocScope, build_corpus};
use crate::error::IndexError;
use crate::rustdoc::{GeneratedRustdocProvider, RustdocProvider};
use crate::store::IndexMeta;
use crate::tantivy_index::{TantivyRetriever, build_index};

#[derive(Clone, Debug)]
pub struct IndexOptions {
    pub rustdoc_scope: RustdocScope,
    /// Toolchain passed to cargo for rustdoc generation (default "nightly").
    pub toolchain: Option<String>,
    /// Read prebuilt rustdoc JSON from this directory instead of invoking
    /// cargo (tests and pinned-toolchain workflows).
    pub prebuilt_rustdoc: Option<PathBuf>,
    /// Skip rustdoc entirely (metadata + markdown only).
    pub skip_rustdoc: bool,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            rustdoc_scope: RustdocScope::Workspace,
            toolchain: Some("nightly".to_string()),
            prebuilt_rustdoc: None,
            skip_rustdoc: false,
        }
    }
}

/// Result of a successful indexing run.
pub struct IndexOutcome {
    pub meta: IndexMeta,
    pub corpus: CorpusReport,
    pub index_dir: PathBuf,
}

/// Default index directory for a workspace root.
pub fn default_index_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".rust-knowledge")
}

/// Runs the full indexing pipeline for a workspace.
pub fn index_workspace(
    manifest_path: Option<&Path>,
    index_dir: Option<&Path>,
    options: &IndexOptions,
) -> Result<IndexOutcome, IndexError> {
    let universe = CargoUniverse::load(manifest_path)?;
    let index_dir = index_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| default_index_dir(universe.workspace_root()));

    let scope = if options.skip_rustdoc {
        RustdocScope::None
    } else {
        options.rustdoc_scope
    };

    let provider: Box<dyn RustdocProvider> = match &options.prebuilt_rustdoc {
        Some(dir) => Box::new(crate::rustdoc::PrebuiltRustdocProvider { dir: dir.clone() }),
        None => Box::new(GeneratedRustdocProvider::new(
            &universe,
            index_dir.join("cache").join("rustdoc"),
            options.toolchain.clone(),
        )),
    };

    let (documents, corpus) = build_corpus(
        &universe,
        provider.as_ref(),
        &CorpusOptions {
            rustdoc_scope: scope,
        },
    )?;

    let meta = IndexMeta {
        schema_version: IndexMeta::supported_schema(),
        workspace_root: universe.workspace_root().to_path_buf(),
        lock_hash: universe.lock_hash(),
        metadata_fingerprint: universe.fingerprint(),
        cargo_version: corpus.cargo_version.clone(),
        toolchain: options.toolchain.clone(),
        rustdoc_format_version: corpus.rustdoc_format_version,
        rustdoc_scope: format!("{scope:?}"),
        package_count: corpus.packages,
        document_count: documents.len(),
        built_at: now_rfc3339(),
        skipped: corpus.skipped.clone(),
        warnings: corpus.warnings.clone(),
    };

    build_index(&index_dir, &documents, &meta)?;
    info!(
        index_dir = %index_dir.display(),
        documents = documents.len(),
        "indexing complete"
    );

    Ok(IndexOutcome {
        meta,
        corpus,
        index_dir,
    })
}

/// Where a served index lives, plus the workspace context resolution had.
#[derive(Clone, Debug)]
pub struct ResolvedIndex {
    /// The index directory to open.
    pub index_dir: PathBuf,
    /// Workspace root as reported by cargo metadata, when it ran. `None`
    /// when an explicit index dir made cargo metadata unnecessary.
    pub workspace_root: Option<PathBuf>,
}

impl ResolvedIndex {
    /// Opens the resolved index directory — resolve-to-open in one step,
    /// without re-running resolution.
    pub fn open(&self) -> Result<TantivyRetriever, IndexError> {
        TantivyRetriever::open(&self.index_dir).map_err(IndexError::from)
    }
}

/// Resolves which index directory to serve, without opening it.
///
/// Precedence mirrors [open_retriever]: an explicit `index_dir` wins and
/// never runs cargo (the caller's working directory is irrelevant in that
/// case); otherwise cargo metadata resolves the workspace — from
/// `manifest_path` when given — and the index defaults to
/// `<workspace_root>/.rust-knowledge`.
pub fn resolve_index(
    manifest_path: Option<&Path>,
    index_dir: Option<&Path>,
) -> Result<ResolvedIndex, IndexError> {
    match index_dir {
        Some(dir) => Ok(ResolvedIndex {
            index_dir: dir.to_path_buf(),
            workspace_root: None,
        }),
        None => {
            let universe = CargoUniverse::load(manifest_path)?;
            let workspace_root = universe.workspace_root().to_path_buf();
            Ok(ResolvedIndex {
                index_dir: default_index_dir(&workspace_root),
                workspace_root: Some(workspace_root),
            })
        }
    }
}

/// Opens the index at the given directory, or the workspace default.
pub fn open_retriever(
    manifest_path: Option<&Path>,
    index_dir: Option<&Path>,
) -> Result<TantivyRetriever, IndexError> {
    resolve_index(manifest_path, index_dir)?.open()
}

fn now_rfc3339() -> String {
    // std-only approximation; the timestamp is informational only.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}
