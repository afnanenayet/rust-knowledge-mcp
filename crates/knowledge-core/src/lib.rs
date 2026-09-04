//! Normalized data model for the rust-knowledge retrieval prototype.
//!
//! This crate holds the corpus model, the stable document-identity scheme, the
//! search/query types and the retrieval interface. It deliberately has no
//! dependency on cargo, rustdoc, Tantivy or MCP: CLI, MCP server, tests and
//! any future (vector) retriever all build on these types.

pub mod error;
pub mod id;
pub mod model;
pub mod query;

pub use error::{KnowledgeError, Result};
pub use id::DocumentId;
pub use model::{KnowledgeDocument, PackageIdentity, SourceKind, SourceSpan};
pub use query::{KnowledgeRetriever, SearchHit, SearchQuery, SymbolInfo, SymbolQuery};
