//! The CLI's stdout carries the commands' own output — for `--json`
//! subcommands, one JSON object per line — so log lines from the tracing
//! subscriber must never mix into it. This pins that contract end to end
//! by spawning the built binary: `packages --json` runs cargo metadata,
//! which logs at INFO under the default filter, so a misrouted writer
//! surfaces as an unparseable stdout line (before this test the
//! guarantee rested only on the `with_writer(stderr)` call in `main`).

use std::path::Path;
use std::process::Command;

#[test]
fn json_stdout_stays_free_of_log_lines() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-workspace");
    let output = Command::new(env!("CARGO_BIN_EXE_rust-knowledge"))
        .arg("packages")
        .arg("--json")
        .arg("--manifest-path")
        .arg(fixture.join("Cargo.toml"))
        .output()
        .expect("spawning rust-knowledge should work");
    assert!(
        output.status.success(),
        "`packages --json` should succeed on the fixture workspace; stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "the fixture workspace should have packages to list"
    );
    for line in lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "every stdout line of `packages --json` must be JSON; got {line:?} \
             (log lines belong on stderr)"
        );
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("resolved cargo universe"),
        "the INFO logs of the cargo metadata run belong on stderr, got: {stderr:?}"
    );
}
