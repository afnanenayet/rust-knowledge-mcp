//! Binary entry point: serves the knowledge MCP over stdio.
//!
//! Logs go to stderr (stdout is the MCP protocol channel).
//!
//! Typical client configuration:
//! {"mcpServers": {"rust-knowledge": {"command": "knowledge-mcp",
//!   "args": ["--manifest-path", "/path/to/workspace/Cargo.toml"]}}}

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
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
    /// Path to the workspace Cargo.toml the index was built for.
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

async fn run(manifest_path: Option<PathBuf>, index_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let retriever = knowledge_index::open_retriever(
        manifest_path.as_deref(),
        index_dir.as_deref(),
    )
    .map_err(|e| anyhow::anyhow!(
        "failed to open the knowledge index: {e}. Build it first with          'rust-knowledge index' (or pass --index-dir)."
    ))?;
    tracing::info!(
        index = retriever.meta().workspace_root.display().to_string(),
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
