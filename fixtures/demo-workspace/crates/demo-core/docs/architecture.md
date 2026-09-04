# demo-core architecture

Notes on the internal structure of the fixture crate.

## Layering

demo-core is layered bottom-up:

1. encoding — pure functions, no state.
2. writer — in-memory state, no IO.
3. runtime — scheduling policy, depends on nothing above it.

## Invariants

The runtime module owns no threads in this fixture; it models policy, not
execution. Writers own their buffers exclusively and never alias.

## Testing strategy

Every module is unit-tested in src, and this fixture as a whole is indexed
by the rust-knowledge integration suite to verify retrieval quality.
