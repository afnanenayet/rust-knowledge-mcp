//! Search requests, compact results, and the retrieval interface.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::Result;
use crate::id::DocumentId;
use crate::model::{KnowledgeDocument, SourceKind, SourceSpan};

/// A lexical knowledge search. Empty filter vectors mean "no restriction".
///
/// `text` is free-form natural language or identifiers; Tantivy query syntax
/// is deliberately *not* exposed.
#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub text: String,
    pub packages: Vec<String>,
    pub source_kinds: Vec<SourceKind>,
    pub item_kinds: Vec<String>,
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        SearchQuery {
            text: text.into(),
            packages: Vec::new(),
            source_kinds: Vec::new(),
            item_kinds: Vec::new(),
            limit: 8,
        }
    }
}

/// A compact search preview. Intentionally small: an agent should be able to
/// decide whether to `get` the full document from this alone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: DocumentId,
    pub package_name: String,
    pub package_version: String,
    pub source_kind: SourceKind,
    pub title: String,
    pub symbol_path: Option<String>,
    pub section_path: Vec<String>,
    /// A few hundred characters, not the document body.
    pub snippet: String,
    /// Relevance score (engine-specific; only ordering is meaningful).
    pub score: f32,
}

impl SearchHit {
    /// Compact one-line provenance, e.g. `tokio@1.40.0 [rustdoc_item]`.
    pub fn provenance(&self) -> String {
        format!(
            "{}@{} [{}]",
            self.package_name, self.package_version, self.source_kind
        )
    }
}

/// Near-exact symbol lookup request.
#[derive(Clone, Debug)]
pub struct SymbolQuery {
    /// Symbol path or last segment, e.g. `tokio::task::spawn_blocking` or
    /// `spawn_blocking`.
    pub symbol: String,
    pub packages: Vec<String>,
    pub limit: usize,
}

impl SymbolQuery {
    pub fn new(symbol: impl Into<String>) -> Self {
        SymbolQuery {
            symbol: symbol.into(),
            packages: Vec::new(),
            limit: 5,
        }
    }
}

/// Compact structured API information returned by symbol lookup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub id: DocumentId,
    pub package_name: String,
    pub package_version: String,
    pub kind: String,
    pub symbol_path: String,
    pub signature: Option<String>,
    /// Short doc snippet (first paragraph-ish), not the whole docs.
    pub snippet: String,
    pub source_path: Option<PathBuf>,
    pub source_span: Option<SourceSpan>,
    pub related_symbols: Vec<String>,
}

/// The retrieval interface. Synchronous on purpose: the Tantivy implementation
/// behind it is synchronous. A future vector or hybrid retriever implements
/// the same interface (see docs/design.md, "Semantic retrieval").
pub trait KnowledgeRetriever {
    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>>;

    fn get(&self, id: &DocumentId) -> Result<KnowledgeDocument>;

    fn symbol_lookup(&self, query: &SymbolQuery) -> Result<Vec<SymbolInfo>>;
}
