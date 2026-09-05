//! In-process MCP integration tests: a real client (over an in-memory duplex
//! transport) talks to the real server backed by a fixture index. Verifies
//! the three tools, their compact JSON payloads, and error semantics.

use std::path::{Path, PathBuf};

use knowledge_core::SourceKind;
use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;
use knowledge_index::store::IndexMeta;
use knowledge_index::tantivy_index::{TantivyRetriever, build_index};
use knowledge_mcp::KnowledgeServer;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::{ClientHandler, RoleClient, ServiceExt, service::RunningService};
use serde_json::{Value, json};

struct NoopClient;
impl ClientHandler for NoopClient {}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

fn fixture_retriever() -> TantivyRetriever {
    let universe =
        CargoUniverse::load(Some(&fixture_dir().join("Cargo.toml"))).expect("cargo metadata");
    let provider = PrebuiltRustdocProvider {
        dir: fixture_dir().join("prebuilt-rustdoc"),
    };
    let (documents, report) = build_corpus(
        &universe,
        &provider,
        &CorpusOptions {
            rustdoc_scope: RustdocScope::All,
        },
    )
    .expect("corpus");
    let dir = tempfile::tempdir().expect("tempdir");
    let meta = IndexMeta {
        schema_version: IndexMeta::supported_schema(),
        workspace_root: universe.workspace_root().to_path_buf(),
        lock_hash: universe.lock_hash(),
        metadata_fingerprint: universe.fingerprint(),
        cargo_version: report.cargo_version.clone(),
        toolchain: None,
        rustdoc_format_version: report.rustdoc_format_version,
        rustdoc_scope: "All".into(),
        package_count: report.packages,
        document_count: documents.len(),
        built_at: "test".into(),
        skipped: Vec::new(),
        warnings: Vec::new(),
    };
    build_index(dir.path(), &documents, &meta).expect("build index");
    let retriever = TantivyRetriever::open(dir.path()).expect("open index");
    #[expect(clippy::mem_forget, reason = "leak the TempDir for the test's lifetime")]
    std::mem::forget(dir);
    retriever
}

async fn serve(retriever: TantivyRetriever) -> RunningService<RoleClient, NoopClient> {
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let server = KnowledgeServer::new(retriever);
    let server_task =
        tokio::spawn(async move { server.serve(server_transport).await.expect("server serves") });
    let client = NoopClient
        .serve(client_transport)
        .await
        .expect("client serves");
    // Keep the server task alive for the lifetime of the test.
    #[expect(clippy::mem_forget, reason = "keep the server task alive for the test")]
    std::mem::forget(server_task);
    client
}

fn tool_params(name: &str, arguments: Value) -> CallToolRequestParams {
    serde_json::from_value(json!({
        "name": name,
        "arguments": arguments,
    }))
    .expect("params")
}

fn json_of(result: &CallToolResult) -> Value {
    assert!(
        !result.is_error.unwrap_or(false),
        "tool call failed: {:?}",
        result.content
    );
    let text = result
        .content
        .first()
        .and_then(|c| match c {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .expect("expected text content");
    serde_json::from_str(text).expect("json payload")
}

#[tokio::test]
async fn exposes_exactly_the_three_tools() {
    let client = serve(fixture_retriever()).await;
    let tools = client.list_tools(None).await.expect("list tools");
    let mut names: Vec<String> = tools.tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        vec!["doc_read", "knowledge_search", "symbol_lookup"],
        "tool set: {names:?}"
    );
    // Tool descriptions must guide the agent workflow.
    for tool in &tools.tools {
        let description = tool.description.clone().unwrap_or_default();
        assert!(description.len() > 40, "tool {} lacks guidance", tool.name);
    }
    client.cancel().await.expect("cancel");
}

#[tokio::test]
async fn knowledge_search_returns_compact_hits() {
    let client = serve(fixture_retriever()).await;
    let result = client
        .call_tool(tool_params(
            "knowledge_search",
            json!({"query": "write_all"}),
        ))
        .await
        .expect("call");

    let payload = json_of(&result);
    let results = payload["results"].as_array().expect("results array");
    assert!(!results.is_empty());
    let top = &results[0];
    assert_eq!(top["package"], "demo-core");
    assert_eq!(top["context"], "demo_core::writer::Writer::write_all");
    assert!(top["id"].as_str().unwrap().len() == 32);
    assert!(
        top["snippet"].as_str().unwrap().len() <= 320,
        "snippets must stay compact"
    );
    client.cancel().await.expect("cancel");
}

#[tokio::test]
async fn knowledge_search_supports_filters() {
    let client = serve(fixture_retriever()).await;
    let result = client
        .call_tool(tool_params(
            "knowledge_search",
            json!({"query": "engine encode", "packages": ["base64@0.22.1"]}),
        ))
        .await
        .expect("call");
    let payload = json_of(&result);
    for hit in payload["results"].as_array().expect("results") {
        assert_eq!(hit["version"], "0.22.1");
    }

    // Invalid source kinds produce a friendly error result, not a crash.
    let result = client
        .call_tool(tool_params(
            "knowledge_search",
            json!({"query": "x", "source_kinds": ["nope"]}),
        ))
        .await
        .expect("call");
    assert!(result.is_error.unwrap_or(false));
    client.cancel().await.expect("cancel");
}

#[tokio::test]
async fn doc_read_round_trips_full_text() {
    let client = serve(fixture_retriever()).await;
    let search = client
        .call_tool(tool_params(
            "knowledge_search",
            json!({"query": "offload_blocking"}),
        ))
        .await
        .expect("call");
    let hits = json_of(&search)["results"]
        .as_array()
        .expect("results")
        .clone();
    let id = hits
        .iter()
        .find(|h| h["context"] == "demo_core::runtime::offload_blocking")
        .expect("offload hit")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let result = client
        .call_tool(tool_params("doc_read", json!({"id": id})))
        .await
        .expect("call");
    let payload = json_of(&result);
    assert_eq!(payload["package"], "demo-core");
    assert_eq!(
        payload["symbol_path"],
        "demo_core::runtime::offload_blocking"
    );
    assert!(
        payload["text"]
            .as_str()
            .unwrap()
            .contains("Runs the provided closure")
    );
    assert!(payload["signature"].as_str().unwrap().contains("pub fn"));
    assert!(payload["source_span"].is_object());
    assert!(
        payload["source_path"]
            .as_str()
            .unwrap()
            .contains("demo-core")
    );
    client.cancel().await.expect("cancel");
}

#[tokio::test]
async fn doc_read_unknown_id_is_a_friendly_error() {
    let client = serve(fixture_retriever()).await;
    let result = client
        .call_tool(tool_params(
            "doc_read",
            json!({"id": "ffffffffffffffffffffffffffffffff"}),
        ))
        .await
        .expect("call");
    assert!(result.is_error.unwrap_or(false));
    client.cancel().await.expect("cancel");
}

#[tokio::test]
async fn symbol_lookup_returns_api_info() {
    let client = serve(fixture_retriever()).await;
    let result = client
        .call_tool(tool_params(
            "symbol_lookup",
            json!({"symbol": "Writer::write_all"}),
        ))
        .await
        .expect("call");
    let payload = json_of(&result);
    let results = payload["results"].as_array().expect("results");
    assert_eq!(
        results[0]["symbol_path"],
        "demo_core::writer::Writer::write_all"
    );
    assert_eq!(results[0]["kind"], "function");
    assert!(
        results[0]["signature"]
            .as_str()
            .unwrap()
            .contains("write_all")
    );
    client.cancel().await.expect("cancel");
}

// Keep the SourceKind import used: mirrors the server-side filter contract.
#[expect(dead_code, reason = "keeps the SourceKind import exercised")]
fn source_kind_round_trip() -> SourceKind {
    "rustdoc_item".parse().expect("kind")
}
