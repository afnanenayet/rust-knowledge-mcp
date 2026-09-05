//! Shared, facet-derived configuration for the `rust-knowledge` frontends.
//!
//! Both binaries declare a *flattened figue config root* over
//! [WorkspaceConfig] (`#[facet(args::config, args::env_prefix = "RUST_KNOWLEDGE", flatten)]`),
//! following figue's layered-configuration recipe: the root's fields stay
//! ordinary top-level flags (`--manifest-path`, `--index-dir`, ...) while
//! figue's env layer addresses them through the exact-name aliases declared
//! below (or the prefixed `RUST_KNOWLEDGE__<FIELD>` forms). figue merges
//! the layers with its own precedence — CLI arguments over environment
//! variables over defaults — so a flag overriding an env var IS figue
//! behavior, not an emulation.
//!
//! figue 4.0.5 constraint (probed, see STATUS.md): a config root must be a
//! single level of leaf fields. A root whose fields are (or contain) plain
//! structs — e.g. flattening a nested struct into the root — never
//! materializes on empty argv ("missing field" errors). The defaulted
//! `log` field below keeps the root present for every invocation.

use std::path::PathBuf;

use facet::Facet;
use figue::{self as args, Driver, DriverError, DriverOutcome, FigueBuiltins, MockEnv};

/// Workspace knobs shared by `rust-knowledge` and `knowledge-mcp`.
///
/// Declared as a flattened figue config root in each binary's argv shape,
/// so every field is simultaneously a top-level CLI flag, an
/// environment-addressable value (via its alias, or
/// `RUST_KNOWLEDGE__<FIELD>`), and file-addressable (`--config <FILE>`,
/// `--config.<field>`) — all resolved by figue with CLI > env > file >
/// defaults.
#[derive(Facet, Debug)]
pub struct WorkspaceConfig {
    /// Path to a Cargo.toml manifest. Defaults to the current directory.
    #[facet(args::named, args::env_alias = "RUST_KNOWLEDGE_MANIFEST_PATH")]
    pub manifest_path: Option<PathBuf>,

    /// Directory for the knowledge index. Defaults to
    /// <workspace>/.rust-knowledge or $RUST_KNOWLEDGE_INDEX_DIR.
    #[facet(args::named, args::env_alias = "RUST_KNOWLEDGE_INDEX_DIR")]
    pub index_dir: Option<PathBuf>,

    /// Explicit cargo binary for metadata/rustdoc invocations. Defaults to
    /// cargo on $PATH, or $RUST_KNOWLEDGE_CARGO.
    #[facet(args::named, args::env_alias = "RUST_KNOWLEDGE_CARGO")]
    pub cargo: Option<PathBuf>,

    /// Tracing env-filter (e.g. "info", "demo_core=debug"). -v overrides
    /// it with "debug".
    #[facet(
        args::named,
        // figue uses the FIRST matching alias, so RUST_KNOWLEDGE_LOG wins
        // over RUST_LOG when both are set.
        args::env_alias = "RUST_KNOWLEDGE_LOG",
        args::env_alias = "RUST_LOG",
        default = "info"
    )]
    pub log: String,
}

/// User-facing description of the `knowledge-mcp` binary.
///
/// Single source for every surface that describes the binary: the figue
/// help configuration passed by its `main` and any generated reference.
pub const MCP_DESCRIPTION: &str = "MCP server exposing the rust-knowledge retrieval engine";

/// Full argument surface of the `knowledge-mcp` binary.
#[derive(Facet, Debug)]
pub struct McpArgs {
    /// Workspace knobs, layered by figue (CLI > env > defaults).
    #[facet(args::config, args::env_prefix = "RUST_KNOWLEDGE", flatten)]
    pub config: WorkspaceConfig,

    /// Standard figue builtins: --help, --html-help, --version,
    /// --completions, --export-jsonschemas.
    #[facet(flatten)]
    pub builtins: FigueBuiltins,
}

/// The effective tracing filter: `--verbose` forces "debug" (args beat the
/// layered log filter), otherwise the figue-resolved `log` value applies.
pub fn effective_log_filter(verbose: bool, log: &str) -> String {
    if verbose {
        "debug".to_string()
    } else {
        log.to_string()
    }
}

/// Parse the real process argv into `T` through figue's layered driver:
/// CLI arguments over environment variables over defaults.
///
/// `program_name`, `version` and `description` drive figue's --help /
/// --version output. The returned [DriverOutcome] carries figue's own
/// help/version/diagnostics handling (see the figue recipes: match
/// [DriverError] variants, or call `unwrap()` for figue's native
/// print-and-exit behavior).
pub fn parse_std_args<T: Facet<'static>>(
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_layered(
        std::env::args().skip(1),
        None,
        program_name,
        version,
        description,
    )
}

/// [parse_std_args] over an explicit argv and environment, for tests.
pub fn parse_args_with<T: Facet<'static>>(
    argv: &[&str],
    env: MockEnv,
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_layered(
        argv.iter().map(|s| (*s).to_string()),
        Some(env),
        program_name,
        version,
        description,
    )
}

fn parse_layered<T: Facet<'static>>(
    argv: impl Iterator<Item = String>,
    env: Option<MockEnv>,
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    let builder = match figue::builder::<T>() {
        Ok(builder) => builder,
        Err(error) => return DriverOutcome::err(DriverError::Builder { error }),
    };
    let config = builder
        .cli(|cli| cli.args(argv))
        .env(|layer| match env {
            Some(mock) => layer.source(mock),
            None => layer,
        })
        .help(|help| {
            help.program_name(program_name)
                .version(version)
                .description(description)
        })
        .build();
    Driver::new(config).run()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use figue::{DriverError, MockEnv};

    use super::{MCP_DESCRIPTION, McpArgs, effective_log_filter, parse_args_with};

    const PROGRAM: &str = "knowledge-mcp";
    const VERSION: &str = "0.1.0";

    fn parse_mcp(argv: &[&str], env: MockEnv) -> figue::DriverOutcome<McpArgs> {
        parse_args_with(argv, env, PROGRAM, VERSION, MCP_DESCRIPTION)
    }

    fn parse_mcp_ok(argv: &[&str], env: MockEnv) -> McpArgs {
        parse_mcp(argv, env)
            .into_result()
            .expect("argv should parse")
            .get()
    }

    #[test]
    fn mcp_empty_argv_parses_to_defaults() {
        let args = parse_mcp_ok(&[], MockEnv::new());
        assert_eq!(args.config.manifest_path, None);
        assert_eq!(args.config.index_dir, None);
        assert_eq!(args.config.cargo, None);
        assert_eq!(args.config.log, "info");
        assert!(!args.builtins.help);
    }

    #[test]
    fn mcp_flags_parse_both_value_forms() {
        for argv in [
            &["--manifest-path", "/ws/Cargo.toml", "--index-dir", "/idx"][..],
            &["--manifest-path=/ws/Cargo.toml", "--index-dir=/idx"][..],
        ] {
            let args = parse_mcp_ok(argv, MockEnv::new());
            assert_eq!(
                args.config.manifest_path.as_deref(),
                Some(Path::new("/ws/Cargo.toml"))
            );
            assert_eq!(args.config.index_dir.as_deref(), Some(Path::new("/idx")));
        }
    }

    #[test]
    fn env_layer_fills_the_gap_over_defaults() {
        let env = MockEnv::from_pairs([
            ("RUST_KNOWLEDGE_INDEX_DIR", "/idx-from-env"),
            ("RUST_KNOWLEDGE_CARGO", "/cargo-from-env"),
        ]);
        let args = parse_mcp_ok(&[], env);
        assert_eq!(
            args.config.index_dir.as_deref(),
            Some(Path::new("/idx-from-env")),
            "env vars must fill fields no flag set"
        );
        assert_eq!(
            args.config.cargo.as_deref(),
            Some(Path::new("/cargo-from-env"))
        );
    }

    #[test]
    fn cli_beats_env_end_to_end() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/idx-from-env")]);
        let args = parse_mcp_ok(&["--index-dir", "/idx-from-flag"], env);
        assert_eq!(
            args.config.index_dir.as_deref(),
            Some(Path::new("/idx-from-flag")),
            "CLI args must beat env vars (figue layer precedence)"
        );
    }

    #[test]
    fn prefixed_env_var_form_is_honored() {
        // Flattened config roots address fields as PREFIX__FIELD (the root
        // field name is not part of the env var name).
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE__INDEX_DIR", "/idx-prefixed")]);
        let args = parse_mcp_ok(&[], env);
        assert_eq!(
            args.config.index_dir.as_deref(),
            Some(Path::new("/idx-prefixed"))
        );
    }

    #[test]
    fn log_env_layering() {
        // No env, no flag: the declared default.
        let args = parse_mcp_ok(&[], MockEnv::new());
        assert_eq!(args.config.log, "info");

        // RUST_LOG fills the gap.
        let env = MockEnv::from_pairs([("RUST_LOG", "warn")]);
        let args = parse_mcp_ok(&[], env);
        assert_eq!(args.config.log, "warn");

        // RUST_KNOWLEDGE_LOG is the first alias, so it wins over RUST_LOG.
        let env = MockEnv::from_pairs([("RUST_LOG", "warn"), ("RUST_KNOWLEDGE_LOG", "trace")]);
        let args = parse_mcp_ok(&[], env);
        assert_eq!(args.config.log, "trace");

        // A --log flag beats every env var.
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_LOG", "trace")]);
        let args = parse_mcp_ok(&["--log", "demo_core=debug"], env);
        assert_eq!(args.config.log, "demo_core=debug");
    }

    #[test]
    fn mcp_unknown_flag_is_an_error() {
        match parse_mcp(&["--bogus"], MockEnv::new()).into_result() {
            Err(e) => {
                assert_eq!(e.exit_code(), 1, "figue-native error exit code");
                assert!(!e.is_success());
            }
            Ok(_) => panic!("unknown flag must not parse"),
        }
    }

    #[test]
    fn mcp_help_flag_short_circuits() {
        for argv in [&["--help"][..], &["-h"][..]] {
            match parse_mcp(argv, MockEnv::new()).into_result() {
                Err(e @ DriverError::Help { .. }) => {
                    assert!(e.is_success());
                    let text = format!("{e}");
                    assert!(text.contains("knowledge-mcp"), "help text: {text}");
                    assert!(text.contains("--manifest-path"), "help text: {text}");
                    assert!(text.contains("--index-dir"), "help text: {text}");
                }
                Err(other) => panic!("expected Help for {argv:?}, got {other:?}"),
                Ok(_) => panic!("{argv:?} must not parse to a value"),
            }
        }
    }

    #[test]
    fn mcp_version_flag_short_circuits() {
        for argv in [&["--version"][..], &["-V"][..]] {
            match parse_mcp(argv, MockEnv::new()).into_result() {
                Err(DriverError::Version { text }) => {
                    assert_eq!(text.trim_end(), "knowledge-mcp 0.1.0");
                }
                Err(other) => panic!("expected Version for {argv:?}, got {other:?}"),
                Ok(_) => panic!("{argv:?} must not parse to a value"),
            }
        }
    }

    #[test]
    fn log_filter_resolution() {
        assert_eq!(effective_log_filter(true, "info"), "debug");
        assert_eq!(effective_log_filter(false, "warn"), "warn");
        assert_eq!(effective_log_filter(false, "info"), "info");
    }
}
