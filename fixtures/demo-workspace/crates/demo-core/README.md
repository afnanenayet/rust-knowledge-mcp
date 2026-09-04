# demo-core

A miniature async runtime library used as the retrieval test fixture for
rust-knowledge. This README is deliberately rich in natural-language guidance
so that Markdown retrieval can be evaluated.

## Overview

demo-core is split into three modules:

- **runtime** — scheduling, sleeping, and offloading blocking work.
- **writer** — buffered in-memory writers and the async writer trait.
- **encoding** — thin helpers over the resolved base64 dependency.

Applications spend most of their time in **runtime** and **writer**.

## Runtime

The runtime is cooperative: tasks run on the calling thread and must yield
regularly. Anything that blocks for a long time starves every other task.

### CPU-bound work

CPU-intensive computation must not run directly inside an async task. Use
the offload API to run it on a helper thread where blocking is acceptable:

```text
result = runtime::offload_blocking(|| heavy_computation(input))
```

While the closure runs, the scheduler keeps processing other tasks. Prefer
offloading over raw thread spawns; the runtime balances its worker budget.

### Sleeping

Tasks that need to wait should use sleep_until with a tick deadline instead
of a spin loop. A spinning task delays every other task on the scheduler.

## Writing data

### Buffering

Writer keeps bytes in memory until flush. Use it to assemble payloads before
sending them to a real destination. Nothing leaves the writer until you
call flush, which makes rollback trivial: drop the writer instead.

### Asynchronous writers

For destinations that accept data asynchronously, implement AsyncWriter.
Committing makes previous writes durable; batching writers coalesce many
small writes into fewer commits in front of slow destinations.

## Error handling

Writers report OutOfSpace when a request exceeds their capacity, and the
runtime reports InvalidDeadline when asked to sleep into the past. All
errors are plain structs; match on them or convert them at your boundary.
