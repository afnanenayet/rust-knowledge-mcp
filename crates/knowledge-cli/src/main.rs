use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use knowledge_core::KnowledgeRetriever;
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::{GeneratedRustdocProvider, PrebuiltRustdocProvider};
use knowledge_index::telemetry::TelemetryOptions;

#[derive(Parser, Debug)]
#[command(
    name = "rust-knowledge",
    version,
    about = "Search documentation of the resolved Cargo dependency universe of a workspace"
)]
struct Cli {
    /// Path to a Cargo.toml manifest. Defaults to the current directory.
    #[arg(long, global = true)]
    manifest_path: Option<PathBuf>,

    /// Directory for the knowledge index. Defaults to <workspace>/.rust-knowledge
    #[arg(long, global = true)]
    index_dir: Option<PathBuf>,

    /// Verbose logging.
    #[arg(long, short = 'v', global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List the packages in the resolved Cargo dependency universe.
    Packages {
        /// Emit JSON (one object per line).
        #[arg(long)]
        json: bool,
    },

    /// Build the normalized documentation corpus and print it (debug view).
    DumpDocs {
        /// Only documents of this package (name or name@version).
        #[arg(long)]
        package: Option<String>,

        /// Which packages get rustdoc JSON generated.
        #[arg(long, default_value = "workspace")]
        rustdoc_scope: String,

        /// Read prebuilt rustdoc JSON artifacts from this directory instead
        /// of invoking cargo.
        #[arg(long)]
        prebuilt_rustdoc: Option<PathBuf>,

        /// Emit one JSON document per line.
        #[arg(long)]
        json: bool,
    },

    /// Build the persistent knowledge index for the workspace.
    Index {
        /// Which packages get rustdoc JSON generated.
        #[arg(long, default_value = "workspace")]
        rustdoc_scope: String,

        /// Toolchain used for rustdoc generation (default: nightly).
        #[arg(long)]
        toolchain: Option<String>,

        /// Read prebuilt rustdoc JSON artifacts from this directory instead
        /// of invoking cargo.
        #[arg(long)]
        prebuilt_rustdoc: Option<PathBuf>,
    },

    /// Search the knowledge index.
    Search {
        /// Free-form query: natural language or Rust identifiers.
        query: String,

        /// Restrict to these packages (name or name@version). Repeatable.
        #[arg(long = "package")]
        packages: Vec<String>,

        /// Restrict to source kinds (rustdoc_item, rustdoc_module,
        /// crate_readme, markdown_document). Repeatable.
        #[arg(long = "source-kind")]
        source_kinds: Vec<String>,

        /// Restrict to item kinds (function, struct, trait, ...). Repeatable.
        #[arg(long = "item-kind")]
        item_kinds: Vec<String>,

        /// Maximum number of results.
        #[arg(long, default_value = "8")]
        limit: usize,

        /// Emit JSON (one hit per line).
        #[arg(long)]
        json: bool,
    },

    /// Retrieve one document by its stable id.
    Get {
        /// Document id (as printed by search).
        id: String,

        /// Emit the full JSON document.
        #[arg(long)]
        json: bool,
    },

    /// Look up a symbol by (partial) path.
    Symbol {
        /// Symbol path or last segment, e.g. demo_core::writer::Writer::flush
        /// or spawn_blocking.
        symbol: String,

        /// Restrict to these packages (name or name@version). Repeatable.
        #[arg(long = "package")]
        packages: Vec<String>,

        /// Emit JSON (one entry per line).
        #[arg(long)]
        json: bool,
    },

    /// Run the retrieval evaluation set against the knowledge index.
    Eval {
        /// Path to the eval file (TOML, [[case]] entries).
        file: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Shared layered tracing init: logs go to stderr (stdout carries the
    // data output of --json modes); -v bumps the built-in default filter,
    // RUST_KNOWLEDGE_LOG / RUST_LOG still win over it.
    knowledge_index::telemetry::init(&TelemetryOptions {
        verbose: cli.verbose,
    });

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Packages { json } => packages(&cli, *json),
        Command::DumpDocs {
            package,
            rustdoc_scope,
            prebuilt_rustdoc,
            json,
        } => dump_docs(&cli, package, rustdoc_scope, prebuilt_rustdoc, *json),
        Command::Index {
            rustdoc_scope,
            toolchain,
            prebuilt_rustdoc,
        } => index(&cli, rustdoc_scope, toolchain, prebuilt_rustdoc),
        Command::Search {
            query,
            packages,
            source_kinds,
            item_kinds,
            limit,
            json,
        } => search(
            &cli,
            query,
            packages,
            source_kinds,
            item_kinds,
            *limit,
            *json,
        ),
        Command::Get { id, json } => get(&cli, id, *json),
        Command::Symbol {
            symbol,
            packages,
            json,
        } => symbol_lookup(&cli, symbol, packages, *json),
        Command::Eval { file } => eval(&cli, file),
    }
}

fn load_universe(cli: &Cli) -> anyhow::Result<CargoUniverse> {
    CargoUniverse::load(cli.manifest_path.as_deref())
        .context("failed to load the resolved Cargo universe")
}

fn packages(cli: &Cli, json: bool) -> anyhow::Result<()> {
    let universe = load_universe(cli)?;

    let mut rows: Vec<_> = universe
        .packages()
        .map(|p| {
            let identity = universe.identity(p);
            (
                identity.name.clone(),
                identity.version.clone(),
                universe.origin(p).as_str().to_string(),
                universe.enabled_features(&p.id).join(","),
                identity.manifest_path.clone(),
                identity.package_id.clone(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    if json {
        for (name, version, origin, features, manifest, package_id) in rows {
            println!(
                "{}",
                serde_json::json!({
                    "name": name,
                    "version": version,
                    "origin": origin,
                    "package_id": package_id,
                    "manifest_path": manifest,
                    "enabled_features": features,
                })
            );
        }
        return Ok(());
    }

    println!(
        "{:<24} {:<12} {:<10} {:<28} PATH",
        "PACKAGE", "VERSION", "ORIGIN", "FEATURES"
    );
    for (name, version, origin, features, manifest, _) in &rows {
        println!(
            "{:<24} {:<12} {:<10} {:<28} {}",
            name,
            version,
            origin,
            if features.is_empty() { "-" } else { features },
            manifest.display()
        );
    }
    Ok(())
}

fn parse_scope(name: &str) -> anyhow::Result<RustdocScope> {
    RustdocScope::parse(name).ok_or_else(|| {
        anyhow::anyhow!("unknown rustdoc scope {name:?}: use workspace, all or none")
    })
}

fn dump_docs(
    cli: &Cli,
    package: &Option<String>,
    rustdoc_scope: &str,
    prebuilt_rustdoc: &Option<PathBuf>,
    json: bool,
) -> anyhow::Result<()> {
    let universe = load_universe(cli)?;
    let scope = parse_scope(rustdoc_scope)?;
    let index_dir = index_dir(cli, &universe);

    let provider: Box<dyn knowledge_index::rustdoc::RustdocProvider> = match prebuilt_rustdoc {
        Some(dir) => Box::new(PrebuiltRustdocProvider { dir: dir.clone() }),
        None => Box::new(GeneratedRustdocProvider::new(
            &universe,
            index_dir.join("cache").join("rustdoc"),
            Some("nightly".to_string()),
        )),
    };

    let options = CorpusOptions {
        rustdoc_scope: scope,
    };
    let (documents, report) = build_corpus(&universe, provider.as_ref(), &options)
        .context("failed to build the documentation corpus")?;

    eprintln!(
        "# corpus: {} packages, {} rustdoc generated, {} markdown files, {} documents",
        report.packages,
        report.rustdoc_packages,
        report.markdown_files,
        documents.len()
    );
    for (spec, reason) in &report.skipped {
        eprintln!("# skipped {spec}: {reason}");
    }
    for warning in &report.warnings {
        eprintln!("# warning: {warning}");
    }

    let documents: Vec<_> = match package {
        None => documents,
        Some(spec) => {
            let pkg = universe.resolve_spec(spec).map_err(anyhow::Error::from)?;
            let id = &pkg.id.repr;
            documents
                .into_iter()
                .filter(|d| d.package.package_id == *id)
                .collect()
        }
    };

    for doc in &documents {
        if json {
            println!("{}", serde_json::to_string(doc)?);
            continue;
        }
        println!("--------------------------------------------------");
        println!("id: {}", doc.id);
        println!("from: {}", doc.provenance());
        println!("context: {}", doc.context());
        if let Some(sig) = &doc.signature {
            println!("signature: {sig}");
        }
        if let Some(span) = &doc.source_span
            && let Some(path) = &doc.source_path
        {
            println!(
                "source: {}:{}-{}",
                path.display(),
                span.start_line,
                span.end_line
            );
        }
        if !doc.related_symbols.is_empty() {
            println!("related: {}", doc.related_symbols.join(", "));
        }
        let text = if doc.text.len() > 1200 {
            let end = doc.text.floor_char_boundary(1200);
            format!("{}...", doc.text.get(..end).unwrap_or_default().trim_end())
        } else {
            doc.text.clone()
        };
        println!("{text}");
    }

    if documents.is_empty() && package.is_some() {
        eprintln!("# no documents matched");
    }
    Ok(())
}

fn index_dir(cli: &Cli, universe: &CargoUniverse) -> PathBuf {
    cli.index_dir
        .clone()
        .unwrap_or_else(|| universe.workspace_root().join(".rust-knowledge"))
}

fn index(
    cli: &Cli,
    rustdoc_scope: &str,
    toolchain: &Option<String>,
    prebuilt_rustdoc: &Option<PathBuf>,
) -> anyhow::Result<()> {
    let scope = parse_scope(rustdoc_scope)?;
    let options = knowledge_index::IndexOptions {
        rustdoc_scope: scope,
        toolchain: toolchain.clone().or_else(|| Some("nightly".to_string())),
        prebuilt_rustdoc: prebuilt_rustdoc.clone(),
        skip_rustdoc: false,
    };
    let outcome = knowledge_index::index_workspace(
        cli.manifest_path.as_deref(),
        cli.index_dir.as_deref(),
        &options,
    )
    .context("indexing failed")?;

    println!(
        "indexed {} packages into {} ({} documents)",
        outcome.corpus.packages,
        outcome.index_dir.display(),
        outcome.meta.document_count
    );
    for (spec, reason) in &outcome.meta.skipped {
        eprintln!("skipped {spec}: {reason}");
    }
    for warning in &outcome.meta.warnings {
        eprintln!("warning: {warning}");
    }
    println!("search it with: rust-knowledge search <query>");
    Ok(())
}

fn parse_source_kinds(raw: &[String]) -> anyhow::Result<Vec<knowledge_core::SourceKind>> {
    raw.iter()
        .map(|s| {
            s.parse::<knowledge_core::SourceKind>()
                .map_err(anyhow::Error::msg)
        })
        .collect()
}

fn open_retriever(cli: &Cli) -> anyhow::Result<knowledge_index::TantivyRetriever> {
    knowledge_index::open_retriever(cli.manifest_path.as_deref(), cli.index_dir.as_deref())
        .map_err(|e| anyhow::anyhow!("failed to open knowledge index: {e}"))
        .with_context(|| "run 'rust-knowledge index' first (or pass --index-dir / --manifest-path)")
}

fn search(
    cli: &Cli,
    query: &str,
    packages: &[String],
    source_kinds: &[String],
    item_kinds: &[String],
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    let retriever = open_retriever(cli)?;
    let query = knowledge_core::SearchQuery {
        text: query.to_string(),
        packages: packages.to_vec(),
        source_kinds: parse_source_kinds(source_kinds)?,
        item_kinds: item_kinds.to_vec(),
        limit,
    };
    let hits = retriever
        .search(&query)
        .map_err(|e| anyhow::anyhow!("search failed: {e}"))?;

    if json {
        for hit in hits {
            println!("{}", serde_json::to_string(&hit)?);
        }
        return Ok(());
    }

    for (n, hit) in hits.iter().enumerate() {
        println!("{}. {}@{}", n + 1, hit.package_name, hit.package_version);
        let context = match (&hit.symbol_path, hit.section_path.is_empty()) {
            (Some(symbol), _) => symbol.clone(),
            (None, false) => hit.section_path.join(" > "),
            (None, true) => hit.title.clone(),
        };
        println!("   {context}");
        println!("   [{}]", hit.source_kind);
        println!("   id: {}", hit.id);
        let text = hit.snippet.replace('\n', " ");
        println!();
        println!("   {text}");
        println!();
    }
    if hits.is_empty() {
        println!("no results");
    }
    Ok(())
}

fn get(cli: &Cli, id: &str, json: bool) -> anyhow::Result<()> {
    let retriever = open_retriever(cli)?;
    let document_id = knowledge_core::DocumentId::from_raw(id)
        .ok_or_else(|| anyhow::anyhow!("{id:?} is not a valid document id"))?;
    let doc = retriever
        .get(&document_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }

    println!("id: {}", doc.id);
    println!("from: {}", doc.provenance());
    println!("context: {}", doc.context());
    if let Some(sig) = &doc.signature {
        println!("signature: {sig}");
    }
    if let Some(path) = &doc.source_path
        && let Some(span) = &doc.source_span
    {
        println!(
            "source: {}:{}-{}",
            path.display(),
            span.start_line,
            span.end_line
        );
    }
    if !doc.related_symbols.is_empty() {
        println!("related: {}", doc.related_symbols.join(", "));
    }
    println!();
    println!("{}", doc.text);
    Ok(())
}

fn symbol_lookup(cli: &Cli, symbol: &str, packages: &[String], json: bool) -> anyhow::Result<()> {
    let retriever = open_retriever(cli)?;
    let query = knowledge_core::SymbolQuery {
        symbol: symbol.to_string(),
        packages: packages.to_vec(),
        limit: 10,
    };
    let infos = retriever
        .symbol_lookup(&query)
        .map_err(|e| anyhow::anyhow!("symbol lookup failed: {e}"))?;

    if json {
        for info in infos {
            println!("{}", serde_json::to_string(&info)?);
        }
        return Ok(());
    }

    for (n, info) in infos.iter().enumerate() {
        println!(
            "{}. {}@{} [{}] {}",
            n + 1,
            info.package_name,
            info.package_version,
            info.kind,
            info.symbol_path
        );
        if let Some(sig) = &info.signature {
            println!("   {sig}");
        }
        println!("   id: {}", info.id);
        let snippet = info.snippet.replace('\n', " ");
        println!("   {snippet}");
        println!();
    }
    if infos.is_empty() {
        println!("no symbol matched {symbol:?}");
    }
    Ok(())
}

fn eval(cli: &Cli, file: &PathBuf) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read eval file {}", file.display()))?;
    let eval_set = knowledge_index::eval::parse_set(&raw)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", file.display()))?;
    let retriever = open_retriever(cli)?;

    // Eval sets are corpus-specific: refuse to score one against a different
    // workspace instead of producing meaningless numbers.
    if let Some(corpus) = &eval_set.corpus {
        let root = retriever.meta().workspace_root.display().to_string();
        anyhow::ensure!(
            root.ends_with(corpus.trim_end_matches('/')),
            "this eval file targets the {corpus:?} workspace; the current index              was built for {root:?}. Point --index-dir/--manifest-path at the              matching workspace."
        );
    }

    let outcomes = knowledge_index::eval::run_eval(&retriever, eval_set.cases);
    let summary = knowledge_index::eval::summarize(&outcomes);

    for outcome in &outcomes {
        let status = if outcome.passed() { "PASS" } else { "FAIL" };
        let rank = outcome
            .rank
            .map(|r| r.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{status} rank {rank:>2} {:>16} {:?}",
            outcome.case.category, outcome.case.text
        );
        if !outcome.passed() {
            println!(
                "     expected one of {:?}, top: {:?}",
                outcome.case.expect_any, outcome.top
            );
        }
    }
    println!(
        "{}/{} passed, MRR {:.3}",
        summary.passed, summary.total, summary.mrr
    );
    if summary.passed != summary.total {
        std::process::exit(1);
    }
    Ok(())
}
