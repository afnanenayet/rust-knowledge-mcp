# Instructions for coding agents working in this repository

When investigating Rust dependencies of this workspace, do not recursively
search ~/.cargo/registry or target directories. The rust-knowledge MCP index
already covers the exact resolved Cargo dependency graph.

Use the MCP tools in this order:

1. knowledge_search — search documentation (READMEs, guides, rustdoc) for APIs
   or conceptual questions. Prefer documentation over dependency source code.
2. Inspect the compact previews; only doc_read the ids that look relevant.
3. symbol_lookup for exact/near-exact symbol questions (paths, signatures,
   source spans).
4. Retrieve dependency implementation source only when documentation is
   insufficient, using the source_path and source_span the tools return.

If the index is missing or stale, run: rust-knowledge index
