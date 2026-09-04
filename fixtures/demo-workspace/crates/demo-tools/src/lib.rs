//! demo-tools: small inspection helpers over demo-core.

use demo_core::writer::Writer;

/// Reports how many bytes a writer has buffered after writing the input.
pub fn measure(input: &[u8]) -> usize {
    let mut writer = Writer::new();
    writer.write_all(input);
    writer.pending()
}
