//! Binary entry point: serves the knowledge MCP over stdio.
//!
//! Logs go to stderr (stdout is the MCP protocol channel).
//!
//! Typical client configuration:
//! {"mcpServers": {"rust-knowledge": {"command": "knowledge-mcp",
//!   "args": ["--manifest-path", "/path/to/workspace/Cargo.toml"]}}}

use std::process::ExitCode;

use figue::DriverError;
use knowledge_index::config::{
    McpArgs,
    parse_std_args,
    resolve_index_dir,
    resolve_log_filter,
    std_argv_requests_help,
};
use knowledge_mcp::KnowledgeServer;
use rmcp::service::serve_server;
use rmcp::transport::stdio;

fn main() -> ExitCode {
    // MCP speaks JSON-RPC on stdout; everything else must go to stderr.
    // Logging initializes before parsing so parse errors are logged to
    // stderr only, never onto the protocol channel.
    tracing_subscriber::fmt()
        .with_env_filter(resolve_log_filter(false))
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();

    let mut cli = match parse_std_args::<McpArgs>(
        "knowledge-mcp",
        env!("CARGO_PKG_VERSION"),
        "MCP server exposing the rust-knowledge retrieval engine",
    )
    .into_result()
    {
        Ok(output) => output.get(),
        Err(DriverError::Help { text, suggestion }) => {
            // Exit-code discipline matches clap: an explicit --help exits 0
            // on stdout; figue's missing-required-fields help exits 2 on
            // stderr. McpArgs has no required fields, so in practice only
            // a genuine --help takes this arm.
            let text = text.trim_end_matches('\n');
            if std_argv_requests_help() {
                println!("{text}");
                if let Some(suggestion) = suggestion {
                    println!("{}", suggestion.render_pretty());
                }
                return ExitCode::SUCCESS;
            }
            eprintln!("{text}");
            if let Some(suggestion) = suggestion {
                eprintln!("{}", suggestion.render_pretty());
            }
            return ExitCode::from(2);
        }
        Err(DriverError::Version { text }) => {
            println!("{}", text.trim_end_matches('\n'));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    // Layered resolution: --index-dir > RUST_KNOWLEDGE_INDEX_DIR > engine
    // default (<workspace>/.rust-knowledge).
    cli.index_dir = resolve_index_dir(cli.index_dir.take());

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

    if let Err(e) = runtime.block_on(run(cli.manifest_path, cli.index_dir)) {
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
