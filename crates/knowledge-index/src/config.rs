//! Shared, facet-derived configuration for the `rust-knowledge` frontends.
//!
//! Both binaries define their user-facing surface through the types in this
//! module, parsed by [figue](https://facet.rs/figue/), so CLI arguments,
//! environment variables and defaults come from one source of truth. The
//! generated HTML reference page (`rust-knowledge config-docs`) walks these
//! same shapes.
//!
//! Layer precedence is CLI arguments > environment variables > defaults.
//! Two constraints shaped the design:
//!
//! * figue only env-addresses fields of a `#[facet(args::config)]` root, but
//!   every config root also exposes a config-file flag (`--root <FILE>`,
//!   which the driver then loads) and dotted override flags
//!   (`--root.field <VALUE>`) — new flag surface these binaries never had,
//!   and working file-based configuration, which issue #2 explicitly
//!   defers. The argv shapes therefore keep config roots out entirely; the
//!   `RUST_KNOWLEDGE_INDEX_DIR` override is modeled as its own env-only
//!   facet shape ([IndexDirEnv]), layered by figue's env layer without any
//!   CLI/file surface, and applied by the typed [resolve_index_dir] helper.
//! * figue leaves `Option` fields absent from the merged value (facet
//!   defaults them during deserialization), so a *flattened* struct whose
//!   fields are all `Option` never materializes when no flag is given
//!   ("missing field" errors). The two shared flags are therefore declared
//!   directly on each binary's shape instead of being flattened in from a
//!   shared struct; [Builtins] (bools with explicit defaults) and
//!   [IndexDirEnv] remain the genuinely shared shapes.

use std::path::PathBuf;

use facet::Facet;
use figue::{self as args, Driver, DriverError, DriverOutcome, MockEnv};

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

/// User-facing description of the `knowledge-mcp` binary.
///
/// Single source for every surface that describes the binary: the
/// additional `--help` description (passed to [parse_std_args] by its
/// `main`) and the generated HTML reference page. The struct-level doc
/// comment below is the API-documentation summary figue also prints as
/// the first help line; this const is the description proper.
pub const MCP_DESCRIPTION: &str = "MCP server exposing the rust-knowledge retrieval engine";

/// Full argument surface of the `knowledge-mcp` binary.
#[derive(Facet, Debug)]
pub struct McpArgs {
    /// Path to the workspace Cargo.toml the index was built for.
    #[facet(args::named)]
    pub manifest_path: Option<PathBuf>,

    /// Knowledge index directory. Defaults to <workspace>/.rust-knowledge
    /// or $RUST_KNOWLEDGE_INDEX_DIR.
    #[facet(args::named)]
    pub index_dir: Option<PathBuf>,

    /// Standard help and version flags.
    #[facet(flatten)]
    pub builtins: Builtins,
}

/// Environment-only shape for the index directory override.
///
/// Never part of a binary's argv surface; it exists so figue's env layer
/// (and the generated HTML reference) model `RUST_KNOWLEDGE_INDEX_DIR`
/// declaratively. See [resolve_index_dir] for how the value is applied.
#[derive(Facet, Debug)]
pub struct IndexDirEnv {
    /// Directory for the knowledge index. Defaults to
    /// <workspace>/.rust-knowledge or $RUST_KNOWLEDGE_INDEX_DIR.
    #[facet(args::env_alias = "RUST_KNOWLEDGE_INDEX_DIR")]
    pub index_dir: Option<PathBuf>,
}

/// Config-root wrapper required for figue to env-address [IndexDirEnv].
#[derive(Facet, Debug)]
struct IndexDirEnvRoot {
    #[facet(args::config)]
    global: IndexDirEnv,
}

/// Resolve the effective index directory.
///
/// Precedence: `--index-dir` > `$RUST_KNOWLEDGE_INDEX_DIR` (read through
/// figue's env layer) > absent (the caller falls back to
/// `<workspace>/.rust-knowledge`).
pub fn resolve_index_dir(flag: Option<PathBuf>) -> Option<PathBuf> {
    flag.or_else(|| env_index_dir(None))
}

/// [resolve_index_dir] against an explicit environment (for tests).
pub fn resolve_index_dir_with(flag: Option<PathBuf>, env: MockEnv) -> Option<PathBuf> {
    flag.or_else(|| env_index_dir(Some(env)))
}

fn env_index_dir(env: Option<MockEnv>) -> Option<PathBuf> {
    let Ok(builder) = figue::builder::<IndexDirEnvRoot>() else {
        return None;
    };
    let config = builder
        .env(|layer| match env {
            Some(mock) => layer.source(mock),
            None => layer,
        })
        .build();
    match Driver::new(config).run().into_result() {
        Ok(output) => output.value.global.index_dir,
        Err(_) => None,
    }
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

/// Whether the real process argv explicitly requests help or version.
///
/// figue reports missing required fields as `DriverError::Help`; this
/// distinguishes a genuine `--help` (exit 0 on stdout, like clap) from that
/// diagnostic path (exit 2 on stderr, like clap).
pub fn std_argv_requests_help() -> bool {
    std::env::args()
        .skip(1)
        .any(|a| matches!(a.as_str(), "--help" | "-h" | "--version" | "-V"))
}

/// [std_argv_requests_help] over an explicit argv slice (for tests).
pub fn argv_requests_help(argv: &[&str]) -> bool {
    argv
        .iter()
        .any(|a| matches!(*a, "--help" | "-h" | "--version" | "-V"))
}

/// Parse the real process argv into `T`.
///
/// `program_name`, `version` and `description` drive `--help` / `--version`
/// output, mirroring what clap's `#[command(...)]` attributes used to render.
pub fn parse_std_args<T: Facet<'static>>(
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_common(std::env::args().skip(1), program_name, version, description)
}

/// Parse a fixed argv slice into `T` (for tests).
pub fn parse_args<T: Facet<'static>>(
    argv: &[&str],
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    parse_common(
        argv.iter().map(|s| (*s).to_string()),
        program_name,
        version,
        description,
    )
}

fn parse_common<T: Facet<'static>>(
    argv: impl Iterator<Item = String>,
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
    const DESCRIPTION: &str = MCP_DESCRIPTION;

    fn parse_mcp(argv: &[&str]) -> DriverOutcome<McpArgs> {
        parse_args(argv, PROGRAM, VERSION, DESCRIPTION)
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
    fn resolve_index_dir_env_alias_honored() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/from-env")]);
        let resolved = resolve_index_dir_with(None, env);
        assert_eq!(resolved.as_deref(), Some(Path::new("/from-env")));
    }

    #[test]
    fn resolve_index_dir_flag_beats_env() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/from-env")]);
        let resolved = resolve_index_dir_with(Some(PathBuf::from("/from-flag")), env);
        assert_eq!(
            resolved.as_deref(),
            Some(Path::new("/from-flag")),
            "--index-dir must win over the environment layer"
        );
    }

    #[test]
    fn resolve_index_dir_absent_when_unset() {
        let resolved = resolve_index_dir_with(None, MockEnv::new());
        assert_eq!(resolved, None);
    }

    #[test]
    fn argv_help_detection() {
        assert!(argv_requests_help(&["--help"]));
        assert!(argv_requests_help(&["-h"]));
        assert!(argv_requests_help(&["search", "--version"]));
        assert!(argv_requests_help(&["-V"]));
        assert!(!argv_requests_help(&["search", "foo"]));
        assert!(!argv_requests_help(&[]));
        assert!(!argv_requests_help(&["--manifest-path", "-h/x"]));
    }

    #[test]
    fn mcp_empty_argv_parses_to_none() {
        let args = parse_mcp(&[]).into_result().expect("parse should succeed").value;
        assert_eq!(args.manifest_path, None);
        assert_eq!(args.index_dir, None);
        assert!(!args.builtins.help);
    }

    #[test]
    fn mcp_both_flags_parse() {
        let outcome = parse_mcp(&[
            "--manifest-path",
            "/ws/Cargo.toml",
            "--index-dir",
            "/idx",
        ]);
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(
            args.manifest_path.as_deref(),
            Some(Path::new("/ws/Cargo.toml"))
        );
        assert_eq!(args.index_dir.as_deref(), Some(Path::new("/idx")));
    }

    #[test]
    fn mcp_flag_equals_form_parses() {
        let outcome = parse_mcp(&["--index-dir=/idx"]);
        let args = outcome.into_result().expect("parse should succeed").value;
        assert_eq!(args.index_dir.as_deref(), Some(Path::new("/idx")));
        assert_eq!(args.manifest_path, None);
    }

    #[test]
    fn mcp_env_alias_composes_with_flags() {
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_INDEX_DIR", "/from-env")]);

        // No --index-dir flag: the env var fills the gap end to end.
        let args = parse_mcp(&[]).into_result().expect("parse should succeed").value;
        let resolved = resolve_index_dir_with(args.index_dir, env.clone());
        assert_eq!(resolved.as_deref(), Some(Path::new("/from-env")));

        // Flag present: it wins over the same env var.
        let args = parse_mcp(&["--index-dir", "/from-flag"])
            .into_result()
            .expect("parse should succeed")
            .value;
        let resolved = resolve_index_dir_with(args.index_dir, env);
        assert_eq!(resolved.as_deref(), Some(Path::new("/from-flag")));
    }

    #[test]
    fn mcp_unknown_flag_is_an_error() {
        let outcome = parse_mcp(&["--bogus"]);
        match outcome.into_result() {
            Err(DriverError::Failed { .. }) => {}
            Err(other) => panic!("expected Failed, got {other:?}"),
            Ok(_) => panic!("unknown flag must not parse"),
        }
    }

    #[test]
    fn mcp_help_flag_short_circuits() {
        for argv in [&["--help"][..], &["-h"][..]] {
            match parse_mcp(argv).into_result() {
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
            match parse_mcp(argv).into_result() {
                Err(DriverError::Version { text }) => {
                    assert_eq!(text.trim_end(), "knowledge-mcp 0.1.0");
                }
                Err(other) => panic!("expected Version, got {other:?}"),
                Ok(_) => panic!("--version must not parse to a value"),
            }
        }
    }
}
