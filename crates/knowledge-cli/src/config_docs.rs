//! Generates the self-contained HTML configuration reference page.
//!
//! The page is rendered by walking the public facet shapes that parse both
//! binaries (`Cli`/`Command` for rust-knowledge, `McpArgs` for
//! knowledge-mcp, `IndexDirEnv` for the environment layer), so the
//! documentation cannot drift from the parsing code (issue #2).
//!
//! Properties: hand-rolled rendering (no template engine), zero JavaScript,
//! inline CSS, every piece of text HTML-escaped, and deterministic output
//! (no timestamps, no absolute paths) so the committed file can be pinned
//! by a regeneration test.

use std::path::Path;

use facet::{
    Def, DefaultSource, Facet, Field, PtrConst, PtrUninit, Shape, Type, TypeOps, UserType, Variant,
};
use facet_reflect::Peek;
use figue::Attr;
use knowledge_index::config::{IndexDirEnv, McpArgs};

use crate::{ABOUT, Cli, PROGRAM};

/// Render the full HTML configuration reference.
pub fn render() -> String {
    let mut html = String::new();
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n");
    html.push_str("<meta charset=\"utf-\">\n");
    html.push_str("<title>rust-knowledge configuration reference</title>\n");
    html.push_str(CSS);
    html.push_str("</head>\n<body>\n");

    html.push_str(&format!(
        "<h1>{} &amp; {} configuration reference</h1>\n",
        esc(PROGRAM),
        esc("knowledge-mcp"),
    ));
    html.push_str(concat!(
        "<p class=\"note\">This page is generated from the facet shapes ",
        "that parse both binaries (regenerate with ",
        "<code>rust-knowledge config-docs</code>), so the reference cannot ",
        "drift from the code that interprets it. Configuration layers apply ",
        "with precedence <strong>CLI arguments &gt; environment variables ",
        "&gt; defaults</strong>.</p>\n",
    ));
    html.push_str(concat!(
        "<nav><ol><li><a href=\"#rust-knowledge\">rust-knowledge</a></li>",
        "<li><a href=\"#knowledge-mcp\">knowledge-mcp</a></li>",
        "<li><a href=\"#environment-variables\">Environment variables</a></li>",
        "</ol></nav>\n",
    ));

    render_binary_section(&mut html, PROGRAM, ABOUT, Cli::SHAPE);
    render_binary_section(
        &mut html,
        "knowledge-mcp",
        "MCP server exposing the rust-knowledge retrieval engine",
        McpArgs::SHAPE,
    );
    render_env_section(&mut html);

    html.push_str(concat!(
        "<footer>Generated from the facet shapes of <code>rust-knowledge</code> ",
        "and <code>knowledge-mcp</code> by <code>rust-knowledge config-docs</code>. ",
        "Output is deterministic: regenerating reproduces this file byte for byte.</footer>\n",
    ));
    html.push_str("</body>\n</html>\n");
    html
}

/// Render the reference and write it to `path`, creating parent directories.
pub fn write_to(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, render())
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(())
}

const CSS: &str = concat!(
    "<style>\n",
    "body { font-family: system-ui, sans-serif; max-width: 62rem; margin: 2rem auto; padding: 0 1rem; line-height: 1.5; }\n",
    "h1 { font-size: 1.6rem; }\n",
    "h2 { border-bottom: 2px solid #8884; padding-bottom: 0.2rem; margin-top: 2.5rem; }\n",
    "h3 { margin-top: 1.6rem; }\n",
    "h4 { margin: 1.4rem 0 0.2rem; }\n",
    "table { border-collapse: collapse; width: 100%; margin: 0.6rem 0 1.2rem; }\n",
    "th, td { text-align: left; padding: 0.35rem 0.6rem; border: 1px solid #8884; vertical-align: top; }\n",
    "th { background: #8882; }\n",
    "code { font-family: ui-monospace, monospace; }\n",
    ".flag { white-space: nowrap; }\n",
    ".required { font-weight: 600; }\n",
    ".note { background: #8881; padding: 0.6rem 0.9rem; border-radius: 0.4rem; }\n",
    "nav ol { padding-left: 1.2rem; }\n",
    "footer { margin-top: 3rem; color: #888; font-size: 0.9rem; border-top: 1px solid #8884; padding-top: 0.6rem; }\n",
    "</style>\n",
);

// ==========================================================================
// Shape walking
// ==========================================================================

/// One rendered argument row.
struct ArgRow {
    /// `--flag` or `<POSITIONAL>`.
    display: String,
    short: Option<char>,
    type_label: String,
    default: String,
    required: bool,
    doc: String,
}

fn struct_fields(shape: &'static Shape) -> &'static [Field] {
    match &shape.ty {
        Type::User(UserType::Struct(s)) => s.fields,
        _ => &[],
    }
}

fn is_positional(field: &Field) -> bool {
    field.has_attr(Some("args"), "positional")
}

fn is_subcommand(field: &Field) -> bool {
    field.has_attr(Some("args"), "subcommand")
}

/// Walk a struct's fields in declaration order, recursing into flattened
/// fields and skipping the subcommand selector.
fn collect_arg_rows(fields: &'static [Field], rows: &mut Vec<ArgRow>) {
    for field in fields {
        if is_subcommand(field) {
            continue;
        }
        if field.is_flattened() {
            collect_arg_rows(struct_fields(field.shape()), rows);
            continue;
        }
        if !is_positional(field) && !field.has_attr(Some("args"), "named") {
            continue;
        }
        rows.push(arg_row(field));
    }
}

fn arg_row(field: &Field) -> ArgRow {
    let name = kebab(field.effective_name());
    let positional = is_positional(field);
    let display = if positional {
        format!("<{}>", name.to_uppercase())
    } else {
        format!("--{name}")
    };
    let (type_label, default, required) = value_info(field);
    ArgRow {
        display,
        short: short_of(field),
        type_label,
        default,
        required,
        doc: doc_text(field.doc),
    }
}

/// Type label, default rendering and the required marker for one field.
fn value_info(field: &Field) -> (String, String, bool) {
    let mut shape = field.shape();
    let mut optional = false;
    if let Def::Option(_) = shape.def {
        optional = true;
        shape = inner_shape(shape);
    }
    let mut repeatable = false;
    if let Def::List(_) = shape.def {
        repeatable = true;
        shape = inner_shape(shape);
    }

    let mut label = last_segment(shape.type_identifier).to_uppercase();
    if optional {
        label.push_str(" (optional)");
    }
    if repeatable {
        label.push_str(" (repeatable)");
    }

    let counted = field.has_attr(Some("args"), "counted");
    let is_bool = last_segment(shape.type_identifier) == "bool";
    let required = !optional && field.default.is_none() && !is_bool && !counted;
    (label, render_default(field), required)
}

fn inner_shape(shape: &'static Shape) -> &'static Shape {
    match shape.def {
        Def::Option(option) => option.t,
        Def::List(list) => list.t,
        _ => shape,
    }
}

fn last_segment(identifier: &str) -> &str {
    identifier.rsplit("::").next().unwrap_or(identifier)
}

/// The `args::short` character, if any (mirrors figue's own extraction).
fn short_of(field: &Field) -> Option<char> {
    field
        .get_attr(Some("args"), "short")
        .and_then(|attr| attr.get_as::<Attr>())
        .and_then(|attr| match attr {
            Attr::Short(c) => c.or_else(|| field.effective_name().chars().next()),
            _ => None,
        })
}

/// Doc-comment lines joined into one paragraph.
fn doc_text(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// kebab-case conversion for the identifiers used in these shapes
/// (`manifest_path` -> `manifest-path`, `DumpDocs` -> `dump-docs`).
fn kebab(name: &str) -> String {
    if name.contains('_') {
        return name.replace('_', "-");
    }
    let mut out = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

// ==========================================================================
// Defaults
// ==========================================================================

/// Render the default value declared on a field.
///
/// `#[facet(default = ...)]` (Custom) and `#[facet(default)]` (FromTrait)
/// are evaluated exactly the way figue evaluates them before parsing: the
/// default function initializes a stack buffer, then the value is read
/// through its Display impl. Fields with no default render `unset` when
/// optional.
fn render_default(field: &Field) -> String {
    let Some(default_source) = field.default.as_ref() else {
        return if matches!(field.shape().def, Def::Option(_)) {
            "unset".to_string()
        } else {
            "\u{2014}".to_string()
        };
    };
    let shape = field.shape();
    if matches!(shape.def, Def::List(_)) {
        // Lists default to empty; Vec does not implement Display.
        return "(empty list)".to_string();
    }

    // All default types in these shapes (bool, usize, String, PathBuf) fit
    // in a small stack buffer.
    let mut storage = [0u8; 1024];
    let ptr = storage.as_mut_ptr().cast::<()>();
    match default_source {
        DefaultSource::FromTrait => {
            let Some(TypeOps::Direct(ops)) = shape.type_ops else {
                return "(type default)".to_string();
            };
            let Some(default_fn) = ops.default_in_place else {
                return "(type default)".to_string();
            };
            // SAFETY: `ptr` points to 1024 writable, sufficiently aligned
            // bytes; `default_fn` fully initializes a value of `shape` in
            // place, per its contract.
            unsafe { default_fn(ptr) };
        }
        DefaultSource::Custom(fn_ptr) => {
            let uninit = PtrUninit::new_sized(ptr);
            // SAFETY: same buffer, and the custom default fully initializes
            // a value of `shape` in place, per the facet derive contract.
            unsafe { (*fn_ptr)(uninit) };
        }
    }

    let ptr_const = PtrConst::new_sized(ptr as *const ());
    // SAFETY: the buffer was initialized above with a value matching
    // `shape`; the peek only reads it.
    let peek = unsafe { Peek::unchecked_new(ptr_const, shape) };
    // Prefer Display; some std shapes (e.g. PathBuf) only register Debug,
    // whose output quotes string-like values — trim the quotes back off.
    let rendered = if shape.vtable.has_display() {
        peek.to_string()
    } else {
        format!("{peek:?}").trim_matches('"').to_string()
    };

    // Heap-carrying defaults (String, PathBuf) must not leak; dropping via
    // the shape's type ops is uniform for every default type here.
    if let Some(TypeOps::Direct(ops)) = shape.type_ops {
        // SAFETY: the buffer holds an initialized value of `shape`,
        // as required by `drop_in_place`.
        unsafe { (ops.drop_in_place)(ptr) };
    }
    rendered
}

// ==========================================================================
// Sections
// ==========================================================================

fn render_binary_section(html: &mut String, program: &str, about: &str, shape: &'static Shape) {
    let id = kebab(program);
    html.push_str(&format!(
        "<section id=\"{id}\">\n<h2><code>{}</code></h2>\n<p>{}</p>\n",
        esc(program),
        esc(about),
    ));

    let fields = struct_fields(shape);
    let mut rows = Vec::new();
    collect_arg_rows(fields, &mut rows);
    html.push_str("<h3>Options</h3>\n");
    push_table(html, &rows);

    for field in fields {
        if !is_subcommand(field) {
            continue;
        }
        let variants = subcommand_variants(field);
        html.push_str("<h3>Subcommands</h3>\n");
        html.push_str(
            "<p>Global options are also accepted after the subcommand name.</p>\n",
        );
        for variant in variants {
            render_subcommand(html, program, variant);
        }
    }
    html.push_str("</section>\n");
}

fn subcommand_variants(field: &Field) -> &'static [Variant] {
    let shape = field.shape();
    let inner = match shape.def {
        Def::Option(option) => option.t,
        _ => shape,
    };
    match &inner.ty {
        Type::User(UserType::Enum(e)) => e.variants,
        _ => &[],
    }
}

fn render_subcommand(html: &mut String, program: &str, variant: &Variant) {
    let name = kebab(variant.effective_name());
    let mut rows = Vec::new();
    collect_arg_rows(variant.data.fields, &mut rows);

    html.push_str(&format!(
        "<h4 id=\"command-{name}\"><code>{} {}</code></h4>\n",
        esc(program),
        esc(&name),
    ));
    let doc = doc_text(variant.doc);
    if !doc.is_empty() {
        html.push_str(&format!("<p>{}</p>\n", esc(&doc)));
    }

    let mut positionals = String::new();
    for row in &rows {
        if row.display.starts_with('<') {
            positionals.push(' ');
            positionals.push_str(&row.display);
        }
    }
    let has_flags = rows.iter().any(|row| row.display.starts_with("--"));
    let flags = if has_flags { " [OPTIONS]" } else { "" };
    html.push_str(&format!(
        "<p class=\"note\">Usage: <code>{} {}{}{}</code></p>\n",
        esc(program),
        esc(&name),
        flags,
        esc(&positionals),
    ));
    push_table(html, &rows);
}

fn push_table(html: &mut String, rows: &[ArgRow]) {
    html.push_str(concat!(
        "<table>\n<thead><tr><th>Argument</th><th>Short</th><th>Value</th>",
        "<th>Default</th><th>Required</th><th>Description</th></tr></thead>\n<tbody>\n",
    ));
    for row in rows {
        let short = row.short.map(|c| c.to_string()).unwrap_or_default();
        let required = if row.required {
            "<span class=\"required\">yes</span>".to_string()
        } else {
            String::new()
        };
        html.push_str(&format!(
            concat!(
                "<tr><td class=\"flag\"><code>{}</code></td><td>{}</td>",
                "<td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            ),
            esc(&row.display),
            esc(&short),
            esc(&row.type_label),
            esc(&row.default),
            required,
            esc(&row.doc),
        ));
    }
    html.push_str("</tbody>\n</table>\n");
}

fn render_env_section(html: &mut String) {
    html.push_str("<section id=\"environment-variables\">\n<h2>Environment variables</h2>\n");
    html.push_str(concat!(
        "<p>Layered below CLI arguments: an explicit flag always wins over ",
        "an environment variable, which wins over the built-in default.</p>\n",
    ));
    html.push_str(concat!(
        "<table>\n<thead><tr><th>Variable</th><th>Applies to</th>",
        "<th>Overridden by</th><th>Description</th></tr></thead>\n<tbody>\n",
    ));

    // Shape-derived rows: RUST_KNOWLEDGE_INDEX_DIR is declared on the
    // IndexDirEnv shape via args::env_alias and layered by figue's env
    // layer at runtime (knowledge_index::config::resolve_index_dir).
    for field in struct_fields(IndexDirEnv::SHAPE) {
        for alias in env_aliases(field) {
            html.push_str(&format!(
                concat!(
                    "<tr><td class=\"flag\"><code>{}</code></td><td>both binaries</td>",
                    "<td><code>--index-dir</code></td><td>{}</td></tr>\n",
                ),
                esc(&alias),
                esc(&doc_text(field.doc)),
            ));
        }
    }

    // Non-shape environment variables, documented by hand because they are
    // consumed outside argv parsing: the log filter initializes tracing
    // before parsing (and --verbose short-circuits it), and the cargo
    // override is read engine-side (knowledge-index) when spawning cargo.
    for (name, applies, overridden, description) in CURATED_ENV_VARS {
        html.push_str(&format!(
            concat!(
                "<tr><td class=\"flag\"><code>{}</code></td><td>{}</td>",
                "<td>{}</td><td>{}</td></tr>\n",
            ),
            esc(name),
            esc(applies),
            esc(overridden),
            esc(description),
        ));
    }
    html.push_str("</tbody>\n</table>\n</section>\n");
}

/// Non-shape environment variables, documented by hand.
const CURATED_ENV_VARS: &[(&str, &str, &str, &str)] = &[
    (
        "RUST_KNOWLEDGE_LOG",
        "both binaries",
        "--verbose (rust-knowledge only)",
        concat!(
            "tracing env-filter for log output. Precedence: --verbose (forces ",
            "debug) > RUST_KNOWLEDGE_LOG > RUST_LOG > info.",
        ),
    ),
    (
        "RUST_LOG",
        "both binaries",
        "RUST_KNOWLEDGE_LOG, --verbose",
        "fallback tracing env-filter when RUST_KNOWLEDGE_LOG is unset.",
    ),
    (
        "RUST_KNOWLEDGE_CARGO",
        "knowledge-index (engine)",
        "nothing (no CLI flag)",
        concat!(
            "path to the cargo executable the engine uses for cargo metadata ",
            "and rustdoc generation; defaults to the cargo on PATH.",
        ),
    ),
];

/// Extract every `args::env_alias = \"...\"` declaration on a field.
fn env_aliases(field: &Field) -> Vec<String> {
    let mut aliases = Vec::new();
    for attr in field.attributes {
        if attr.ns == Some("args") && attr.key == "env_alias" {
            if let Some(alias) = attr.get_as::<&str>() {
                aliases.push(alias.to_string());
            }
        }
    }
    aliases
}

/// HTML-escape every piece of text rendered onto the page.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kebab_matches_the_real_flag_and_subcommand_names() {
        // The subcommand names below are the empirically verified figue
        // names; the flag names are frozen clap surface.
        for (raw, expected) in [
            ("manifest_path", "manifest-path"),
            ("index_dir", "index-dir"),
            ("rustdoc_scope", "rustdoc-scope"),
            ("prebuilt_rustdoc", "prebuilt-rustdoc"),
            ("source-kind", "source-kind"),
            ("Packages", "packages"),
            ("DumpDocs", "dump-docs"),
            ("Index", "index"),
            ("Search", "search"),
            ("Get", "get"),
            ("Symbol", "symbol"),
            ("Eval", "eval"),
            ("ConfigDocs", "config-docs"),
        ] {
            assert_eq!(kebab(raw), expected, "kebab({raw})");
        }
    }

    #[test]
    fn render_is_stable_and_complete() {
        let html = render();
        for needle in [
            "--manifest-path",
            "--index-dir",
            "dump-docs",
            "config-docs",
            "docs/config-reference.html",
            "RUST_KNOWLEDGE_INDEX_DIR",
            "RUST_KNOWLEDGE_CARGO",
            "&lt;QUERY&gt;",
        ] {
            assert!(html.contains(needle), "page should contain {needle}");
        }
        // Deterministic: two renders are byte-identical.
        assert_eq!(html, render());
    }
}
