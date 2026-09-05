//! Shared cargo-binary discovery.
//!
//! Every place the engine spawns cargo — `cargo metadata` in
//! [`CargoUniverse::load`](super::CargoUniverse::load)) and rustdoc JSON
//! generation in `crate::rustdoc::provider` — resolves the binary through
//! [`resolve`], so a single index build can never mix two different cargo
//! binaries. Resolution order (first match wins):
//!
//! 1. `RUST_KNOWLEDGE_CARGO` — explicit escape hatch, unchanged. Suppresses
//!    the `+toolchain` argument: the caller controls the whole toolchain,
//!    including the rustdoc on PATH.
//! 2. `$CARGO` — set by cargo when it invokes us (build scripts, cargo
//!    plugins, `cargo run`); honoring it for un-toolchained lookups keeps
//!    us consistent with the cargo that invoked us, exactly as the
//!    cargo_metadata crate already does. When a toolchain is requested,
//!    this tier is skipped: `$CARGO` is always the invoking toolchain's
//!    *real* binary (cargo sets it to its own path), which cannot honor
//!    the proxy-only `+<toolchain>` argument, so the rustup tier resolves
//!    the requested toolchain instead.
//! 3. Toolchain-consistent cargo via rustup: `rustup which --toolchain <t>
//!    cargo` for the requested toolchain, `rustup which cargo` for the
//!    active one. The `+toolchain` argument is suppressed here: rustup
//!    returns the concrete toolchain binary — not the rustup proxy — which
//!    rejects the proxy-only `+<toolchain>` argument, and the lookup itself
//!    already pins the toolchain. Because that concrete binary resolves
//!    rustc/rustdoc through `PATH`, the invocation also carries the
//!    toolchain's bin directory to prepend to the spawned process's
//!    `PATH` — the environment the rustup proxy would otherwise prepare
//!    (without it, nightly cargo run with a stable rustdoc first on
//!    `PATH` fails on `-Zunstable-options`).
//! 4. `$CARGO_HOME/bin/cargo`, computed with the `home` crate — the library
//!    cargo itself uses — so a relocated `CARGO_HOME` is honored exactly as
//!    cargo honors it, with `$HOME/.cargo` as the default.
//! 5. Plain `cargo` from PATH, resolved at spawn time.
//!
//! When a toolchain is requested, tiers 4 and 5 pass `+<toolchain>`: those
//! locations usually hold the rustup proxy, which understands the argument
//! (a non-proxy binary fails either way, since rustdoc JSON needs nightly).
//! Tiers 1 and 3 suppress it: tier 1 by documented contract, tier 3 because
//! the rustup lookup already selected the concrete toolchain binary.
//!
//! Empty or whitespace-only environment values are treated as unset, and
//! non-UTF-8 values are ignored. rustup is never required: when it is not
//! installed, not on PATH, or missing the requested toolchain, resolution
//! falls through silently to the next tier.
//!
//! [`resolve`] caches its result — and the resolved binary's `--version`
//! output — for the process, keyed by requested toolchain: the rustup probe
//! is a process spawn, runs at most once per toolchain, and never runs at
//! all when tiers 1 or 2 win.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::{debug, info};

/// Which tier of the precedence order produced a resolved cargo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CargoSource {
    /// The `RUST_KNOWLEDGE_CARGO` environment variable.
    Explicit,
    /// The `$CARGO` environment variable.
    CargoEnv,
    /// `rustup which [--toolchain <t>] cargo`.
    Rustup,
    /// `$CARGO_HOME/bin/cargo` (via the `home` crate).
    CargoHome,
    /// Plain `cargo`, resolved from PATH at spawn time.
    Path,
}

/// A resolved cargo invocation: the binary plus the arguments that must
/// precede any real cargo arguments (at most one `+<toolchain>`) and any
/// environment the spawn needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCargo {
    /// Program to spawn: a path for tiers 1-4, bare `cargo` for tier 5.
    pub program: PathBuf,
    /// Arguments inserted before the real cargo arguments, e.g.
    /// `["+nightly"]`. Empty when no toolchain was requested or when the
    /// source already pins the toolchain (tiers 1 and 3).
    pub pre_args: Vec<String>,
    /// Directory to prepend to the spawned process's `PATH` so the cargo
    /// finds the rustc/rustdoc of the toolchain it belongs to. Set only by
    /// the rustup tier: it spawns the concrete toolchain binary directly,
    /// and a toolchain cargo resolves rustc and rustdoc through `PATH` —
    /// the rustup proxy would otherwise have prepared this environment
    /// (verified: nightly cargo spawned without this documents with the
    /// stable rustdoc and fails on `-Zunstable-options`).
    pub path_prepend: Option<PathBuf>,
    /// Which precedence tier produced this invocation.
    pub source: CargoSource,
}

/// Everything [`resolve_with`] depends on, injected so unit tests never
/// spawn a real cargo or rustup.
pub struct ResolveInputs<'a> {
    /// Requested toolchain (e.g. "nightly"); None = the active toolchain.
    pub toolchain: Option<&'a str>,
    /// Raw `RUST_KNOWLEDGE_CARGO` value; empty/whitespace counts as unset.
    pub explicit_cargo: Option<&'a str>,
    /// Raw `$CARGO` value; empty/whitespace counts as unset.
    pub cargo_env: Option<&'a str>,
    /// Ask rustup for the cargo of the given toolchain (the active one
    /// when None). Returns None when rustup is absent or fails.
    pub rustup_which: &'a dyn Fn(Option<&str>) -> Option<PathBuf>,
    /// `$CARGO_HOME/bin/cargo` when it exists (via the `home` crate);
    /// None when the cargo home is unknown or holds no cargo binary.
    pub cargo_home_bin: &'a dyn Fn() -> Option<PathBuf>,
}

/// Pure resolution decision over injected inputs. See the
/// [module docs](self) for the precedence order and the `+toolchain`
/// semantics; this function performs no I/O.
pub fn resolve_with(inputs: &ResolveInputs<'_>) -> ResolvedCargo {
    // Tier 1: explicit override wins and suppresses `+toolchain`: the
    // caller controls the toolchain, including the rustdoc on PATH.
    if let Some(explicit) = non_empty_trimmed(inputs.explicit_cargo) {
        return ResolvedCargo {
            program: PathBuf::from(explicit),
            pre_args: Vec::new(),
            path_prepend: None,
            source: CargoSource::Explicit,
        };
    }
    // A requested toolchain becomes a proxy-style `+t` pre-argument for
    // the tiers whose binary may be a rustup proxy; the rustup tier
    // suppresses it because its lookup already pins the toolchain.
    let pre_args = toolchain_pre_args(inputs.toolchain);
    // Tier 2: the cargo that invoked us (build scripts, cargo plugins) —
    // for lookups that did not pin a toolchain, matching what the
    // cargo_metadata crate already does. A toolchain request skips this
    // tier: $CARGO is the invoking toolchain's real binary, which cannot
    // honor the proxy-only `+t` argument, and the rustup tier resolves to
    // the requested toolchain's binary without it (never worse: when the
    // invoking cargo already runs the requested toolchain, both tiers
    // resolve to the very same binary).
    if inputs.toolchain.is_none()
        && let Some(cargo_env) = non_empty_trimmed(inputs.cargo_env)
    {
        return ResolvedCargo {
            program: PathBuf::from(cargo_env),
            pre_args,
            path_prepend: None,
            source: CargoSource::CargoEnv,
        };
    }
    // Tier 3: the cargo that goes with the requested (or active) toolchain.
    // The spawned binary is the concrete toolchain cargo, which resolves
    // rustc/rustdoc through PATH; carry the toolchain's bin directory (the
    // parent of the resolved binary) so spawns can mirror the environment
    // the rustup proxy would have prepared.
    if let Some(path) = (inputs.rustup_which)(inputs.toolchain)
        && !path.as_os_str().is_empty()
    {
        let path_prepend = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf);
        return ResolvedCargo {
            program: path,
            pre_args: Vec::new(),
            path_prepend,
            source: CargoSource::Rustup,
        };
    }
    // Tier 4: cargo home, computed the same way cargo computes it.
    if let Some(path) = (inputs.cargo_home_bin)() {
        return ResolvedCargo {
            program: path,
            pre_args,
            path_prepend: None,
            source: CargoSource::CargoHome,
        };
    }
    // Tier 5: plain PATH cargo.
    ResolvedCargo {
        program: PathBuf::from("cargo"),
        pre_args,
        path_prepend: None,
        source: CargoSource::Path,
    }
}

/// Trims an environment value; None when unset, empty, or whitespace-only.
fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// The proxy-style pre-arguments for a requested toolchain.
fn toolchain_pre_args(toolchain: Option<&str>) -> Vec<String> {
    match toolchain {
        Some(toolchain) => vec![format!("+{toolchain}")],
        None => Vec::new(),
    }
}

/// A shared, process-cached cargo resolution with its `--version` output
/// cached after the first query.
pub struct CachedCargo {
    resolved: ResolvedCargo,
    version: OnceLock<Option<String>>,
}

impl CachedCargo {
    /// The resolved invocation.
    pub fn resolved(&self) -> &ResolvedCargo {
        &self.resolved
    }

    /// The invocation as an argv prefix (program first) for display in
    /// diagnostics. Paths are rendered lossily; spawns should use
    /// [`CachedCargo::resolved`] instead.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = vec![self.resolved.program.display().to_string()];
        argv.extend(self.resolved.pre_args.iter().cloned());
        argv
    }

    /// The `cargo --version` output of the resolved binary, spawned at
    /// most once per process per requested toolchain.
    pub fn version(&self) -> Option<String> {
        self.version.get_or_init(|| self.query_version()).clone()
    }

    /// A `Command` for the resolved invocation: the binary, any
    /// `+<toolchain>` pre-args, and — when the rustup tier won — the
    /// toolchain's bin directory prepended to the child's `PATH`, so the
    /// concrete toolchain cargo finds its matching rustc/rustdoc instead
    /// of whatever the ambient `PATH` resolves first (the environment the
    /// rustup proxy prepares itself). Other tiers spawn binaries that do
    /// their own toolchain handling, so they carry no `PATH` requirement.
    pub fn command(&self) -> Command {
        let resolved = &self.resolved;
        let mut cmd = Command::new(&resolved.program);
        for arg in &resolved.pre_args {
            cmd.arg(arg);
        }
        if let Some(dir) = &resolved.path_prepend {
            // Split the ambient PATH into segments, prepend the toolchain
            // bin dir, and re-join: join_paths validates segments, so the
            // whole path-list must never be passed as one segment.
            let mut segments: Vec<PathBuf> = vec![dir.clone()];
            if let Some(existing) = std::env::var_os("PATH") {
                segments.extend(std::env::split_paths(&existing));
            }
            // join_paths only fails when a segment contains the platform
            // list separator (a malformed PATH entry); skip the prepend
            // rather than fail the spawn.
            match std::env::join_paths(&segments) {
                Ok(path) => {
                    debug!(
                        dir = %dir.display(),
                        "prepending the toolchain bin dir to the cargo child's PATH"
                    );
                    cmd.env("PATH", path);
                }
                Err(cause) => {
                    debug!(error = %cause, "could not join PATH; spawning with ambient PATH");
                }
            }
        }
        cmd
    }

    /// `cargo --version` of the resolved binary, for index provenance.
    fn query_version(&self) -> Option<String> {
        let mut cmd = self.command();
        cmd.arg("--version");
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!version.is_empty()).then_some(version)
    }
}

/// Process-wide resolution cache, keyed by requested toolchain. The lock
/// is held across resolution so the rustup probe spawns at most once per
/// toolchain per process.
static CACHE: OnceLock<Mutex<HashMap<Option<String>, Arc<CachedCargo>>>> = OnceLock::new();

/// Resolves the cargo binary for the given toolchain (None = the active
/// toolchain, used by the `cargo metadata` call site) through the shared
/// precedence order, caching the result per process. Tracing-logs the
/// chosen binary and its source tier on the first resolution.
///
/// This is the single entry point every cargo spawn must go through; see
/// the [module docs](self) for the precedence order.
pub fn resolve(toolchain: Option<&str>) -> Arc<CachedCargo> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // The mutex is never held across a panic (resolution code cannot
    // panic), so poisoning is impossible; recover anyway rather than
    // fail the index build over a cache.
    let mut entries = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = toolchain.map(str::to_string);
    if let Some(hit) = entries.get(&key) {
        return Arc::clone(hit);
    }
    let explicit_cargo = env_var("RUST_KNOWLEDGE_CARGO");
    let cargo_env = env_var("CARGO");
    let resolved = resolve_with(&ResolveInputs {
        toolchain,
        explicit_cargo: explicit_cargo.as_deref(),
        cargo_env: cargo_env.as_deref(),
        rustup_which: &rustup_which,
        cargo_home_bin: &cargo_home_bin,
    });
    info!(
        cargo = %resolved.program.display(),
        source = ?resolved.source,
        toolchain = ?toolchain,
        "resolved cargo binary"
    );
    let entry = Arc::new(CachedCargo {
        resolved,
        version: OnceLock::new(),
    });
    entries.insert(key, Arc::clone(&entry));
    entry
}

/// Reads an environment variable; non-UTF-8 values are ignored (matching
/// the previous `std::env::var`-based behavior). Trimming and the
/// empty-means-unset rule happen in [`resolve_with`].
fn env_var(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

/// `rustup which [--toolchain <t>] cargo`: the concrete toolchain binary,
/// not the rustup proxy. Degrades silently (None) when rustup is absent,
/// not on PATH, fails, or the requested toolchain is not installed —
/// rustup is never required.
fn rustup_which(toolchain: Option<&str>) -> Option<PathBuf> {
    let mut cmd = Command::new("rustup");
    cmd.arg("which");
    if let Some(toolchain) = toolchain {
        cmd.arg("--toolchain").arg(toolchain);
    }
    cmd.arg("cargo");
    match cmd.output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!path.is_empty()).then(|| PathBuf::from(path))
        }
        Ok(output) => {
            debug!(
                toolchain = ?toolchain,
                code = ?output.status.code(),
                "rustup which failed; falling through to the next tier"
            );
            None
        }
        Err(cause) => {
            debug!(error = %cause, "rustup unavailable; falling through to the next tier");
            None
        }
    }
}

/// `$CARGO_HOME/bin/cargo` with the platform executable suffix, computed
/// exactly the way cargo computes its home (the `home` crate, with
/// `$HOME/.cargo` as the default); None when the home is unknown or holds
/// no cargo binary.
fn cargo_home_bin() -> Option<PathBuf> {
    let home = home::cargo_home().ok()?;
    let bin = home
        .join("bin")
        .join(format!("cargo{}", std::env::consts::EXE_SUFFIX));
    bin.is_file().then_some(bin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test double for the rustup probe: records every call (so tests can
    /// assert it was never consulted) and returns a canned result.
    struct RustupProbe {
        result: Option<PathBuf>,
        calls: std::cell::RefCell<Vec<Option<String>>>,
    }

    impl RustupProbe {
        fn new(result: Option<PathBuf>) -> Self {
            RustupProbe {
                result,
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn probe(&self, toolchain: Option<&str>) -> Option<PathBuf> {
            self.calls.borrow_mut().push(toolchain.map(str::to_string));
            self.result.clone()
        }

        fn calls(&self) -> Vec<Option<String>> {
            self.calls.borrow().clone()
        }
    }

    /// `resolve_with` with every tier controllable from one place; nothing
    /// here reads the real environment or spawns a real process.
    fn resolve_case(
        toolchain: Option<&str>,
        explicit_cargo: Option<&str>,
        cargo_env: Option<&str>,
        probe: &RustupProbe,
        home_bin: Option<PathBuf>,
    ) -> ResolvedCargo {
        resolve_with(&ResolveInputs {
            toolchain,
            explicit_cargo,
            cargo_env,
            rustup_which: &|t| probe.probe(t),
            cargo_home_bin: &|| home_bin.clone(),
        })
    }

    #[test]
    fn explicit_cargo_wins_over_every_tier_and_suppresses_toolchain() {
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/nightly/bin/cargo")));
        let resolved = resolve_case(
            Some("nightly"),
            Some("/opt/custom/cargo"),
            Some("/invoking/cargo"),
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::Explicit);
        assert_eq!(resolved.program, PathBuf::from("/opt/custom/cargo"));
        assert!(resolved.pre_args.is_empty());
        assert!(
            probe.calls().is_empty(),
            "explicit override must not spawn rustup"
        );
    }

    #[test]
    fn empty_or_whitespace_explicit_cargo_counts_as_unset() {
        let probe = RustupProbe::new(None);
        let resolved = resolve_case(
            None,
            Some("   "),
            Some("/invoking/cargo"),
            &probe,
            None,
        );
        assert_eq!(resolved.source, CargoSource::CargoEnv);
    }

    #[test]
    fn cargo_env_wins_over_rustup_for_untoolchained_lookups() {
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/stable/bin/cargo")));
        let resolved = resolve_case(
            None,
            None,
            Some("/invoking/cargo"),
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::CargoEnv);
        assert_eq!(resolved.program, PathBuf::from("/invoking/cargo"));
        assert!(resolved.pre_args.is_empty());
        assert!(
            probe.calls().is_empty(),
            "$CARGO must not spawn rustup for untoolchained lookups"
        );
    }

    #[test]
    fn cargo_env_is_skipped_when_a_toolchain_is_requested() {
        // $CARGO is the invoking toolchain's *real* binary, which rejects
        // the proxy-only `+toolchain` argument; the rustup tier resolves
        // the requested toolchain instead. This keeps `cargo run -- index`
        // working on a stable-active rustup machine.
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/nightly/bin/cargo")));
        let resolved = resolve_case(
            Some("nightly"),
            None,
            Some("/rustup/stable/bin/cargo"),
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::Rustup);
        assert_eq!(resolved.program, PathBuf::from("/rustup/nightly/bin/cargo"));
        assert!(resolved.pre_args.is_empty());
        assert_eq!(probe.calls(), vec![Some("nightly".to_string())]);
    }

    #[test]
    fn cargo_env_is_trimmed() {
        let probe = RustupProbe::new(None);
        let resolved = resolve_case(None, None, Some("  /invoking/cargo  "), &probe, None);
        assert_eq!(resolved.program, PathBuf::from("/invoking/cargo"));
    }

    #[test]
    fn empty_cargo_env_counts_as_unset() {
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/stable/bin/cargo")));
        let resolved = resolve_case(None, None, Some(""), &probe, None);
        assert_eq!(resolved.source, CargoSource::Rustup);
    }

    #[test]
    fn rustup_tier_pins_the_requested_toolchain_and_suppresses_the_arg() {
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/nightly/bin/cargo")));
        let resolved = resolve_case(
            Some("nightly"),
            None,
            None,
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::Rustup);
        assert_eq!(resolved.program, PathBuf::from("/rustup/nightly/bin/cargo"));
        assert!(
            resolved.pre_args.is_empty(),
            "rustup already pinned the toolchain; +toolchain would be rejected \
             by the concrete toolchain binary"
        );
        assert_eq!(
            resolved.path_prepend.as_deref(),
            Some(std::path::Path::new("/rustup/nightly/bin")),
            "the concrete toolchain cargo must be spawned with its own bin \
             dir first on PATH so it finds the matching rustc/rustdoc"
        );
        assert_eq!(probe.calls(), vec![Some("nightly".to_string())]);
    }

    #[test]
    fn rustup_tier_asks_for_the_active_toolchain_when_none_is_requested() {
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/stable/bin/cargo")));
        let resolved = resolve_case(None, None, None, &probe, None);
        assert_eq!(resolved.source, CargoSource::Rustup);
        assert!(resolved.pre_args.is_empty());
        assert_eq!(
            resolved.path_prepend.as_deref(),
            Some(std::path::Path::new("/rustup/stable/bin"))
        );
        assert_eq!(probe.calls(), vec![None]);
    }

    #[test]
    fn only_the_rustup_tier_carries_a_path_prepend() {
        // The concrete toolchain binary needs its bin dir prepended to
        // PATH; every other tier spawns a binary that does its own
        // toolchain handling (explicit binary, the invoking cargo, the
        // $CARGO_HOME proxy, or a PATH lookup).
        let probe = RustupProbe::new(Some(PathBuf::from("/rustup/nightly/bin/cargo")));
        assert!(
            resolve_case(None, Some("/opt/custom/cargo"), None, &probe, None)
                .path_prepend
                .is_none(),
            "explicit override: the caller owns the whole environment"
        );
        assert!(
            resolve_case(None, None, Some("/invoking/cargo"), &probe, None)
                .path_prepend
                .is_none(),
            "$CARGO: the invoking cargo already prepared the environment"
        );
        let rustup = resolve_case(None, None, None, &probe, None);
        assert_eq!(
            rustup.path_prepend.as_deref(),
            Some(std::path::Path::new("/rustup/nightly/bin"))
        );
        let no_rustup = RustupProbe::new(None);
        assert!(
            resolve_case(None, None, None, &no_rustup, Some(PathBuf::from("/home/u/.cargo/bin/cargo")))
                .path_prepend
                .is_none(),
            "$CARGO_HOME/bin/cargo is the rustup proxy; it prepares its own env"
        );
        assert!(
            resolve_case(None, None, None, &no_rustup, None)
                .path_prepend
                .is_none(),
            "PATH cargo: resolved by the OS at spawn time"
        );
    }

    #[test]
    fn missing_rustup_falls_through_to_cargo_home() {
        let probe = RustupProbe::new(None);
        let resolved = resolve_case(
            Some("nightly"),
            None,
            None,
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::CargoHome);
        assert_eq!(resolved.program, PathBuf::from("/home/u/.cargo/bin/cargo"));
        assert_eq!(resolved.pre_args, vec!["+nightly".to_string()]);
    }

    #[test]
    fn empty_rustup_output_falls_through_to_cargo_home() {
        let probe = RustupProbe::new(Some(PathBuf::new()));
        let resolved = resolve_case(
            None,
            None,
            None,
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        assert_eq!(resolved.source, CargoSource::CargoHome);
    }

    #[test]
    fn path_fallback_keeps_the_toolchain_arg() {
        let probe = RustupProbe::new(None);
        let resolved = resolve_case(Some("nightly"), None, None, &probe, None);
        assert_eq!(resolved.source, CargoSource::Path);
        assert_eq!(resolved.program, PathBuf::from("cargo"));
        assert_eq!(resolved.pre_args, vec!["+nightly".to_string()]);
    }

    #[test]
    fn path_fallback_without_a_requested_toolchain_has_no_pre_args() {
        let probe = RustupProbe::new(None);
        let resolved = resolve_case(None, None, None, &probe, None);
        assert_eq!(resolved.source, CargoSource::Path);
        assert_eq!(resolved.program, PathBuf::from("cargo"));
        assert!(resolved.pre_args.is_empty());
    }

    #[test]
    fn cargo_home_pre_args_follow_the_requested_toolchain() {
        let probe = RustupProbe::new(None);
        let with = resolve_case(
            Some("nightly"),
            None,
            None,
            &probe,
            Some(PathBuf::from("/home/u/.cargo/bin/cargo")),
        );
        let without = resolve_case(None, None, None, &probe, None);
        // The cargo-home tier keeps the proxy-style argument when a
        // toolchain is requested (that location usually holds the rustup
        // proxy) and has nothing to add otherwise.
        assert_eq!(with.pre_args, vec!["+nightly".to_string()]);
        assert!(without.pre_args.is_empty());
    }
}
