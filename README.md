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

Every command takes `--manifest-path` and `--index-dir`.

## MCP

The same engine is exposed as an MCP server with three tools:

- `knowledge_search` — search documentation of the resolved graph, return compact
  previews with stable ids. Use it before touching source files.
- `doc_read` — retrieve the full text of one document by id, with
  package/version provenance, signature and source span.
- `symbol_lookup` — near-exact symbol lookup with structured API info.

### One-time global registration (recommended)

Register the server once per machine with **no per-repo arguments**:

    claude mcp add --scope user rust-knowledge <path-to>/knowledge-mcp

or add the equivalent user-scope entry to `~/.claude.json`:

    {
      "mcpServers": {
        "rust-knowledge": {
          "command": "<path-to>/knowledge-mcp"
        }
      }
    }

Codex-style clients take the same form: put the no-argument command in your
user-level MCP config (for Codex CLI, `~/.codex/config.toml`).

With zero arguments the server infers the workspace from its working
directory: it walks up from cwd to the nearest `Cargo.toml` and serves that
workspace's index at `<workspace>/.rust-knowledge`, wherever in the checkout
cwd is (a member manifest resolves to its owning workspace via cargo
metadata). The **cwd contract**: the client must launch the server with its
working directory inside the project — Claude Code does this for stdio MCP
servers. The inferred workspace root and index directory are logged to
stderr at startup.

Per-repo `.mcp.json` entries are no longer needed with this registration;
existing ones keep working unchanged.

When inference cannot work the server exits immediately with the fix:

- no `Cargo.toml` at or above the working directory: pass
  `--manifest-path /path/to/workspace/Cargo.toml`, or launch it from
  inside a workspace;
- workspace found but index missing: run the exact command it prints, e.g.
  `rust-knowledge index --manifest-path /path/to/workspace/Cargo.toml`.

### Explicit per-repo configuration (still supported)

For clients that cannot set the working directory:

    {
      "mcpServers": {
        "rust-knowledge": {
          "command": "<path-to>/knowledge-mcp",
          "args": ["--manifest-path", "/path/to/workspace/Cargo.toml"]
        }
      }
    }

`--manifest-path` overrides cwd inference. `--index-dir` (or
`RUST_KNOWLEDGE_INDEX_DIR`) serves an existing index directly, without
resolving any workspace, so the working directory does not matter there.
`RUST_KNOWLEDGE_CARGO` overrides the cargo binary used for generation.
Logs go to stderr; stdout is the MCP channel. Multiple sessions may serve
the same index concurrently (the server opens it readers-only, no writer
lock).

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
