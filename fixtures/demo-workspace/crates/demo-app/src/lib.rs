//! demo-app wires the demo-core library into a small application.
//!
//! The application crate demonstrates cross-package questions an agent might
//! ask: how demo-app uses demo-core, which base64 version ended up in the
//! resolved graph (0.22, while demo-core resolved 0.21), and how errors are
//! propagated with anyhow.

use anyhow::Result;
use base64::Engine;

/// Default payload size for batched writes.
pub const DEFAULT_PAYLOAD_BYTES: usize = 4096;

/// Builds a fully encoded payload using demo-core and base64.
///
/// Data is buffered through demo-core's Writer, flushed once, and encoded
/// with the base64 0.22 engine resolved for this package. Returns the
/// encoded string ready for transmission.
pub fn build_payload(raw: &[u8]) -> Result<String> {
    let mut writer = demo_core::writer::Writer::new();
    writer.write_all(raw);
    let mut staged: Vec<u8> = Vec::new();
    writer.flush(&mut staged);
    Ok(base64::engine::general_purpose::STANDARD.encode(staged))
}

/// Reports how many live tasks demo-app considers healthy.
pub fn health_snapshot(guard: &demo_core::runtime::RuntimeGuard) -> u32 {
    guard.live_tasks
}
