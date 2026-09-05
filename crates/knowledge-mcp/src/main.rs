//! Binary entry point: serves the knowledge MCP over stdio.
//!
//! Logs go to stderr (stdout is the MCP protocol channel).
//!
//! Typical client configuration:
//! {"mcpServers": {"rust-knowledge": {"command": "knowledge-mcp",
//!   "args": ["--manifest-path", "/path/to/workspace/Cargo.toml"]}}}
//!
//! Arguments are parsed by figue over a facet shape (issue #2): the
//! flattened config root layers CLI flags over the RUST_KNOWLEDGE_* env
//! vars over defaults with figue's own precedence, and
//! figue::FigueBuiltins contributes --help/--version and friends.

use std::process::ExitCode;

use knowledge_index::config::{
    MCP_DESCRIPTION,
    McpArgs,
    effective_log_filter,
    parse_std_args,
};
use knowledge_mcp::KnowledgeServer;
use rmcp::service::serve_server;
use rmcp::transport::stdio;

fn main() -> ExitCode {
    // Parse first: the log filter itself comes from figue's config layer
    // (--log > $RUST_KNOWLEDGE_LOG > $RUST_LOG > default), and parse
    // diagnostics go to stderr, so stdout only ever carries protocol
    // traffic. Logging inits right after the parse; nothing is logged
    // before that, and tracing macros without a subscriber are no-ops.
    // DriverOutcome::unwrap is figue's native outcome handling: help,
    // version, completions and schemas print to stdout and exit 0;
    // diagnostics print to stderr and exit 1 (git-like-multitool recipe).
    let cli =
        parse_std_args::<McpArgs>("knowledge-mcp", env!("CARGO_PKG_VERSION"), MCP_DESCRIPTION)
            .unwrap();

    tracing_subscriber::fmt()
        .with_env_filter(effective_log_filter(false, &cli.config.log))
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();

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

    if let Err(e) = runtime.block_on(run(cli.config.manifest_path, cli.config.index_dir)) {
        eprintln!("error: {e:#}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

async fn run(
    manifest_path: Option<std::path::PathBuf>,
    index_dir: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
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
