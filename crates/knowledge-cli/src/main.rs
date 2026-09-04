use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use clap::{Parser, Subcommand};
use knowledge_index::CargoUniverse;

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
    match cli.command {
        Command::Packages { json } => packages(&cli, json),
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
                identity,
                universe.enabled_features(&p.id).join(","),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    if json {
        for (name, version, origin, identity, features) in rows {
            println!(
                "{}",
                serde_json::json!({
                    "name": name,
                    "version": version,
                    "origin": origin,
                    "package_id": identity.package_id,
                    "manifest_path": identity.manifest_path,
                    "source": identity.source,
                    "enabled_features": features,
                })
            );
        }
        return Ok(());
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<24} {:<12} {:<10} {:<28} PATH\n",
        "PACKAGE", "VERSION", "ORIGIN", "FEATURES"
    ));
    for (name, version, origin, _identity, features) in &rows {
        out.push_str(&format!(
            "{:<24} {:<12} {:<10} {:<28} {}\n",
            name,
            version,
            origin,
            if features.is_empty() { "-" } else { features },
            _identity.manifest_path.display()
        ));
    }
    print!("{out}");
    Ok(())
}
