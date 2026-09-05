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
    /// Explicit cargo binary (frontends' config layer). None falls back to
    /// $RUST_KNOWLEDGE_CARGO, then to cargo on $PATH.
    pub cargo: Option<PathBuf>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            rustdoc_scope: RustdocScope::Workspace,
            toolchain: Some("nightly".to_string()),
            prebuilt_rustdoc: None,
            skip_rustdoc: false,
            cargo: None,
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
    let universe = CargoUniverse::load_with(manifest_path, options.cargo.as_deref())?;
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
            options.cargo.clone(),
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

/// Opens the index at the given directory, or the workspace default.
///
/// Delegates to [open_retriever_with] with no explicit cargo binary:
/// the metadata spawn then falls back to $RUST_KNOWLEDGE_CARGO and
/// finally to cargo on $PATH.
pub fn open_retriever(
    manifest_path: Option<&Path>,
    index_dir: Option<&Path>,
) -> Result<TantivyRetriever, IndexError> {
    open_retriever_with(manifest_path, index_dir, None)
}

/// Like [open_retriever], with an explicit cargo binary for the
/// `cargo metadata` run that discovers the workspace when `index_dir`
/// is absent. Both frontends pass their resolved `--cargo` here, so
/// every path that spawns cargo honors the flag (see
/// [CargoUniverse::load_with] for the None fallback chain).
pub fn open_retriever_with(
    manifest_path: Option<&Path>,
    index_dir: Option<&Path>,
    cargo: Option<&Path>,
) -> Result<TantivyRetriever, IndexError> {
    let index_dir = match index_dir {
        Some(dir) => dir.to_path_buf(),
        None => {
            let universe = CargoUniverse::load_with(manifest_path, cargo)?;
            default_index_dir(universe.workspace_root())
        }
    };
    TantivyRetriever::open(&index_dir).map_err(IndexError::from)
}

fn now_rfc3339() -> String {
    // std-only approximation; the timestamp is informational only.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::open_retriever_with;
    use crate::error::IndexError;

    /// Pins the --cargo plumbing: an explicit cargo binary must reach the
    /// metadata spawn that discovers the workspace when --index-dir is
    /// absent (a nonexistent path fails the spawn) instead of silently
    /// falling back to cargo on $PATH.
    #[test]
    fn explicit_cargo_reaches_the_metadata_spawn() {
        let bad_cargo = Path::new("/definitely-not-a-cargo-binary-0123456789");
        let error = match open_retriever_with(None, None, Some(bad_cargo)) {
            Err(error) => error,
            Ok(_) => panic!("a bad cargo path must fail the metadata spawn"),
        };
        assert!(
            matches!(error, IndexError::CargoMetadata { .. }),
            "the bad cargo must fail the metadata spawn, got: {error:?}"
        );
    }
}
