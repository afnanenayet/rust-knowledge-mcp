use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::{GeneratedRustdocProvider, PrebuiltRustdocProvider};

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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(if cli.verbose { "debug" } else { "info" })
        .with_target(false)
        .compact()
        .init();

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
            format!("{}...", doc.text[..1200].trim_end())
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
