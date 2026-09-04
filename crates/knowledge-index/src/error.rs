//! Typed errors with enough context to diagnose failures:
//! which package, which command, which path, which rustdoc format version.

use std::path::PathBuf;

use knowledge_core::KnowledgeError;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("cargo metadata failed: {message}")]
    CargoMetadata { message: String },

    #[error("package not found in the resolved universe: {spec}")]
    PackageNotFound { spec: String },

    #[error("failed to spawn cargo for rustdoc generation ({command}): {cause}")]
    RustdocSpawn { command: String, cause: String },

    #[error("rustdoc generation failed for {spec}: {reason}")]
    RustdocFailed { spec: String, reason: String },

    #[error(
        "failed to parse rustdoc JSON for {package}: format version {got} is incompatible with the parser (rustdoc-types supports {expected}); artifact: {artifact}"
    )]
    RustdocFormatVersion {
        package: String,
        got: u32,
        expected: u32,
        artifact: PathBuf,
    },

    #[error(
        "failed to parse rustdoc JSON for {package} (format version {got}, parser supports {expected}); artifact: {artifact}; cause: {cause}"
    )]
    RustdocParse {
        package: String,
        got: u32,
        expected: u32,
        artifact: PathBuf,
        cause: String,
    },

    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("index metadata at {path} is corrupt: {source}")]
    MetaCorrupt {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[error(transparent)]
    Knowledge(#[from] KnowledgeError),

    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),
}

impl IndexError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        IndexError::Io {
            path: path.into(),
            source,
        }
    }

    pub fn cargo_metadata(source: impl std::fmt::Display) -> Self {
        IndexError::CargoMetadata {
            message: source.to_string(),
        }
    }
}
