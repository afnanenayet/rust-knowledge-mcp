# rust-knowledge

[![codecov](https://codecov.io/gh/afnanenayet/rust-knowledge-mcp/graph/badge.svg?token=Q8VRZY2HXH)](https://codecov.io/gh/afnanenayet/rust-knowledge-mcp)
[![Rust](https://github.com/afnanenayet/rust-knowledge-mcp/actions/workflows/rust.yml/badge.svg)](https://github.com/afnanenayet/rust-knowledge-mcp/actions/workflows/rust.yml)

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
    rust-knowledge config-docs                    # regenerate the reference below

Every command takes `--manifest-path` and `--index-dir` (also accepted after
the subcommand name). Both binaries define their whole argument surface with
[facet](https://facet.rs)-derived shapes parsed by
[figue](https://facet.rs/figue/), following its layered-configuration
recipe: a flattened config root layers CLI flags over the
`RUST_KNOWLEDGE_*` environment variables over built-in defaults,
`FigueBuiltins` contributes `--help`/`--version`/`--completions`/
`--html-help`/`--export-jsonschemas`, and the subcommands come from a
facet enum.

The interface is figue-native — issue #2 deliberately stopped emulating the
previous clap parser, so a few edges differ from it:

- Help, version and completions print to **stdout** and exit 0. A missing
  subcommand — or a missing positional such as `rust-knowledge search` —
  shows the relevant help plus a corrected-command suggestion and also
  exits 0.
- Usage errors (unknown flags, unknown subcommands, extra positionals)
  print figue diagnostics to **stderr** and exit 1 (the clap parser used 2).
- `--help`/`-h`/`--version`/`-V` bind at any level, including after a
  subcommand name.
- figue's config-root machinery adds `--config <FILE>` (JSON) and dotted
  `--config.<field> <VALUE>` overrides alongside the plain flags.

`docs/config-reference.html` is figue's own generated HTML help
(`figue::generate_html_help`) for the `rust-knowledge` shape — there is
no hand-rolled rendering. After changing any shape, regenerate it with
`rust-knowledge config-docs` and commit the result (a test pins the
committed file). `knowledge-mcp --help` documents that binary's surface.

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

The environment layer sits below the CLI in both binaries (flags always
win): `RUST_KNOWLEDGE_INDEX_DIR` replaces `--index-dir`,
`RUST_KNOWLEDGE_MANIFEST_PATH` replaces `--manifest-path`, and
`RUST_KNOWLEDGE_CARGO` (or `--cargo` in either binary) overrides the
cargo binary for every cargo invocation: indexing, rustdoc generation,
and the `cargo metadata` run that discovers the workspace when
`--index-dir` is absent (the typical MCP deployment passes only
`--manifest-path`, so that run happens). `RUST_KNOWLEDGE_LOG` —
falling back to `RUST_LOG` — sets the tracing filter (`--log` and
`rust-knowledge --verbose` override it; `-v` forces `debug`). The CLI
logs to stdout; the MCP server logs to stderr, and a usage error exits 1
with its diagnostic on stderr — stdout only ever carries MCP protocol
traffic. See `docs/config-reference.html` for the full reference.

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
