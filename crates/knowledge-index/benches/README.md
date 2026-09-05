# Engine benchmarks (`cargo bench -p knowledge-index`)

Criterion benchmarks over the **real engine stages**, on deterministic
inputs: the committed fixture workspace (`fixtures/demo-workspace`) with
its **prebuilt rustdoc JSON artifacts** — the same trick the integration
tests use — plus the committed 21-query eval set. No network, no nightly
toolchain, no generated inputs.

The eval harness (`evals/queries.toml` + `tests/eval.rs`) measures
*relevance*; this suite measures *cost*. Together they are the two gates
any perf-relevant engine change should pass.

**Benchmarks are not CI.** Nothing here runs automatically; a green CI
build says nothing about measured safety. If you touch a perf-sensitive
path (query construction, boosts, chunking, index writes, the request
path the tracing/config work will add), run this suite before and after
and compare.

## Running

```sh
cargo bench -p knowledge-index                  # full suite
cargo bench -p knowledge-index -- symbol_lookup # one group (regex filter)
cargo bench -p knowledge-index -- --list        # list bench ids
```

Stable Rust is enough (prebuilt artifacts; nightly is only needed for the
repo's clippy gate). A full run takes a few minutes on a laptop-class
Apple Silicon machine; the per-group settings below are part of that
tradeoff.

### Comparing against a baseline

```sh
cargo bench -p knowledge-index -- --save-baseline before
# ...make the change...
cargo bench -p knowledge-index -- --baseline before
```

`--baseline` fails loudly if a saved baseline is missing;
`--baseline-lenient` runs without comparing. Criterion also compares
against the *previous run* automatically. Results and plots land under
`target/criterion/<group>/<bench>/`.

### Reading the output

Each bench reports mean time per iteration with a confidence interval,
throughput where meaningful (docs/sec, queries/sec, lookups/sec), and a
change classification against the comparison run. Treat
`Regression`/`Improvement` verdicts with their p-values as the signal;
treat raw means as noisy until two runs agree.

## The suite

| Bench | Represents | A regression means |
| --- | --- | --- |
| `corpus/rustdoc_parse/<artifact>` | parse + normalize of one prebuilt rustdoc JSON (file read included), per resolved package | slower indexing per dependency; attribute per-artifact regressions to rustdoc parsing/normalization changes |
| `corpus/markdown_chunk/<pkg>@<ver>/<file>` | pulldown-cmark chunking of one discovered markdown file (reads excluded — covered by `build_full`) | slower README/docs ingestion |
| `corpus/build_full` | the whole corpus stage: all artifacts + all markdown (fixture **and** registry READMEs) + deterministic sort | slower `rust-knowledge index` overall |
| `index_build/from_scratch_dir` | `build_index` into a fresh dir: segment writes, commit, multithreaded merge (`wait_merging_threads`), `corpus.jsonl` + `index-meta.json` writes — docs/sec includes commit by design | slower index rebuilds and server cold starts |
| `search_eval/case_NN_<slug>` | one eval query, the exact `SearchQuery` `eval::run_eval` would issue (same text, same limit) | slower `knowledge_search` for that query shape (known symbol, natural language, version-sensitive) |
| `search_eval/mixed_workload` | all 21 eval queries per iteration (queries/sec) | the end-to-end search workload got slower |
| `symbol_lookup/exact` | fully-qualified symbol hit via the untokenized `symbol_exact` term | slower exact-path lookups |
| `symbol_lookup/bare_last_segment` | bare identifier ranked via the `symbol_last` term | slower "just the name" lookups |
| `symbol_lookup/qualified_conjunction` | qualified multi-token query that is *not* an exact path (`demo_core::writer::write_all`): the conjunction clause (MUST over every path token) does the ranking | slower "misremembered path" lookups |
| `doc_get/by_fixed_id` | `doc_read`: id term query + stored-field reconstruction of the full document | slower full-document fetches |
| `startup/open_and_first_query` | server startup path: meta load, schema check, tantivy open, reader + reload, first search (warm page cache; retriever teardown measured too) | slower MCP-server cold starts; compare with the per-case benches to split open vs query |

Per-case search ids (`case_NN_<slug>`) derive from the eval file's order
and text. **Editing `evals/queries.toml` breaks comparability** with
saved baselines — that file is the stable workload contract.

## Tuned settings and noise expectations

The suite deliberately uses two setting profiles:

* **CPU-bound retrieval groups** (`search_eval`, `symbol_lookup`,
  `doc_get`): sample_size 100, measurement 3 s, warm-up 1 s. These are the
  steadiest benches (warm page cache, no I/O): expect a few percent
  run-to-run scatter on a quiet laptop. `startup` uses the same sample
  size with measurement 5 s: one open+first-query iteration is ~0.6 ms,
  and 100 samples of that do not fit in 3 s.
* **I/O-bound build groups**: sample_size 10, warm-up 1 s. `corpus` gets
  measurement 10 s; `index_build` — the slowest, noisiest bench in the
  suite, with hundreds-of-ms iterations dominated by disk I/O and
  tantivy's multithreaded segment merges — gets 20 s so its ten samples
  fit. Expect double-digit-percent scatter *between machines* and several
  percent between runs. Fewer, longer samples keep the wall-clock cost
  practical without hiding the stage costs.

Honest expectations for laptop-class machines: run the suite twice before
trusting a change verdict on the I/O groups; keep the machine on AC power
and otherwise idle; treat small (`<5%`) deltas on `corpus` /
`index_build` as noise unless they reproduce. Criterion's statistical
comparison handles the rest.

If tantivy merge-thread variance ever becomes unbearable, the remedy is a
*recorded engine-side decision* (a thread-count option in
`tantivy_index`), not a silent harness workaround — none was needed to
get usable numbers.

## Determinism, hygiene, leakage

* Inputs are committed: the fixture workspace, its prebuilt rustdoc
  artifacts, `evals/queries.toml`. `RustdocScope::All` additionally reads
  the READMEs of registry packages (anyhow, base64 x2) from the local
  cargo registry cache, exactly like the integration tests; if those
  checkouts are missing the corpus benches fail loudly in setup rather
  than measuring a half-corpus.
* `cargo metadata` is a subprocess and runs **only in setup** — never
  inside a measured iteration.
* All writes go to `tempfile::tempdir()` scratch dirs: the persistent
  index the retrieval benches read, and a fresh dir per `index_build`
  iteration (created in setup, torn down outside the timed region via
  `BatchSize::PerIteration`). The harness never touches
  `<workspace>/.rust-knowledge`, default index locations, or
  `RUST_KNOWLEDGE_INDEX_DIR`, and it never mutates the fixture or the
  registry cache.
* `DocumentId` is a deterministic SHA-256 over semantic identity, so the
  doc-get bench measures a stable hit path.

## Known limitations

* The fixture corpus is **small by design** (a handful of packages).
  Per-document rates generalize imperfectly to huge workspaces, where
  fixed costs amortize differently; absolute numbers here are for
  before/after comparison, not capacity planning.
* The harness is shaped so a larger synthetic corpus can be added later
  without redesign: benches consume only the shared `BenchState`
  (`benches/support/mod.rs`), so a bigger `documents`/inputs source plugs
  in behind the same interface.
* `startup/open_and_first_query` measures a **warm** open (page cache);
  a genuinely cold first open is slower and is not what a long-lived
  server pays anyway.
* The suite does not spawn the MCP server binary or the stdio transport;
  `startup` covers the in-process startup path only.
