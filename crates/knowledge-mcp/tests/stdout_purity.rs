//! The MCP binary's stdout is the JSON-RPC protocol channel: a parse
//! failure must never leak diagnostics into it. This pins that contract
//! end to end by spawning the built binary with an unknown flag: figue
//! prints its diagnostic to stderr and exits 1, leaving stdout empty
//! (before this test the guarantee rested only on the
//! parse-before-logging order in `main` and figue's own contract).

use std::process::{Command, Stdio};

#[test]
fn parse_failure_keeps_stdout_protocol_clean() {
    let output = Command::new(env!("CARGO_BIN_EXE_knowledge-mcp"))
        .arg("--bogus-flag")
        .stdin(Stdio::null())
        .output()
        .expect("spawning knowledge-mcp should work");
    assert!(
        output.stdout.is_empty(),
        "stdout is the MCP protocol channel; parse diagnostics leaked: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !output.stderr.is_empty(),
        "the parse diagnostic belongs on stderr"
    );
    let code = output
        .status
        .code()
        .expect("the binary should exit normally");
    assert_eq!(code, 1, "figue-native usage errors exit 1");
}
