# demo-core guide

A longer walkthrough of the crate, kept in docs/ to exercise Markdown
discovery beyond the README.

## Getting started

Add demo-core to your workspace as a path dependency and call one of the
entry points from the runtime module. The crate has no async runtime of its
own: it is a library that other runtimes embed.

## Choosing a writer

### When to use Writer

Use Writer when you assemble a payload in memory and hand it off once. It
never performs IO and never fails. Buffering is cheap: bytes are appended to
a growable buffer.

### When to use AsyncWriter

Use the AsyncWriter trait when the destination is asynchronous: databases,
network sinks, anything where writing must not block the caller. Commit
makes writes durable. If your destination is slow, wrap it in
BatchingWriter, which coalesces writes.

### Example: combining them

A common pattern buffers a payload with Writer, flushes it into an
AsyncWriter, then commits once for the whole payload.

## Blocking and the runtime

The runtime schedules cooperatively, so blocking operations poison the
scheduler. CPU-heavy compression, hashing, parsing, image decoding: all of
these belong on an offload thread, not on the task itself. The runtime does
not detect violators; they simply show up as latency spikes in unrelated
tasks.

## Migration notes

Version 0.1 renamed Writer::pending from Writer::len. Older code should
switch; pending now never panics on empty writers.
