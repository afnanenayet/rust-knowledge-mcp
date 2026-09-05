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
[figue](https://facet.rs/figue/), and `docs/config-reference.html` documents
every command, flag, environment variable and default — generated from those
same shapes, so it cannot drift from the parsing code. After changing any
shape, regenerate it with `rust-knowledge config-docs` and commit the result.

figue deviates from the old clap parser in three permissive ways (recorded
here so nobody files them as regressions; every clap-era invocation behaves
exactly as before, and the deviations only affect argv that clap rejected):

- `--help`/`-h` and `--version`/`-V` are accepted after a subcommand name
  (`rust-knowledge search --version` exits 0; clap required them at the
  top level and exited 2).
- An explicit help or version token anywhere in argv wins even where a
  value was expected: `rust-knowledge --manifest-path --help` prints help
  and exits 0, where clap exited 2 (`--help` was the missing value).
- Hyphen-leading tokens are consumed as values where clap refused them:
  `rust-knowledge --index-dir -h packages` runs `packages` (the `-h` is
  the `--index-dir` value), and `search foo --package --help` filters by
  the literal `--help` instead of exiting 2.

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

`RUST_KNOWLEDGE_INDEX_DIR` can replace `--index-dir` in both binaries
(the flag always wins); `RUST_KNOWLEDGE_CARGO` overrides the cargo binary
used for generation. `RUST_KNOWLEDGE_LOG` (or `RUST_LOG` as fallback) sets
the tracing filter for both binaries, and `rust-knowledge --verbose` forces
debug logging over any environment setting. Logs go to stderr; stdout is the
MCP channel. See `docs/config-reference.html` for the full reference.

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
