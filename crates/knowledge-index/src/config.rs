//! Shared, facet-derived configuration for the `rust-knowledge` frontends.
//!
//! Both binaries define their user-facing surface through the types in this
//! module, parsed by [figue](https://facet.rs/figue/), so CLI arguments,
//! environment variables and defaults come from one source of truth. The
//! generated HTML reference page (`rust-knowledge config-docs`) walks these
//! same shapes.
//!
//! Layer precedence is CLI arguments > environment variables > code defaults,
//! applied by figue's driver. Only fields of a `#[facet(args::config)]` root
//! are env-addressable; `GlobalConfig` is used as a *flattened* config root so
//! its fields stay ordinary top-level flags (`--manifest-path`) while the
//! `RUST_KNOWLEDGE_INDEX_DIR` alias still layers in when the flag is absent.

use std::path::PathBuf;

use facet::Facet;
use figue::{self as args, Driver, DriverError, DriverOutcome, MockEnv};

/// Knobs shared by both binaries.
#[derive(Facet, Debug)]
pub struct GlobalConfig {
    /// Path to a Cargo.toml manifest. Defaults to the current directory.
    pub manifest_path: Option<PathBuf>,

    /// Directory for the knowledge index. Defaults to
    /// <workspace>/.rust-knowledge or $RUST_KNOWLEDGE_INDEX_DIR.
    #[facet(args::env_alias = "RUST_KNOWLEDGE_INDEX_DIR")]
    pub index_dir: Option<PathBuf>,
}

/// Standard `--help` / `--version` flags, frozen to the historical clap
/// surface.
///
/// Deliberately not figue's `FigueBuiltins`: that type would add
/// `--html-help`, `--completions` and `--export-jsonschemas` flags these
/// binaries never exposed.
#[derive(Facet, Debug)]
pub struct Builtins {
    /// Show help message and exit.
    #[facet(args::named, args::short = 'h', args::help, default)]
    pub help: bool,

    /// Show version and exit.
    #[facet(args::named, args::short = 'V', args::version, default)]
    pub version: bool,
}

/// Full argument surface of the `knowledge-mcp` binary.
#[derive(Facet, Debug)]
pub struct McpArgs {
    /// Shared knobs: `--manifest-path` and `--index-dir` (plus the
    /// RUST_KNOWLEDGE_INDEX_DIR env fallback).
    #[facet(args::config)]
    #[facet(flatten)]
    pub global: GlobalConfig,

    /// Standard help and version flags.
    #[facet(flatten)]
    pub builtins: Builtins,
}

/// Resolve the tracing env-filter.
///
/// Precedence: `--verbose` (forces debug) > `RUST_KNOWLEDGE_LOG` >
/// `RUST_LOG` > the default `info`. The log filter cannot be a CLI flag
/// itself because logging must be initialized before parsing (the MCP binary
/// keeps stdout protocol-clean), so the hierarchy lives in this tested
/// helper instead of the facet shapes.
pub fn resolve_log_filter(verbose: bool) -> String {
    resolve_log_filter_with(verbose, |name| std::env::var(name))
}

fn resolve_log_filter_with(
    verbose: bool,
    lookup: impl Fn(&str) -> Result<String, std::env::VarError>,
) -> String {
    if verbose {
        return "debug".to_string();
    }
    lookup("RUST_KNOWLEDGE_LOG")
        .or_else(|_| lookup("RUST_LOG"))
        .unwrap_or_else(|_| "info".to_string())
}

/// Parse the real process argv and environment into `T`.
///
/// `program_name`, `version` and `description` drive `--help` / `--version`
/// output, mirroring what clap's `#[command(...)]` attributes used to render.
pub fn parse_from_env<T: Facet<'static>>(
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_common(
        std::env::args().skip(1),
        None,
        program_name,
        version,
        description,
    )
}

/// Parse a fixed argv slice against a mock environment.
///
/// Test entry point: same layering as [parse_from_env], but with an
/// injectable environment so cases never depend on (or mutate) the process
/// environment.
pub fn parse_with_mock_env<T: Facet<'static>>(
    argv: &[&str],
    env: MockEnv,
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_common(
        argv.iter().map(|s| (*s).to_string()),
        Some(env),
        program_name,
        version,
        description,
    )
}

fn parse_common<T: Facet<'static>>(
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

    use super::*;

    const PROGRAM: &str = "knowledge-mcp";
    const VERSION: &str = "0.1.0";
    const DESCRIPTION: &str = "MCP server exposing the rust-knowledge retrieval engine";

    fn parse_mcp(argv: &[&str], env: MockEnv) -> DriverOutcome<McpArgs> {
        parse_with_mock_env(argv, env, PROGRAM, VERSION, DESCRIPTION)
    }

    #[test]
    fn resolve_log_filter_verbose_beats_everything() {
        let filter = resolve_log_filter_with(true, |name| {
            panic!("env must not be consulted when --verbose is set, got {name}");
        });
        assert_eq!(filter, "debug");
    }

    #[test]
    fn resolve_log_filter_precedence() {
        let none = |_: &str| Err(std::env::VarError::NotPresent);
        assert_eq!(resolve_log_filter_with(false, none), "info");

        let only_rust_log = |name: &str| match name {
            "RUST_LOG" => Ok("warn".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(resolve_log_filter_with(false, only_rust_log), "warn");

        let both = |name: &str| match name {
            "RUST_LOG" => Ok("warn".to_string()),
            "RUST_KNOWLEDGE_LOG" => Ok("trace".to_string()),
            _ => Err(std::env::VarError::NotPresent),
        };
        assert_eq!(resolve_log_filter_with(false, both), "trace");
    }

    #[test]
    fn mcp_both_flags_parse() {
        let outcome = parse_mcp(
            &[
                "--manifest-path",
                "/ws/Cargo.toml",
                "--index-dir",
                "/idx",
            ],
            MockEnv::new(),
        );
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(
            args.global.manifest_path.as_deref(),
            Some(Path::new("/ws/Cargo.toml"))
        );
        assert_eq!(args.global.index_dir.as_deref(), Some(Path::new("/idx")));
    }

    #[test]
    fn mcp_flag_equals_form_parses() {
        let outcome = parse_mcp(&["--index-dir=/idx"], MockEnv::new());
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(args.global.index_dir.as_deref(), Some(Path::new("/idx")));
        assert_eq!(args.global.manifest_path, None);
    }

    #[test]
    fn mcp_env_alias_sets_index_dir() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/from-env")]);
        let outcome = parse_mcp(&[], env);
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(
            args.global.index_dir.as_deref(),
            Some(Path::new("/from-env"))
        );
        assert_eq!(args.global.manifest_path, None);
    }

    #[test]
    fn mcp_args_beat_env_alias() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/from-env")]);
        let outcome = parse_mcp(&["--index-dir", "/from-flag"], env);
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(
            args.global.index_dir.as_deref(),
            Some(Path::new("/from-flag")),
            "CLI must win over the environment layer"
        );
    }

    #[test]
    fn mcp_unknown_flag_is_an_error() {
        let outcome = parse_mcp(&["--bogus"], MockEnv::new());
        match outcome.into_result() {
            Err(DriverError::Failed { .. }) => {}
            Err(other) => panic!("expected Failed, got {other:?}"),
            Ok(_) => panic!("unknown flag must not parse"),
        }
    }

    #[test]
    fn mcp_help_flag_short_circuits() {
        for argv in [&["--help"][..], &["-h"][..]] {
            match parse_mcp(argv, MockEnv::new()).into_result() {
                Err(DriverError::Help { text, .. }) => {
                    assert!(text.contains("knowledge-mcp"), "help text: {text}");
                    assert!(text.contains("--manifest-path"), "help text: {text}");
                }
                Err(other) => panic!("expected Help, got {other:?}"),
                Ok(_) => panic!("--help must not parse to a value"),
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
                Err(other) => panic!("expected Version, got {other:?}"),
                Ok(_) => panic!("--version must not parse to a value"),
            }
        }
    }
}
