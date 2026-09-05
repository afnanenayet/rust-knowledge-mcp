//! Binary entry point: serves the knowledge MCP over stdio.
//!
//! Logs go to stderr (stdout is the MCP protocol channel).
//!
//! Register the server once, globally, with no arguments: the workspace to
//! serve is inferred from the working directory (nearest `Cargo.toml`
//! ancestor; a member manifest resolves to its owning workspace), so
//! clients that launch stdio servers with cwd set to the project directory
//! need no per-repo configuration:
//!
//! {"mcpServers": {"rust-knowledge": {"command": "<path-to>/knowledge-mcp"}}}
//!
//! Explicit `--manifest-path` / `--index-dir` override the inference and
//! stay supported for clients that cannot set cwd; see the README for the
//! full contract.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use knowledge_core::KnowledgeError;
use knowledge_index::IndexError;
use knowledge_mcp::KnowledgeServer;
use rmcp::service::serve_server;
use rmcp::transport::stdio;

#[derive(Parser, Debug)]
#[command(
    name = "knowledge-mcp",
    version,
    about = "MCP server exposing the rust-knowledge retrieval engine"
)]
struct Cli {
    /// Path to the workspace Cargo.toml the index was built for. Defaults
    /// to inferring the workspace from the current working directory.
    #[arg(long, global = true)]
    manifest_path: Option<PathBuf>,

    /// Knowledge index directory. Defaults to <workspace>/.rust-knowledge
    /// or RUST_KNOWLEDGE_INDEX_DIR.
    #[arg(long, global = true)]
    index_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    // MCP speaks JSON-RPC on stdout; everything else must go to stderr.
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_KNOWLEDGE_LOG").unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let index_dir = cli.index_dir.or_else(|| {
        std::env::var("RUST_KNOWLEDGE_INDEX_DIR")
            .ok()
            .map(PathBuf::from)
    });

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: failed to start tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = runtime.block_on(run(cli.manifest_path, index_dir)) {
        eprintln!("error: {e:#}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// How the manifest was determined for this run (startup log context).
enum ManifestSource {
    Explicit,
    InferredFromCwd,
    UnusedExplicitIndexDir,
}

impl ManifestSource {
    fn as_str(&self) -> &'static str {
        match self {
            ManifestSource::Explicit => "explicit --manifest-path",
            ManifestSource::InferredFromCwd => "inferred from cwd",
            ManifestSource::UnusedExplicitIndexDir => "unused (explicit index dir)",
        }
    }
}

async fn run(manifest_path: Option<PathBuf>, index_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let index_dir_explicit = index_dir.is_some();

    // Resolve which workspace to serve. Precedence: --manifest-path beats cwd
    // inference; an explicit index dir (--index-dir or RUST_KNOWLEDGE_INDEX_DIR)
    // makes the manifest irrelevant — cargo metadata never runs on that path,
    // so the working directory does not matter there.
    let (manifest, manifest_source) = match (&manifest_path, index_dir_explicit) {
        (_, true) => (None, ManifestSource::UnusedExplicitIndexDir),
        (Some(path), _) => (Some(path.clone()), ManifestSource::Explicit),
        (None, false) => {
            let cwd = std::env::current_dir().map_err(|e| {
                anyhow::anyhow!(
                    "failed to determine the current working directory: {e}. \
                    Pass --manifest-path /path/to/workspace/Cargo.toml to serve a \
                    specific workspace."
                )
            })?;
            match knowledge_index::nearest_manifest(&cwd) {
                Some(manifest) => (Some(manifest), ManifestSource::InferredFromCwd),
                None => {
                    return Err(anyhow::anyhow!(
                        "no Cargo.toml at or above the current working directory \
                        ({}); cannot infer which workspace to serve. Pass \
                        --manifest-path /path/to/workspace/Cargo.toml, or launch \
                        knowledge-mcp with its working directory inside the workspace \
                        (stdio MCP clients such as Claude Code set cwd to the project \
                        directory).",
                        cwd.display()
                    ));
                }
            }
        }
    };

    let resolved = knowledge_index::resolve_index(manifest.as_deref(), index_dir.as_deref())
        .map_err(|e| anyhow::anyhow!("failed to resolve the workspace to serve: {e}"))?;

    // Open the already-resolved index dir directly; re-calling
    // open_retriever here would run resolution a second time.
    let retriever = resolved
        .open()
        .map_err(|e| index_open_error(e, &resolved, manifest.as_deref(), index_dir_explicit))?;

    // cargo metadata did not run when the index dir was explicit; the root
    // recorded at build time is the best context available in that case.
    let workspace_root = resolved
        .workspace_root
        .clone()
        .unwrap_or_else(|| retriever.meta().workspace_root.clone());
    tracing::info!(
        workspace_root = %workspace_root.display(),
        index_dir = %resolved.index_dir.display(),
        manifest = manifest_source.as_str(),
        index_dir_source = if index_dir_explicit {
            "explicit (--index-dir or RUST_KNOWLEDGE_INDEX_DIR)"
        } else {
            "workspace default (<workspace>/.rust-knowledge)"
        },
        documents = retriever.meta().document_count,
        "serving knowledge MCP on stdio"
    );

    let server = KnowledgeServer::new(retriever);
    let service = serve_server(server, stdio())
        .await
        .map_err(|e| anyhow::anyhow!("failed to start MCP server: {e:?}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("server task failed: {e}"))?;
    Ok(())
}

/// Maps index-open failures to actionable messages. The missing-index case
/// names the workspace, the expected index directory, and the exact rebuild
/// command; every other failure passes through with the index dir named.
fn index_open_error(
    e: IndexError,
    resolved: &knowledge_index::ResolvedIndex,
    manifest: Option<&Path>,
    index_dir_explicit: bool,
) -> anyhow::Error {
    let index_dir = resolved.index_dir.display();
    match e {
        IndexError::Knowledge(KnowledgeError::NoIndex { .. }) if index_dir_explicit => {
            anyhow::anyhow!(
                "no knowledge index at {index_dir} (set via --index-dir or \
                RUST_KNOWLEDGE_INDEX_DIR). Build one with `rust-knowledge index \
                --index-dir {index_dir}` from inside the workspace, or point the \
                flag at an existing index."
            )
        }
        IndexError::Knowledge(KnowledgeError::NoIndex { .. }) => {
            let root = resolved
                .workspace_root
                .as_deref()
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_string());
            let rebuild = match manifest {
                Some(path) => format!("rust-knowledge index --manifest-path {}", path.display()),
                None => "rust-knowledge index".to_string(),
            };
            anyhow::anyhow!(
                "no knowledge index for workspace {root}: expected index directory \
                {index_dir}. Build it first with `{rebuild}`, then restart this MCP \
                server (or point --index-dir / RUST_KNOWLEDGE_INDEX_DIR at an \
                existing index)."
            )
        }
        other => anyhow::anyhow!("failed to open the knowledge index at {index_dir}: {other}"),
    }
}
