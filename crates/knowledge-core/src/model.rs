//! The normalized corpus model.
//!
//! Every retrievable artifact is a [KnowledgeDocument] with full provenance:
//! which package (exact Cargo identity, not just crate name), which source
//! kind, where it came from on disk, and — for Rust symbols — the qualified
//! path and source span.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::DocumentId;

/// Where a document came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A documented rustdoc item (struct, fn, trait, method, ...).
    RustdocItem,
    /// A rustdoc module (including the crate root, i.e. `//!` docs).
    RustdocModule,
    /// A package README.
    CrateReadme,
    /// A Markdown document under the package (e.g. `docs/*.md`).
    MarkdownDocument,
}

impl SourceKind {
    pub const ALL: [SourceKind; 4] = [
        SourceKind::RustdocItem,
        SourceKind::RustdocModule,
        SourceKind::CrateReadme,
        SourceKind::MarkdownDocument,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::RustdocItem => "rustdoc_item",
            SourceKind::RustdocModule => "rustdoc_module",
            SourceKind::CrateReadme => "crate_readme",
            SourceKind::MarkdownDocument => "markdown_document",
        }
    }

    /// True for documents derived from rustdoc JSON.
    pub fn is_rustdoc(&self) -> bool {
        matches!(self, SourceKind::RustdocItem | SourceKind::RustdocModule)
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SourceKind::ALL
            .iter()
            .find(|k| k.as_str() == s)
            .copied()
            .ok_or_else(|| {
                format!(
                    "unknown source kind: {s:?} (use one of rustdoc_item, rustdoc_module, crate_readme, markdown_document)"
                )
            })
    }
}

/// A 1-based line/column range in a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// Exact identity of a resolved Cargo package.
///
/// Keyed by Cargo's opaque `PackageId`, so two versions of the same crate
/// in one graph are two distinct identities. `source` is Cargo's source repr
/// (`registry+...`, `git+...`); `None` means the package is local
/// (workspace member or path dependency).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PackageIdentity {
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub manifest_path: PathBuf,
}

impl PackageIdentity {
    /// Package root directory (the manifest's parent).
    pub fn root(&self) -> &Path {
        self.manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
    }

    /// Short human-readable source label ("local", "registry", "git").
    /// Distinguishing workspace members from path dependencies requires the
    /// full Cargo metadata and lives in `knowledge-index`.
    pub fn source_label(&self) -> &str {
        match &self.source {
            None => "local",
            Some(repr) if repr.starts_with("registry+") => "registry",
            Some(repr) if repr.starts_with("git+") => "git",
            Some(_) => "source",
        }
    }

    pub fn display(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// One normalized, retrievable piece of documentation with provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    /// Stable, deterministic identifier (see [crate::id]).
    pub id: DocumentId,

    /// Exact package identity this document came from.
    pub package: PackageIdentity,

    pub source_kind: SourceKind,

    /// Display title: symbol name, heading, or file title.
    pub title: String,

    /// Fully qualified Rust symbol path, e.g. `tokio::task::spawn_blocking`.
    pub symbol_path: Option<String>,

    /// Rust item kind ("function", "struct", "trait", ...), when applicable.
    pub item_kind: Option<String>,

    /// Markdown heading ancestry (for Markdown-derived documents).
    pub section_path: Vec<String>,

    /// Full text of this chunk (Markdown; item docs plus signature for rustdoc).
    pub text: String,

    /// File this document came from, when known.
    pub source_path: Option<PathBuf>,

    /// Span in `source_path`, when known (rustdoc items).
    pub source_span: Option<SourceSpan>,

    /// Related symbols (e.g. resolved intra-doc link targets).
    pub related_symbols: Vec<String>,

    /// Concise API signature, when applicable (e.g. `pub fn flush(&mut self)`).
    pub signature: Option<String>,
}

impl KnowledgeDocument {
    /// Heading/symbol context for compact display, e.g.
    /// `Runtime > CPU-bound work` or `tokio::task::spawn_blocking`.
    pub fn context(&self) -> String {
        if let Some(symbol) = &self.symbol_path {
            symbol.clone()
        } else if !self.section_path.is_empty() {
            self.section_path.join(" > ")
        } else {
            self.title.clone()
        }
    }

    /// Short provenance label, e.g. `tokio@1.40 [rustdoc_item / function]`.
    pub fn provenance(&self) -> String {
        match self.item_kind {
            Some(ref kind) => {
                format!(
                    "{} [{} / {}]",
                    self.package.display(),
                    self.source_kind,
                    kind
                )
            }
            None => format!("{} [{}]", self.package.display(), self.source_kind),
        }
    }
}
