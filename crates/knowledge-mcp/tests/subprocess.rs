//! Subprocess MCP integration tests: launch the real `knowledge-mcp` binary
//! the way a globally registered client does — zero arguments, working
//! directory inside the workspace — and talk MCP over its stdio with the
//! rmcp client. Also pins the two fail-fast error contracts (no workspace
//! inferable; workspace without an index) and the explicit `--index-dir` /
//! `RUST_KNOWLEDGE_INDEX_DIR` paths, which must stay cargo-free.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use knowledge_index::CargoUniverse;
use knowledge_index::corpus::{CorpusOptions, RustdocScope, build_corpus};
use knowledge_index::rustdoc::PrebuiltRustdocProvider;
use knowledge_index::store::IndexMeta;
use knowledge_index::tantivy_index::build_index;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use rmcp::{ClientHandler, RoleClient, ServiceExt, service::RunningService};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// A hang anywhere (child or protocol) must fail the test, never block the
/// suite; generous because the child resolves cargo metadata first.
const STEP_TIMEOUT: Duration = Duration::from_secs(90);

struct NoopClient;
impl ClientHandler for NoopClient {}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace")
}

/// Builds the fixture corpus (committed prebuilt rustdoc; no nightly) into
/// the given directory, exactly the way `rust-knowledge index` would.
fn build_fixture_index(dir: &Path) {
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
    build_index(dir, &documents, &meta).expect("build index");
}

/// Removes the fixture's default index directory on drop, so even a
/// failing test cannot leave a stale index behind for the next run.
struct FixtureIndexCleanup(PathBuf);

impl Drop for FixtureIndexCleanup {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// Spawns the built binary with a scrubbed environment: host
/// `RUST_KNOWLEDGE_*` settings must not skew the child's resolution.
fn spawn_server(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_knowledge-mcp"));
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("RUST_KNOWLEDGE_INDEX_DIR")
        .env_remove("RUST_KNOWLEDGE_CARGO");
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.spawn().expect("spawn knowledge-mcp")
}

/// Connects the rmcp client to a freshly spawned server over its stdio
/// (client reads child stdout, writes child stdin).
async fn connect(mut child: Child) -> (RunningService<RoleClient, NoopClient>, Child) {
    let stdin = child.stdin.take().expect("child stdin piped");
    let stdout = child.stdout.take().expect("child stdout piped");
    let client = NoopClient
        .serve((stdout, stdin))
        .await
        .expect("client completes MCP handshake");
    (client, child)
}

/// Drains the child's stderr after it has exited and reaps it.
async fn collect_exit(mut child: Child) -> (String, std::process::ExitStatus) {
    let mut stderr = child.stderr.take().expect("child stderr piped");
    let mut text = String::new();
    stderr
        .read_to_string(&mut text)
        .await
        .expect("read child stderr");
    let status = child.wait().await.expect("reap child");
    (text, status)
}

fn tool_params(name: &str, arguments: Value) -> CallToolRequestParams {
    serde_json::from_value(json!({
        "name": name,
        "arguments": arguments,
    }))
    .expect("params")
}

fn search_hits(result: &CallToolResult) -> Vec<Value> {
    assert!(
        !result.is_error.unwrap_or(false),
        "search call failed: {:?}",
        result.content
    );
    let text = result
        .content
        .first()
        .and_then(|c| match c {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .expect("text content");
    serde_json::from_str::<Value>(text).expect("json payload")["results"]
        .as_array()
        .expect("results array")
        .clone()
}

/// The unique random suffix of a tempdir: the only part of its path that
/// survives the kernel canonicalizing the temp root (e.g. /tmp ->
/// /private/tmp) between this process and the child's reported cwd.
fn dir_token(dir: &Path) -> String {
    dir.file_name()
        .expect("tempdir basename")
        .to_string_lossy()
        .to_string()
}

/// Zero arguments, cwd inside a member crate of the fixture workspace: the
/// server must infer the workspace (member manifest resolves to its owning
/// workspace), default the index to `<workspace>/.rust-knowledge`, serve
/// searches, and log the inference at startup.
#[tokio::test]
async fn zero_args_infers_the_workspace_from_a_member_subdir() {
    // The default index location is a shared, gitignored directory: build
    // it fresh and remove it on unwind. This is the only test in this
    // binary that touches it.
    let index_dir = fixture_dir().join(".rust-knowledge");
    drop(std::fs::remove_dir_all(&index_dir));
    let _cleanup = FixtureIndexCleanup(index_dir.clone());
    build_fixture_index(&index_dir);

    let child = spawn_server(&[], &fixture_dir().join("crates/demo-app"), &[]);
    let stderr = tokio::time::timeout(STEP_TIMEOUT, async {
        let (client, child) = connect(child).await;
        let tools = client.list_tools(None).await.expect("list tools");
        assert_eq!(tools.tools.len(), 3, "tool set: {:?}", tools.tools);

        let result = client
            .call_tool(tool_params(
                "knowledge_search",
                json!({"query": "write_all"}),
            ))
            .await
            .expect("search call");
        let hits = search_hits(&result);
        let top = hits.first().expect("top hit");
        assert_eq!(top["package"], "demo-core", "hits: {hits:?}");

        client.cancel().await.expect("cancel client");
        let (stderr, status) = collect_exit(child).await;
        assert!(status.success(), "server must exit cleanly: {stderr}");
        stderr
    })
    .await
    .expect("zero-arg server answers within timeout");

    assert!(
        stderr.contains("inferred from cwd"),
        "startup log must mark the inference: {stderr}"
    );
    assert!(
        stderr.contains(".rust-knowledge"),
        "startup log must name the index dir: {stderr}"
    );
    assert!(
        stderr.contains("fixtures/demo-workspace"),
        "startup log must name the workspace root: {stderr}"
    );
}

/// Failure mode A: no `Cargo.toml` at or above cwd → refuse to serve, name
/// the cwd, and show both fixes. Must exit promptly rather than hang.
#[tokio::test]
async fn no_manifest_ancestor_fails_fast_with_the_fix() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let child = spawn_server(&[], cwd.path(), &[]);
    let (stderr, status) = tokio::time::timeout(STEP_TIMEOUT, collect_exit(child))
        .await
        .expect("server must exit promptly instead of hanging");
    assert!(
        !status.success(),
        "server must refuse to serve: {stderr}"
    );
    assert!(
        stderr.contains("Cargo.toml"),
        "must name the missing manifest: {stderr}"
    );
    assert!(
        stderr.contains("--manifest-path"),
        "must show the manifest fix: {stderr}"
    );
    assert!(
        stderr.contains(&dir_token(cwd.path())),
        "must name the working directory: {stderr}"
    );
}

/// Failure mode B: workspace found, index missing → name the workspace, the
/// expected index dir, and the exact `rust-knowledge index` rebuild command.
#[tokio::test]
async fn workspace_without_index_names_the_rebuild_command() {
    let workspace = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(workspace.path().join("src")).expect("src dir");
    std::fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname = \"demo-empty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("manifest");
    std::fs::write(
        workspace.path().join("src").join("lib.rs"),
        "// minimal demo crate\n",
    )
    .expect("lib.rs");

    let child = spawn_server(&[], workspace.path(), &[]);
    let (stderr, status) = tokio::time::timeout(STEP_TIMEOUT, collect_exit(child))
        .await
        .expect("server must exit promptly instead of hanging");
    assert!(
        !status.success(),
        "server must refuse to serve: {stderr}"
    );
    assert!(
        stderr.contains(".rust-knowledge"),
        "must name the expected index dir: {stderr}"
    );
    assert!(
        stderr.contains("rust-knowledge index --manifest-path"),
        "must give the exact rebuild command: {stderr}"
    );
    assert!(
        stderr.contains(&dir_token(workspace.path())),
        "must name the workspace root and manifest: {stderr}"
    );
}

/// Explicit `--index-dir` keeps working and stays cargo-free: served from a
/// cwd with no workspace at all, and the flag must win over a bogus
/// `RUST_KNOWLEDGE_INDEX_DIR` inherited from the environment.
#[tokio::test]
async fn explicit_index_dir_serves_from_a_foreign_cwd() {
    let index = tempfile::tempdir().expect("index tempdir");
    build_fixture_index(index.path());
    let cwd = tempfile::tempdir().expect("foreign cwd");
    let index_arg = index
        .path()
        .to_str()
        .expect("index path is utf-8")
        .to_string();

    let child = spawn_server(
        &["--index-dir", index_arg.as_str()],
        cwd.path(),
        &[("RUST_KNOWLEDGE_INDEX_DIR", "/definitely/not/an/index")],
    );
    let stderr = tokio::time::timeout(STEP_TIMEOUT, async {
        let (client, child) = connect(child).await;
        let tools = client.list_tools(None).await.expect("list tools");
        assert_eq!(tools.tools.len(), 3, "tool set: {:?}", tools.tools);

        let result = client
            .call_tool(tool_params(
                "knowledge_search",
                json!({"query": "write_all"}),
            ))
            .await
            .expect("search call");
        assert!(!search_hits(&result).is_empty(), "expected hits");

        client.cancel().await.expect("cancel client");
        let (stderr, status) = collect_exit(child).await;
        assert!(status.success(), "server must exit cleanly: {stderr}");
        stderr
    })
    .await
    .expect("explicit-index server answers within timeout");

    assert!(
        !stderr.contains("resolved cargo universe"),
        "cargo metadata must never run with an explicit index dir: {stderr}"
    );
}

/// `RUST_KNOWLEDGE_INDEX_DIR` alone (no flag, no manifest) also serves from
/// a foreign cwd — the env-var half of the override story.
#[tokio::test]
async fn index_dir_env_var_serves_from_a_foreign_cwd() {
    let index = tempfile::tempdir().expect("index tempdir");
    build_fixture_index(index.path());
    let cwd = tempfile::tempdir().expect("foreign cwd");
    let index_env = index
        .path()
        .to_str()
        .expect("index path is utf-8")
        .to_string();

    let child = spawn_server(&[], cwd.path(), &[("RUST_KNOWLEDGE_INDEX_DIR", index_env.as_str())]);
    tokio::time::timeout(STEP_TIMEOUT, async {
        let (client, child) = connect(child).await;
        let tools = client.list_tools(None).await.expect("list tools");
        assert_eq!(tools.tools.len(), 3, "tool set: {:?}", tools.tools);

        let result = client
            .call_tool(tool_params(
                "knowledge_search",
                json!({"query": "write_all"}),
            ))
            .await
            .expect("search call");
        assert!(!search_hits(&result).is_empty(), "expected hits");

        client.cancel().await.expect("cancel client");
        let (stderr, status) = collect_exit(child).await;
        assert!(status.success(), "server must exit cleanly: {stderr}");
    })
    .await
    .expect("env-index server answers within timeout");
}
