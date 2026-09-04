//! Cargo-universe ingestion, corpus normalization and lexical retrieval.
//!
//! Layout (one small module per concern):
//!
//! * [cargo] - the resolved package universe from cargo metadata.
//! * [error] - typed errors with package/command/path context.
//! * [rustdoc] - rustdoc JSON generation (behind a provider interface) and
//!   normalization into [knowledge_core::KnowledgeDocument]s.
//! * [markdown] - README/docs/*.md discovery and structural chunking.
//! * [corpus] - assembles the full normalized corpus for a workspace.
//! * [tantivy_index] - schema, index build, and the [KnowledgeRetriever]
//!   implementation.
//! * [store] - persistent index layout and metadata.

pub mod cargo;
pub mod corpus;
pub mod error;
pub mod markdown;
pub mod rustdoc;

pub use cargo::CargoUniverse;
pub use corpus::{CorpusOptions, CorpusReport, RustdocScope, build_corpus};
pub use error::IndexError;
