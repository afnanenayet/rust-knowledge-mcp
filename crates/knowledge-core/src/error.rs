//! Typed errors for retrieval operations.

use std::path::PathBuf;

use crate::id::DocumentId;

pub type Result<T> = std::result::Result<T, KnowledgeError>;

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("no knowledge index at {path}; run `rust-knowledge index` first")]
    NoIndex { path: PathBuf },

    #[error(
        "index at {path} was built with schema version {index}, but this binary supports {supported}; rebuild the index"
    )]
    SchemaVersion {
        path: PathBuf,
        index: u32,
        supported: u32,
    },

    #[error("no document with id {0}")]
    DocumentNotFound(DocumentId),

    /// Engine-specific failures are converted at the implementation boundary;
    /// the context string must be enough to diagnose the underlying cause.
    #[error("search engine failure: {0}")]
    Engine(String),
}

impl KnowledgeError {
    pub fn engine(source: impl std::fmt::Display) -> Self {
        KnowledgeError::Engine(source.to_string())
    }
}
