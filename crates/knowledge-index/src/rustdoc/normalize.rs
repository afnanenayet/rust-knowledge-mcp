//! Normalizing a parsed rustdoc JSON artifact into KnowledgeDocuments.
//!
//! Walks the local crate from the root module, keeping the fully qualified
//! path of every public item. Item documentation stays attached to its
//! symbol; the crate-level and module docs become RustdocModule documents.
//! Derive-generated/synthetic impls and impls of foreign traits are skipped
//! as compiler noise; trait items and local-trait impl methods are kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use knowledge_core::{DocumentId, KnowledgeDocument, PackageIdentity, SourceKind, SourceSpan};
use rustdoc_types::{Crate, Id, Item, ItemEnum, Visibility};
use tracing::{debug, info, info_span};

use crate::error::IndexError;
use crate::rustdoc::render::{render_function, render_type};

/// Result of normalizing one rustdoc artifact.
pub struct RustdocNormalized {
    pub format_version: u32,
    pub documents: Vec<KnowledgeDocument>,
    pub warnings: Vec<String>,
}

/// Parses and normalizes one rustdoc JSON artifact.
pub fn normalize(
    package: &PackageIdentity,
    artifact: &Path,
    workspace_root: &Path,
) -> Result<RustdocNormalized, IndexError> {
    let span = info_span!("rustdoc_normalize", package = %package.display());
    let _enter = span.enter();

    let raw = std::fs::read_to_string(artifact).map_err(|e| IndexError::io(artifact, e))?;

    // The JSON is a versioned external format: check before parsing fully,
    // and produce a precise diagnostic when versions mismatch.
    let probe: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| IndexError::RustdocParse {
            package: package.display(),
            got: 0,
            expected: rustdoc_types::FORMAT_VERSION,
            artifact: artifact.to_path_buf(),
            cause: e.to_string(),
        })?;
    let got = probe
        .get("format_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let expected = rustdoc_types::FORMAT_VERSION;

    let parsed: Crate = match serde_json::from_str(&raw) {
        Ok(krate) => krate,
        Err(e) => {
            if got != expected {
                // Version mismatch is the likely cause; say so explicitly.
                return Err(IndexError::RustdocFormatVersion {
                    package: package.display(),
                    got,
                    expected,
                    artifact: artifact.to_path_buf(),
                });
            }
            return Err(IndexError::RustdocParse {
                package: package.display(),
                got,
                expected,
                artifact: artifact.to_path_buf(),
                cause: e.to_string(),
            });
        }
    };

    let mut warnings = Vec::new();
    if got != expected {
        let warning = format!(
            "rustdoc JSON for {} has format_version {got}, parser supports {expected};              parsing succeeded, but results may be inaccurate",
            package.display()
        );
        debug!("{warning}");
        warnings.push(warning);
    }

    let mut walker = Walker {
        krate: &parsed,
        package,
        workspace_root,
        local_paths: HashMap::new(),
        docs: Vec::new(),
    };
    let root_path = crate_name_for(package);
    walker.walk_item(parsed.root, root_path, Context::Root);

    info!(
        documents = walker.docs.len(),
        format_version = got,
        "normalized rustdoc artifact"
    );

    Ok(RustdocNormalized {
        format_version: got,
        documents: walker.docs,
        warnings,
    })
}

/// The symbol-path prefix for a package: its lib target name is not always
/// the package name (dashes become underscores); rustdoc gives the root
/// item that name, so fall back to the normalized package name when the
/// root item is nameless.
fn crate_name_for(package: &PackageIdentity) -> String {
    // rustdoc names the root module after the crate; package names with
    // dashes are already underscored in crate names.
    package.name.replace('-', "_")
}

/// Where an item sits, driving inclusion policy.
#[derive(Clone, Copy, PartialEq)]
enum Context {
    /// Module tree: only public items count.
    Module,
    /// Inside a trait declaration: items are API even though rustdoc marks
    /// them with default visibility.
    Trait,
    /// Inside an impl of a local trait: same as trait items.
    LocalImpl,
    /// The crate root module itself.
    Root,
}

struct Walker<'a> {
    krate: &'a Crate,
    package: &'a PackageIdentity,
    workspace_root: &'a Path,
    /// Local item id -> qualified symbol path.
    local_paths: HashMap<Id, String>,
    docs: Vec<KnowledgeDocument>,
}

impl<'a> Walker<'a> {
    fn walk_item(&mut self, id: Id, path: String, ctx: Context) {
        let Some(item) = self.krate.index.get(&id) else {
            return;
        };
        if item.crate_id != 0 {
            return; // external crate item (std/core etc.)
        }

        match &item.inner {
            ItemEnum::Module(m) => {
                self.emit_module(item, &path, m.is_crate);
                for child in &m.items {
                    if let Some(name) = self.item_name(child) {
                        let child_path = format!("{path}::{name}");
                        self.walk_item(*child, child_path, Context::Module);
                    } else {
                        debug!(id = ?child, "skipping nameless module child");
                    }
                }
            }
            ItemEnum::Struct(s) => {
                self.emit_item(item, &path, "struct", ctx);
                self.local_paths.insert(id, path.clone());
                for impl_id in &s.impls {
                    self.walk_impl(*impl_id, path.clone());
                }
            }
            ItemEnum::Enum(e) => {
                self.emit_item(item, &path, "enum", ctx);
                self.local_paths.insert(id, path.clone());
                for variant_id in &e.variants {
                    if let Some(name) = self.item_name(variant_id) {
                        let variant_path = format!("{path}::{name}");
                        self.walk_item(*variant_id, variant_path, Context::Module);
                    }
                }
                for impl_id in &e.impls {
                    self.walk_impl(*impl_id, path.clone());
                }
            }
            ItemEnum::Union(u) => {
                self.emit_item(item, &path, "union", ctx);
                self.local_paths.insert(id, path.clone());
                for impl_id in &u.impls {
                    self.walk_impl(*impl_id, path.clone());
                }
            }
            ItemEnum::Trait(t) => {
                self.emit_item(item, &path, "trait", ctx);
                self.local_paths.insert(id, path.clone());
                for child in &t.items {
                    if let Some(name) = self.item_name(child) {
                        let child_path = format!("{path}::{name}");
                        self.walk_item(*child, child_path, Context::Trait);
                    }
                }
                // Implementations of this trait elsewhere are intentionally
                // not walked in v1 (their methods duplicate trait docs).
            }
            ItemEnum::Function(f) => {
                self.emit_function(item, &path, f, ctx);
            }
            ItemEnum::TypeAlias(_) => {
                self.emit_item(item, &path, "type_alias", ctx);
            }
            ItemEnum::Constant { .. } => {
                self.emit_item(item, &path, "constant", ctx);
            }
            ItemEnum::Static(_) => {
                self.emit_item(item, &path, "static", ctx);
            }
            ItemEnum::Variant(_) => {
                self.emit_item(item, &path, "variant", ctx);
            }
            ItemEnum::Macro(_) => {
                self.emit_item(item, &path, "macro", ctx);
            }
            ItemEnum::ProcMacro(pm) => {
                let kind = match pm.kind {
                    rustdoc_types::MacroKind::Bang => "macro",
                    rustdoc_types::MacroKind::Attr => "attribute_macro",
                    rustdoc_types::MacroKind::Derive => "derive_macro",
                };
                self.emit_item(item, &path, kind, ctx);
            }
            // Re-exports, extern crates, struct fields (v1: skipped as noise),
            // and impl containers reached outside walk_impl.
            ItemEnum::Use(_)
            | ItemEnum::ExternCrate { .. }
            | ItemEnum::StructField(_)
            | ItemEnum::Impl(_)
            | ItemEnum::ExternType
            | ItemEnum::TraitAlias(_)
            | ItemEnum::AssocConst { .. }
            | ItemEnum::AssocType { .. }
            | ItemEnum::Primitive(_) => {}
        }
    }

    /// Walks one impl block for the given owner (the qualified path of the
    /// type the impl is for, when known).
    fn walk_impl(&mut self, impl_id: Id, owner_path: String) {
        let Some(item) = self.krate.index.get(&impl_id) else {
            return;
        };
        if item.crate_id != 0 {
            return;
        }
        let ItemEnum::Impl(imp) = &item.inner else {
            return;
        };
        if imp.is_synthetic {
            return; // derive/autotrait-generated noise
        }
        // Impl blocks of foreign traits on local types (Debug, From, ...) are
        // boilerplate for every type in existence: skip them. Inherent impls
        // and impls of local traits are the interesting ones.
        if let Some(trait_path) = &imp.trait_
            && !self.local_paths.contains_key(&trait_path.id)
        {
            return;
        }
        let parent = self.impl_parent(imp, &owner_path);
        for child in &imp.items {
            if let Some(name) = self.item_name(child) {
                let child_path = format!("{parent}::{name}");
                self.walk_item(*child, child_path, Context::LocalImpl);
            }
        }
    }

    /// Resolves the parent path for an impl block's items.
    fn impl_parent(&self, imp: &rustdoc_types::Impl, owner_path: &str) -> String {
        // A local, non-generic self type resolves to its walked path.
        if let rustdoc_types::Type::ResolvedPath(p) = &imp.for_
            && let Some(full) = self.local_paths.get(&p.id)
        {
            return full.clone();
        }
        let rendered = render_type(&imp.for_);
        if !rendered.is_empty() && rendered != owner_path {
            return rendered;
        }
        owner_path.to_string()
    }

    fn item_name(&self, id: &Id) -> Option<String> {
        self.krate.index.get(id).and_then(|item| item.name.clone())
    }

    fn is_public(&self, item: &Item) -> bool {
        matches!(item.visibility, Visibility::Public)
    }

    fn emit_module(&mut self, item: &Item, path: &str, is_root: bool) {
        // Modules without docs carry no retrieval value by themselves.
        if item.docs.as_deref().unwrap_or("").trim().is_empty() {
            return;
        }
        let title = match (&item.name, is_root) {
            (Some(name), _) => name.clone(),
            (None, _) => self.package.name.clone(),
        };
        self.push_doc(item, path, SourceKind::RustdocModule, &title, None, None);
    }

    fn emit_item(&mut self, item: &Item, path: &str, kind: &str, ctx: Context) {
        if !self.is_public(item) && ctx == Context::Module {
            return;
        }
        let title = item.name.clone().unwrap_or_else(|| path.to_string());
        self.push_doc(
            item,
            path,
            SourceKind::RustdocItem,
            &title,
            Some(kind),
            None,
        );
    }

    fn emit_function(
        &mut self,
        item: &Item,
        path: &str,
        f: &rustdoc_types::Function,
        ctx: Context,
    ) {
        if !self.is_public(item) && ctx == Context::Module {
            return;
        }
        let title = item.name.clone().unwrap_or_else(|| path.to_string());
        let public = self.is_public(item) || ctx != Context::Module;
        let signature = render_function(&title, f, public);
        self.push_doc(
            item,
            path,
            SourceKind::RustdocItem,
            &title,
            Some("function"),
            Some(signature),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_doc(
        &mut self,
        item: &Item,
        path: &str,
        source_kind: SourceKind,
        title: &str,
        item_kind: Option<&str>,
        signature: Option<String>,
    ) {
        let id = DocumentId::from_identity(&[
            &self.package.package_id,
            source_kind.as_str(),
            path,
            "",
            "",
        ]);

        let related = self.related_symbols(item, path);

        let (source_path, source_span) = self.resolve_span(item);

        self.docs.push(KnowledgeDocument {
            id,
            package: self.package.clone(),
            source_kind,
            title: title.to_string(),
            symbol_path: Some(path.to_string()),
            item_kind: item_kind.map(|k| k.to_string()),
            section_path: Vec::new(),
            text: item.docs.clone().unwrap_or_default(),
            source_path,
            source_span,
            related_symbols: related,
            signature,
        });
    }

    /// Resolves intra-doc link targets of this item to symbol paths.
    fn related_symbols(&self, item: &Item, self_path: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for target in item.links.values() {
            if let Some(path) = self.symbol_path_of(target)
                && path != self_path
                && !out.contains(&path)
            {
                out.push(path);
            }
            if out.len() >= 8 {
                break;
            }
        }
        out
    }

    /// Resolves an item id to a qualified path, locally first, then through
    /// the crate's paths summary (which covers external items).
    fn symbol_path_of(&self, id: &Id) -> Option<String> {
        if let Some(local) = self.local_paths.get(id) {
            return Some(local.clone());
        }
        self.krate
            .paths
            .get(id)
            .map(|summary| summary.path.join("::"))
    }

    /// rustdoc spans are relative to the invocation directory (the cargo
    /// workspace root); resolve them there.
    fn resolve_span(&self, item: &Item) -> (Option<PathBuf>, Option<SourceSpan>) {
        let Some(span) = &item.span else {
            return (None, None);
        };
        let mut path = PathBuf::from(&span.filename);
        if path.is_relative() {
            path = self.workspace_root.join(path);
        }
        let source_span = SourceSpan {
            start_line: span.begin.0 as u32,
            start_col: span.begin.1 as u32,
            end_line: span.end.0 as u32,
            end_col: span.end.1 as u32,
        };
        (Some(path), Some(source_span))
    }
}
