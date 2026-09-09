//! Persistent index layout and metadata.
//!
//! <index-dir>/
//! ├── index-meta.json      provenance & fingerprint (see `IndexMeta`)
//! ├── tantivy/             the lexical index
//! ├── corpus.jsonl         normalized documents (inspectability)
//! └── cache/rustdoc/       versioned rustdoc JSON artifacts
//!
//! The layout is deliberately simple so that incremental indexing can later
//! replace "rebuild everything" without another format migration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::IndexError;
use crate::tantivy_index::schema::INDEX_SCHEMA_VERSION;

pub const META_FILE: &str = "index-meta.json";
pub const CORPUS_FILE: &str = "corpus.jsonl";
pub const TANTIVY_DIR: &str = "tantivy";

/// Everything needed to determine what built an index and whether the index
/// is still valid for the current workspace state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexMeta {
    pub schema_version: u32,
    pub workspace_root: PathBuf,
    /// SHA-256 of the workspace Cargo.lock, when present.
    pub lock_hash: String,
    /// Fingerprint of the resolved package universe.
    pub metadata_fingerprint: String,
    /// cargo version used for rustdoc generation.
    pub cargo_version: Option<String>,
    /// Toolchain requested for rustdoc generation (e.g. "nightly").
    pub toolchain: Option<String>,
    /// rustdoc JSON format version of the parsed artifacts.
    pub rustdoc_format_version: Option<u32>,
    pub rustdoc_scope: String,
    pub package_count: usize,
    pub document_count: usize,
    /// RFC 3339 build timestamp (informational; not part of identity).
    pub built_at: String,
    pub skipped: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

impl IndexMeta {
    #[must_use]
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Saves index metadata as pretty-printed JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when metadata cannot be serialized or written.
    pub fn save(&self, path: &Path) -> Result<(), IndexError> {
        let json = serde_json::to_string_pretty(self).map_err(|e| IndexError::MetaCorrupt {
            path: path.to_path_buf(),
            source: e,
        })?;
        std::fs::write(path, json).map_err(|e| IndexError::io(path, e))
    }

    /// Loads index metadata; a missing file means "no index here".
    ///
    /// # Errors
    ///
    /// Returns an error when the metadata file cannot be read or decoded.
    pub fn load(path: &Path) -> Result<IndexMeta, IndexError> {
        if !path.is_file() {
            return Err(IndexError::Knowledge(
                knowledge_core::KnowledgeError::NoIndex {
                    path: path.parent().unwrap_or(path).to_path_buf(),
                },
            ));
        }
        let raw = std::fs::read_to_string(path).map_err(|e| IndexError::io(path, e))?;
        serde_json::from_str(&raw).map_err(|e| IndexError::MetaCorrupt {
            path: path.to_path_buf(),
            source: e,
        })
    }

    /// The tantivy index directory inside an index dir.
    #[must_use]
    pub fn tantivy_dir(index_dir: &Path) -> PathBuf {
        index_dir.join(TANTIVY_DIR)
    }

    #[must_use]
    pub fn corpus_path(index_dir: &Path) -> PathBuf {
        index_dir.join(CORPUS_FILE)
    }

    #[must_use]
    pub fn meta_path(index_dir: &Path) -> PathBuf {
        index_dir.join(META_FILE)
    }

    #[must_use]
    pub fn supported_schema() -> u32 {
        INDEX_SCHEMA_VERSION
    }
}

/// Writes the normalized corpus as JSONL (debug/inspection artifact).
///
/// # Errors
///
/// Returns an error when the corpus file cannot be created or written, or a
/// document cannot be serialized.
pub fn write_corpus(
    index_dir: &Path,
    documents: &[knowledge_core::KnowledgeDocument],
) -> Result<(), IndexError> {
    use std::io::Write;
    let path = IndexMeta::corpus_path(index_dir);
    let file = std::fs::File::create(&path).map_err(|e| IndexError::io(&path, e))?;
    let mut writer = std::io::BufWriter::new(file);
    for doc in documents {
        let line = serde_json::to_string(doc).map_err(|e| IndexError::MetaCorrupt {
            path: path.clone(),
            source: e,
        })?;
        writeln!(writer, "{line}").map_err(|e| IndexError::io(&path, e))?;
    }
    writer.flush().map_err(|e| IndexError::io(&path, e))?;
    Ok(())
}

/// Loads the normalized corpus (used by tooling/tests, not by search).
///
/// # Errors
///
/// Returns an error when the corpus file cannot be read or a line is invalid.
pub fn read_corpus(index_dir: &Path) -> Result<Vec<knowledge_core::KnowledgeDocument>, IndexError> {
    let path = IndexMeta::corpus_path(index_dir);
    let raw = std::fs::read_to_string(&path).map_err(|e| IndexError::io(&path, e))?;
    let mut docs = Vec::new();
    for (n, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let doc: knowledge_core::KnowledgeDocument = serde_json::from_str(line)
            .map_err(|e| IndexError::Engine(format!("corpus.jsonl line {}: {e}", n + 1)))?;
        docs.push(doc);
    }
    Ok(docs)
}
