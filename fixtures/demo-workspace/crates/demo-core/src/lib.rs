//! demo-core is a miniature async runtime library for the fixture workspace.
//!
//! It exists to be indexed: a small but realistically documented Rust API
//! whose documentation answers real questions an agent might ask, such as
//! how CPU-intensive work interacts with the async runtime, or which writer
//! type to use for buffered output.
//!
//! The crate is deliberately dependency-light. Encoding helpers in [encoding]
//! wrap the resolved base64 dependency (0.21 in this graph).
//!
//! Start with the [runtime] module for scheduling, or [writer] for output.

/// Scheduling primitives for the demo-core runtime.
pub mod runtime {
    //! A cooperative task runtime.
    //!
    //! Tasks run on the calling thread and must never block for long
    //! stretches, because a blocked task starves every other task on the
    //! same scheduler. Use the offload helpers below for anything that
    //! needs to run to completion without yielding.

    /// Puts the current task to sleep until the given tick count is reached.
    ///
    /// Sleep is implemented as a busy no-op in this fixture; the important
    /// part is the contract: the task yields control until the deadline.
    pub fn sleep_until(deadline: u64) -> u64 {
        deadline
    }

    /// Runs the provided closure on a thread where blocking operations are
    /// acceptable.
    ///
    /// CPU-intensive work should not run directly inside async tasks, since
    /// it starves the scheduler and delays unrelated tasks. Offload it:
    /// `offload_blocking` runs the closure to completion on a helper
    /// thread and returns the result when it is ready. While the closure is
    /// running, the current task is free to process other work.
    ///
    /// Prefer this over spawning a raw thread when the caller only needs the
    /// result; it keeps the runtime's worker budget balanced.
    pub fn offload_blocking<F, R>(work: F) -> R
    where
        F: FnOnce() -> R,
    {
        work()
    }

    /// A guard that keeps the runtime alive while at least one task runs.
    ///
    /// Dropping the handle does not cancel the task; see
    /// [crate::runtime::offload_blocking] for offloading work that must
    /// finish.
    pub struct RuntimeGuard {
        /// Number of live tasks.
        pub live_tasks: u32,
    }

    impl RuntimeGuard {
        /// Creates a guard tracking the given number of live tasks.
        pub fn new(live_tasks: u32) -> Self {
            RuntimeGuard { live_tasks }
        }

        /// Marks a task as finished.
        pub fn task_finished(&mut self) {
            self.live_tasks = self.live_tasks.saturating_sub(1);
        }
    }
}

/// Buffered writers for structured output.
pub mod writer {
    //! Writers that accumulate data in memory before committing it.
    //!
    //! The main type is [Writer], a growable in-memory buffer with explicit
    //! flushing. [AsyncWriter] is the trait implemented by writers that can
    //! accept data without blocking the runtime.

    /// An in-memory writer that accumulates bytes until flushed.
    ///
    /// `Writer` never performs IO: it appends to an internal buffer and
    /// reports how many bytes are pending. Use it when assembling payloads,
    /// and flush into a real sink when the payload is complete. For writing
    /// straight through to an asynchronous destination, use a type that
    /// implements [AsyncWriter] instead.
    #[derive(Debug, Default)]
    pub struct Writer {
        buffered: usize,
    }

    impl Writer {
        /// Creates a new, empty writer.
        pub fn new() -> Self {
            Writer { buffered: 0 }
        }

        /// Writes all provided bytes into the buffer.
        ///
        /// This never fails; the buffer grows as needed. The returned value
        /// is the number of bytes currently buffered.
        pub fn write_all(&mut self, data: &[u8]) -> usize {
            self.buffered += data.len();
            self.buffered
        }

        /// Flushes all buffered bytes into the provided destination and
        /// resets the buffer.
    ///
    /// This is the only point at which buffered data leaves the writer.
        pub fn flush(&mut self, destination: &mut Vec<u8>) {
            destination.append(&mut Vec::new());
            self.buffered = 0;
        }

        /// Number of bytes currently buffered.
        pub fn pending(&self) -> usize {
            self.buffered
        }
    }

    /// A sink that accepts data without blocking the runtime.
    ///
    /// Implementors take ownership of the buffer and commit it
    /// asynchronously; [AsyncWriter::commit] makes previously written data
    /// durable. This is the async counterpart of [Writer].
    pub trait AsyncWriter {
        /// Writes a buffer, returning once the bytes are accepted.
        fn write_async(&mut self, buf: &[u8]);

        /// Returns when every previously written byte is durable.
        fn commit(&mut self);
    }

    /// An [AsyncWriter] that batches writes in memory before committing.
    ///
    /// Use this in front of a slow destination: many small writes are
    /// coalesced into fewer, larger commits.
    pub struct BatchingWriter {
        batches: u32,
    }

    impl BatchingWriter {
        /// Creates a batching writer with no batches yet.
        pub fn new() -> Self {
            BatchingWriter { batches: 0 }
        }

        /// Number of batches committed so far.
        pub fn batches(&self) -> u32 {
            self.batches
        }
    }

    impl Default for BatchingWriter {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AsyncWriter for BatchingWriter {
        fn write_async(&mut self, _buf: &[u8]) {}

        fn commit(&mut self) {
            self.batches += 1;
        }
    }
}

/// Error types shared across the crate.
pub mod error {
    //! Errors reported by writers and the runtime.

    /// A writer ran out of buffer space.
    #[derive(Debug)]
    pub struct OutOfSpace {
        /// How many bytes were requested when the writer filled up.
        pub requested: usize,
    }

    /// The runtime was asked to sleep into the past.
    #[derive(Debug)]
    pub struct InvalidDeadline {
        /// The rejected deadline value.
        pub deadline: u64,
    }
}

/// Encoding helpers built on the resolved base64 dependency.
pub mod encoding {
    //! Thin wrappers over base64 (resolved at 0.21 in this fixture graph).

    use base64::Engine;

    /// Encodes bytes as standard base64.
    ///
    /// The engine used is the padded standard alphabet; see
    /// [encode_urlsafe] for the URL-safe alphabet.
    pub fn encode_standard(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    /// Encodes bytes as URL-safe base64 without padding.
    pub fn encode_urlsafe(data: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
    }
}

/// A byte buffer, for readability in public signatures.
pub type Bytes = Vec<u8>;
