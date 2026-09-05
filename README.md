# rust-knowledge

A local, Cargo-aware documentation retrieval engine for Rust monorepos, built
for coding agents (Claude Code, Codex) and humans. It answers questions like
"what API should I use?", "how is this crate intended to work?" and "where is
this documented?" from the **exact resolved Cargo dependency universe** of your
workspace — so an agent never has to recursively crawl `~/.cargo/registry`,
`target/`, or generated files.

## What it does

- Runs `cargo metadata` and treats the resolved graph as the source of truth:
  every document is tied to an exact package identity (Cargo's opaque
  `PackageId`, not the crate name — two versions of one crate stay separate).
- Generates rustdoc JSON (nightly) for the workspace's Rust crates and parses
  it into documented API items: symbol paths, kinds, signatures, doc text,
  source spans, resolved intra-doc links.
- Ingests natural-language documentation: crate/module/item docs, READMEs,
  `docs/*.md`, chunked by heading structure with full provenance.
- Indexes everything into a local Tantivy index with weighted fields
  (identifier-heavy queries rank exact symbol matches extremely strongly).
- Serves compact previews: `search` → ids → `doc_read` on demand, keeping agent
  context usage small.

## Install and index

    cargo install --path crates/knowledge-cli
    rust-knowledge index --manifest-path ./Cargo.toml

rustdoc JSON generation requires a nightly toolchain (rustdoc JSON is
unstable): `rustup toolchain install nightly`. Indexing with
`--rustdoc-scope all` also documents resolved registry dependencies
(expensive once, then queries are cheap). The index lands in
`<workspace>/.rust-knowledge`.

## CLI

    rust-knowledge packages                       # the resolved universe
    rust-knowledge index                          # build the index
    rust-knowledge search "run blocking CPU work" # compact previews
    rust-knowledge get <document-id>              # full text + provenance
    rust-knowledge symbol Writer::write_all       # exact/near-exact lookup
    rust-knowledge dump-docs --package demo-core  # raw corpus (debug)
    rust-knowledge eval evals/queries.toml        # retrieval eval report

Every command takes `--manifest-path` and `--index-dir`.

## MCP

The same engine is exposed as an MCP server with three tools:

- `knowledge_search` — search documentation of the resolved graph, return compact
  previews with stable ids. Use it before touching source files.
- `doc_read` — retrieve the full text of one document by id, with
  package/version provenance, signature and source span.
- `symbol_lookup` — near-exact symbol lookup with structured API info.

Claude Code configuration (or Codex equivalent):

    {
      "mcpServers": {
        "rust-knowledge": {
          "command": "<path-to>/knowledge-mcp",
          "args": ["--manifest-path", "/path/to/workspace/Cargo.toml"]
        }
      }
    }

`RUST_KNOWLEDGE_INDEX_DIR` can replace `--index-dir`. Logs go to stderr;
stdout is the MCP channel.

## Cargo binary resolution

Every place the engine spawns cargo — `cargo metadata` for the dependency
universe and `cargo rustdoc` for JSON generation — resolves the binary
through one shared resolver (`knowledge_index::cargo::resolve`), so a
single index build can never mix two different cargo binaries. The
precedence order (first match wins):

1. `RUST_KNOWLEDGE_CARGO` — explicit escape hatch, unchanged. Wins for both
   call sites and suppresses the `+toolchain` argument: the caller
   controls the whole toolchain, including the rustdoc on PATH.
2. `$CARGO` — set by cargo when it invokes us (build scripts, cargo
   plugins, `cargo run`); honored for un-toolchained lookups so the engine
   stays consistent with the cargo that invoked it. When a toolchain is
   requested this tier is skipped: `$CARGO` always points at the invoking
   toolchain's concrete binary, which rejects rustup's proxy-only
   `+toolchain` argument.
3. Toolchain-consistent cargo via rustup: `rustup which --toolchain <t>
   cargo` for the requested toolchain, `rustup which cargo` for the active
   one. The `+toolchain` argument is suppressed here because rustup
   already pinned the toolchain. The toolchain's bin directory is
   prepended to the spawned process's `PATH` (mirroring what the rustup
   proxy does), so the concrete toolchain cargo finds its matching
   rustc/rustdoc. Degrades silently to the next tier when rustup is absent
   or lacks the toolchain.
4. `$CARGO_HOME/bin/cargo` — computed with the `home` crate, the library
   cargo itself uses, so a relocated `CARGO_HOME` is honored exactly the
   way cargo honors it; `$HOME/.cargo` is the default.
5. Plain `cargo` from PATH, resolved at spawn time.

When a toolchain is requested, tiers 4 and 5 keep the `+toolchain`
argument: those locations usually hold the rustup proxy, which understands
it (a non-proxy binary fails either way, since rustdoc JSON needs nightly).

Consequences worth knowing:

- Tier 3 preempts a PATH cargo whenever rustup is installed (for example a
  Nix shell that ships its own cargo alongside rustup). The escape hatches
  are `$CARGO` and `RUST_KNOWLEDGE_CARGO`.
- Empty or whitespace-only environment values count as unset.
- The chosen binary is logged (tracing) on first resolution and recorded in
  the index provenance (`index-meta.json` → `cargo_version`).

## Workflow for coding agents (see AGENTS.md)

1. Search documentation (`knowledge_search`) for APIs or conceptual questions.
2. Inspect the compact previews; only `doc_read` promising ids.
3. Open dependency source only when documentation is insufficient.
4. Do not recursively search `~/.cargo/registry` or `target/`.

## Architecture

    Cargo metadata ─> exact package identities (workspace/path/git/registry)
    rustdoc JSON ───> documented API items (symbol paths, signatures, spans)
    README/docs ───> heading-structured natural-language chunks
                      |
                      v
            normalized KnowledgeDocument corpus (deterministic ids)
                      v
            local Tantivy index (weighted fields, filters)
                      v
        KnowledgeRetriever trait  ─>  CLI  ─>  MCP

Four crates: `knowledge-core` (data model + retrieval trait, no engine deps),
`knowledge-index` (ingestion, corpus, Tantivy), `knowledge-cli`, `knowledge-mcp`.
See `docs/design.md` for the full design, upstream-API findings, and the
staged plan (including the designed-but-not-yet-built semantic retrieval).

## Tests

    cargo test        # unit + integration: fixture workspace, corpus, retrieval, MCP, eval

The integration suite runs against a committed fixture workspace
(`fixtures/demo-workspace`: two workspace members, one excluded path dependency,
registry deps, and two versions of base64 in one graph) with committed rustdoc
artifacts, so tests never need a nightly toolchain. The eval set
(`evals/queries.toml`) asserts retrieval quality: 21 agent-style queries
across known-symbol / API-discovery / conceptual / cross-package /
version-sensitive categories, each requiring a useful hit within its rank
threshold (currently 21/21, MRR ≈ 0.87).
