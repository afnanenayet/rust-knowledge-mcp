//! Shared, facet-derived configuration for the `rust-knowledge` frontends.
//!
//! Both binaries declare a *flattened figue config root* over
//! [`WorkspaceConfig`] (`#[facet(args::config, args::env_prefix = "RUST_KNOWLEDGE", flatten)]`),
//! following figue's layered-configuration recipe: the root's fields stay
//! ordinary top-level flags (`--manifest-path`, `--index-dir`, ...) while
//! figue's env layer addresses them through the exact-name aliases declared
//! below (or the prefixed `RUST_KNOWLEDGE__<FIELD>` forms). figue merges
//! the layers with its own precedence — CLI arguments over environment
//! variables over defaults — so a flag overriding an env var IS figue
//! behavior, not an emulation.
//!
//! figue 4.0.5 constraint (empirically probed): a config root
//! materializes on empty argv only if it holds at least one defaulted
//! non-Option leaf — figue leaves Option fields absent from the merged
//! value, so an all-Option root fails with a "missing field" error (the
//! only failing shape: nested struct fields, `flatten`ed ones included,
//! materialize fine). The defaulted `log` field below satisfies it.
//! Keeping the root a single level of leaf fields follows figue's
//! deploy-cli recipe — a shape choice, not a figue requirement.

use std::path::{Path, PathBuf};

use facet::Facet;
use figue::{self as args, Driver, DriverError, DriverOutcome, DriverReport, MockEnv};

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
    /// <workspace>/.rust-knowledge or $`RUST_KNOWLEDGE_INDEX_DIR`.
    #[facet(args::named, args::env_alias = "RUST_KNOWLEDGE_INDEX_DIR")]
    pub index_dir: Option<PathBuf>,

    /// Explicit cargo binary for metadata/rustdoc invocations. Defaults to
    /// $`RUST_KNOWLEDGE_CARGO`, or cargo on $PATH.
    #[facet(args::named, args::env_alias = "RUST_KNOWLEDGE_CARGO")]
    pub cargo: Option<PathBuf>,

    /// Tracing env-filter (e.g. "info", "`demo_core=debug`"). -v overrides
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

impl WorkspaceConfig {
    /// The effective tracing filter for a run: `--verbose` forces "debug"
    /// (a flag beats the layered log value), otherwise the figue-resolved
    /// `log` value (default "info") applies. Validate the result with
    /// `EnvFilter::try_new` where the subscriber is initialized —
    /// `EnvFilter::new` silently ignores invalid directives, which would
    /// degrade logging to ERROR-only with no diagnostic.
    #[must_use]
    pub fn log_filter(&self, verbose: bool) -> String {
        if verbose {
            "debug".to_string()
        } else {
            self.log.clone()
        }
    }

    /// The index directory for a resolved workspace root: an explicit
    /// `--index-dir` wins; otherwise the workspace default,
    /// `<root>/.rust-knowledge` ([`crate::pipeline::default_index_dir`]).
    #[must_use]
    pub fn resolve_index_dir(&self, workspace_root: &Path) -> PathBuf {
        self.index_dir
            .clone()
            .unwrap_or_else(|| crate::pipeline::default_index_dir(workspace_root))
    }
}

/// Parse the real process argv into `T` through figue's layered driver:
/// CLI arguments over environment variables over defaults.
///
/// An argument that is not valid UTF-8 cannot be represented in figue's
/// CLI layer (String values), so the parse fails hard with a diagnostic
/// on stderr and a [`DriverError::Failed`] outcome (exit 1 via
/// `DriverOutcome::unwrap`) — it is neither skipped nor panicked on
/// (`std::env::args` would panic): skipping an entry mid-argv re-binds
/// the surrounding flags (the next token would become the previous
/// flag's value), silently misparsing the rest of argv.
///
/// `program_name`, `version` and `description` drive figue's --help /
/// --version output. The returned [`DriverOutcome`] carries figue's own
/// help/version/diagnostics handling (see the figue recipes: match
/// [`DriverError`] variants, or call `unwrap()` for figue's native
/// print-and-exit behavior).
pub fn parse_std_args<T: Facet<'static>>(
    program_name: &str,
    version: &str,
    description: &str,
) -> DriverOutcome<T> {
    let (argv, skipped) = utf8_argv(std::env::args_os().skip(1));
    if skipped > 0 {
        // figue's Diagnostic type is private, so a Failed report cannot
        // carry this message; it prints here and the default (empty)
        // report below supplies the non-zero exit. Stderr is figue's
        // diagnostic channel at this stage (the subscriber does not
        // exist yet), and stdout stays protocol-clean for the MCP binary.
        eprintln!(
            "error: {skipped} argument{} not valid UTF-8; refusing to guess \
             the rest of argv (all CLI values are UTF-8)",
            if skipped == 1 { " is" } else { "s are" }
        );
        return DriverOutcome::err(DriverError::Failed {
            report: Box::new(DriverReport::default()),
        });
    }
    parse_layered(argv.into_iter(), None, program_name, version, description)
}

/// Splits arguments into the UTF-8-parseable ones and a count of the
/// rest; a non-zero count is a hard parse failure in [`parse_std_args`]
/// (figue parses String CLI values only).
fn utf8_argv<I: Iterator<Item = std::ffi::OsString>>(args: I) -> (Vec<String>, usize) {
    let mut argv = Vec::new();
    let mut skipped = 0;
    for arg in args {
        match arg.to_str() {
            Some(text) => argv.push(text.to_owned()),
            None => skipped += 1,
        }
    }
    (argv, skipped)
}

/// [`parse_std_args`] over an explicit argv and environment, for tests.
///
/// `env` is figue's [`MockEnv`] (figue's public env-layer source type;
/// `std::env::var` cannot be swapped out in-process), so this is the
/// cross-crate test seam for the frontends' parse plumbing.
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
    use std::path::{Path, PathBuf};

    use facet::Facet;
    use figue::{self as args, DriverError, FigueBuiltins, MockEnv};

    use super::{WorkspaceConfig, parse_args_with, utf8_argv};

    const PROGRAM: &str = "test-binary";
    const VERSION: &str = "0.1.0";
    const DESCRIPTION: &str = "shape under test: the shared workspace config root";

    /// The binaries' flattened config-root shape (figue config root over
    /// [WorkspaceConfig] plus figue's builtins), declared here because
    /// each real argv shape belongs to its own binary.
    #[derive(Facet, Debug)]
    struct TestArgs {
        #[facet(args::config, args::env_prefix = "RUST_KNOWLEDGE", flatten)]
        config: WorkspaceConfig,

        #[facet(flatten)]
        builtins: FigueBuiltins,
    }

    fn parse(argv: &[&str], env: MockEnv) -> figue::DriverOutcome<TestArgs> {
        parse_args_with(argv, env, PROGRAM, VERSION, DESCRIPTION)
    }

    fn parse_ok(argv: &[&str], env: MockEnv) -> TestArgs {
        parse(argv, env)
            .into_result()
            .expect("argv should parse")
            .get()
    }

    fn default_config() -> WorkspaceConfig {
        WorkspaceConfig {
            manifest_path: None,
            index_dir: None,
            cargo: None,
            log: "info".to_string(),
        }
    }

    #[test]
    fn empty_argv_parses_to_defaults() {
        let args = parse_ok(&[], MockEnv::new());
        assert_eq!(args.config.manifest_path, None);
        assert_eq!(args.config.index_dir, None);
        assert_eq!(args.config.cargo, None);
        assert_eq!(args.config.log, "info");
        assert!(!args.builtins.help);
    }

    #[test]
    fn flags_parse_both_value_forms() {
        for argv in [
            &["--manifest-path", "/ws/Cargo.toml", "--index-dir", "/idx"][..],
            &["--manifest-path=/ws/Cargo.toml", "--index-dir=/idx"][..],
        ] {
            let args = parse_ok(argv, MockEnv::new());
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
        let args = parse_ok(&[], env);
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
        let args = parse_ok(&["--index-dir", "/idx-from-flag"], env);
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
        let args = parse_ok(&[], env);
        assert_eq!(
            args.config.index_dir.as_deref(),
            Some(Path::new("/idx-prefixed"))
        );
    }

    #[test]
    fn log_env_layering() {
        // No env, no flag: the declared default.
        let args = parse_ok(&[], MockEnv::new());
        assert_eq!(args.config.log, "info");

        // RUST_LOG fills the gap.
        let env = MockEnv::from_pairs([("RUST_LOG", "warn")]);
        let args = parse_ok(&[], env);
        assert_eq!(args.config.log, "warn");

        // RUST_KNOWLEDGE_LOG is the first alias, so it wins over RUST_LOG.
        let env = MockEnv::from_pairs([("RUST_LOG", "warn"), ("RUST_KNOWLEDGE_LOG", "trace")]);
        let args = parse_ok(&[], env);
        assert_eq!(args.config.log, "trace");

        // A --log flag beats every env var.
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_LOG", "trace")]);
        let args = parse_ok(&["--log", "demo_core=debug"], env);
        assert_eq!(args.config.log, "demo_core=debug");
    }

    /// The file layer documented on [WorkspaceConfig]: a `--config <FILE>`
    /// value sits below CLI flags and env vars, and above defaults
    /// (CLI > env > file > defaults).
    #[test]
    fn config_file_sits_between_env_and_cli() {
        let file = tempfile::NamedTempFile::with_suffix(".json").expect("temp config file");
        std::fs::write(
            &file,
            r#"{"cargo": "/cargo-from-file", "index_dir": "/idx-from-file", "log": "trace"}"#,
        )
        .expect("write config file");
        let path = file.path().to_str().expect("temp paths are UTF-8");

        // The file beats defaults: with no flag and no env var, its
        // values apply.
        let args = parse_ok(&["--config", path], MockEnv::new());
        assert_eq!(
            args.config.cargo.as_deref(),
            Some(Path::new("/cargo-from-file")),
            "the config file must beat defaults"
        );
        assert_eq!(
            args.config.index_dir.as_deref(),
            Some(Path::new("/idx-from-file"))
        );
        assert_eq!(args.config.log, "trace");

        // Env vars beat the file.
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_CARGO", "/cargo-from-env")]);
        let args = parse_ok(&["--config", path], env);
        assert_eq!(
            args.config.cargo.as_deref(),
            Some(Path::new("/cargo-from-env")),
            "env vars must beat the config file"
        );

        // CLI flags beat the file (and the env var).
        let env = MockEnv::from_pairs([("RUST_KNOWLEDGE_CARGO", "/cargo-from-env")]);
        let args = parse_ok(&["--config", path, "--cargo", "/cargo-from-flag"], env);
        assert_eq!(
            args.config.cargo.as_deref(),
            Some(Path::new("/cargo-from-flag")),
            "CLI flags must beat the config file and env vars"
        );
    }

    #[test]
    fn unknown_flag_is_an_error() {
        match parse(&["--bogus"], MockEnv::new()).into_result() {
            Err(e) => {
                assert_eq!(e.exit_code(), 1, "figue-native error exit code");
                assert!(!e.is_success());
            }
            Ok(_) => panic!("unknown flag must not parse"),
        }
    }

    #[test]
    fn help_flag_short_circuits() {
        for argv in [&["--help"][..], &["-h"][..]] {
            match parse(argv, MockEnv::new()).into_result() {
                Err(e @ DriverError::Help { .. }) => {
                    assert!(e.is_success());
                    let text = format!("{e}");
                    assert!(text.contains(PROGRAM), "help text: {text}");
                    assert!(text.contains("--manifest-path"), "help text: {text}");
                    assert!(text.contains("--index-dir"), "help text: {text}");
                }
                Err(other) => panic!("expected Help for {argv:?}, got {other:?}"),
                Ok(_) => panic!("{argv:?} must not parse to a value"),
            }
        }
    }

    #[test]
    fn version_flag_short_circuits() {
        for argv in [&["--version"][..], &["-V"][..]] {
            match parse(argv, MockEnv::new()).into_result() {
                Err(DriverError::Version { text }) => {
                    assert_eq!(text.trim_end(), "test-binary 0.1.0");
                }
                Err(other) => panic!("expected Version for {argv:?}, got {other:?}"),
                Ok(_) => panic!("{argv:?} must not parse to a value"),
            }
        }
    }

    #[test]
    fn log_filter_resolution() {
        assert_eq!(default_config().log_filter(true), "debug");
        assert_eq!(default_config().log_filter(false), "info");
        let quiet = WorkspaceConfig {
            log: "warn".to_string(),
            ..default_config()
        };
        assert_eq!(quiet.log_filter(false), "warn");
    }

    #[test]
    fn resolve_index_dir_defaults_to_the_workspace() {
        assert_eq!(
            default_config().resolve_index_dir(Path::new("/ws")),
            PathBuf::from("/ws/.rust-knowledge"),
            "no --index-dir: the workspace default"
        );
        let explicit = WorkspaceConfig {
            index_dir: Some(PathBuf::from("/explicit-idx")),
            ..default_config()
        };
        assert_eq!(
            explicit.resolve_index_dir(Path::new("/ws")),
            PathBuf::from("/explicit-idx"),
            "an explicit --index-dir wins over the workspace default"
        );
    }

    /// Non-UTF-8 argv entries are counted (the count drives the hard
    /// failure in [super::parse_std_args] — a skip would re-bind the
    /// surrounding flags) instead of panicking the process the way
    /// `std::env::args` would.
    #[test]
    #[cfg(unix)]
    fn utf8_argv_counts_non_utf8_entries_for_the_hard_failure() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![0xff, 0xfe]);
        let (argv, skipped) = utf8_argv(["ok".into(), invalid, "--flag".into()].into_iter());
        assert_eq!(argv, vec!["ok".to_string(), "--flag".to_string()]);
        assert_eq!(skipped, 1);
    }
}
