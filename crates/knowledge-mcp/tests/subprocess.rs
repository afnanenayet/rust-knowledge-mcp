//! Subprocess MCP integration tests: launch the real `knowledge-mcp` binary
//! the way a globally registered client does — zero arguments, working
//! directory inside the workspace — and talk MCP over its stdio with the
//! rmcp client. Also pins the two fail-fast error contracts (no workspace
//! inferable; workspace without an index) and the explicit `--manifest-path`
//! / `--index-dir` / `RUST_KNOWLEDGE_INDEX_DIR` paths, of which the
//! index-dir forms must stay cargo-free.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use knowledge_index::{IndexOptions, IndexOutcome, RustdocScope, index_workspace};
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

/// Builds the fixture index through the engine's own end-to-end entry
/// point (`index_workspace`; committed prebuilt rustdoc, so no nightly) so
/// tests serve exactly what `rust-knowledge index` produces. `None` lands
/// at the default `<workspace>/.rust-knowledge` output location.
fn build_fixture_index(index_dir: Option<&Path>) -> IndexOutcome {
    index_workspace(
        Some(&fixture_dir().join("Cargo.toml")),
        index_dir,
        &IndexOptions {
            rustdoc_scope: RustdocScope::All,
            toolchain: None,
            prebuilt_rustdoc: Some(fixture_dir().join("prebuilt-rustdoc")),
            ..IndexOptions::default()
        },
    )
    .expect("index_workspace")
}

/// Removes the fixture's default index directory on drop, so even a
/// failing test cannot leave a stale index behind for the next run.
struct FixtureIndexCleanup(PathBuf);

impl Drop for FixtureIndexCleanup {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(&self.0));
    }
}

/// Serializes the tests that build and clean the fixture's default index
/// directory (`<fixture>/.rust-knowledge`): the zero-arg inference test
/// and the explicit-manifest test both use it, and this binary's tests run
/// in parallel threads. A `tokio::sync::Mutex` because the guard is held
/// across the test's await points.
static FIXTURE_INDEX_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn fixture_index_lock() -> tokio::sync::MutexGuard<'static, ()> {
    FIXTURE_INDEX_LOCK.lock().await
}

/// Spawns the built binary with a scrubbed environment: host
/// `RUST_KNOWLEDGE_*` settings must not skew the child's resolution or its
/// log output — the child reads `RUST_KNOWLEDGE_LOG` for its tracing level,
/// so a host value like `error` or `off` would silence the startup lines
/// these tests pin (and make log-absence assertions pass vacuously).
fn spawn_server(args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_knowledge-mcp"));
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("RUST_KNOWLEDGE_INDEX_DIR")
        .env_remove("RUST_KNOWLEDGE_CARGO")
        .env_remove("RUST_KNOWLEDGE_LOG");
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
    let payload: Value = serde_json::from_str(text).expect("json payload");
    payload
        .get("results")
        .and_then(Value::as_array)
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
    // it fresh and remove it on unwind. The explicit-manifest test shares
    // it; the lock serializes the two (this binary's tests run in parallel).
    let _guard = fixture_index_lock().await;
    let index_dir = fixture_dir().join(".rust-knowledge");
    drop(std::fs::remove_dir_all(&index_dir));
    let _cleanup = FixtureIndexCleanup(index_dir.clone());
    // `None` = the engine's own default output location, so serving it also
    // pins that what `rust-knowledge index` writes by default is exactly
    // what a zero-arg server finds.
    let outcome = build_fixture_index(None);
    assert!(
        outcome.index_dir.ends_with("fixtures/demo-workspace/.rust-knowledge"),
        "index_workspace must default to <workspace>/.rust-knowledge, got {}",
        outcome.index_dir.display()
    );

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
        assert_eq!(
            top.get("package"),
            Some(&json!("demo-core")),
            "hits: {hits:?}"
        );

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
///
/// Assumes the OS temp tree is not inside a cargo workspace (true for
/// standard TMPDIR setups); if it ever were, this test would fail
/// spuriously rather than silently.
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
    build_fixture_index(Some(index.path()));
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
    build_fixture_index(Some(index.path()));
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

/// Explicit `--manifest-path` from a cwd with no workspace around it: the
/// flag must resolve the workspace (and its default index dir) without cwd
/// inference. Pins `--manifest-path` > cwd inference — reordering the
/// resolution arms so an explicit manifest still triggered the cwd walk
/// (which cannot succeed from this cwd) would break this test.
#[tokio::test]
async fn explicit_manifest_path_serves_from_a_foreign_cwd() {
    // Shares the fixture's default index dir with the zero-arg test; the
    // lock serializes the two.
    let _guard = fixture_index_lock().await;
    let index_dir = fixture_dir().join(".rust-knowledge");
    drop(std::fs::remove_dir_all(&index_dir));
    let _cleanup = FixtureIndexCleanup(index_dir.clone());
    build_fixture_index(None);

    let manifest_arg = fixture_dir()
        .join("Cargo.toml")
        .to_str()
        .expect("manifest path is utf-8")
        .to_string();
    let cwd = tempfile::tempdir().expect("foreign cwd");

    let child = spawn_server(&["--manifest-path", manifest_arg.as_str()], cwd.path(), &[]);
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
    .expect("explicit-manifest server answers within timeout");

    assert!(
        stderr.contains("explicit --manifest-path"),
        "startup log must mark the manifest as explicit: {stderr}"
    );
    assert!(
        !stderr.contains("inferred from cwd"),
        "an explicit manifest must skip cwd inference: {stderr}"
    );
}
