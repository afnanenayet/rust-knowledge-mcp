//! Stable, deterministic document identifiers.
//!
//! Document identity is derived from *semantic* identity (package id, source
//! kind, symbol or relative document path, section and chunk identity) — never
//! from vector offsets or insertion order, so rebuilding the same corpus
//! reproduces the same ids and cached references stay valid.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Unit separator used between identity parts.
const SEP: u8 = 0x1f;

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentId(String);

impl DocumentId {
    /// Derives an id from ordered identity parts.
    pub fn from_identity(parts: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                hasher.update([SEP]);
            }
            hasher.update(part.as_bytes());
        }
        let digest = hasher.finalize();
        // 16 bytes (32 hex chars): collision-free for this corpus size, compact
        // enough for an LLM to echo back verbatim.
        let mut hex = String::with_capacity(32);
        for byte in &digest[..16] {
            hex.push_str(&format!("{byte:02x}"));
        }
        DocumentId(hex)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rebuilds an id from its hex form (as stored in an index). Only accepts
    /// the shape this crate produces.
    pub fn from_raw(raw: impl Into<String>) -> Option<Self> {
        let raw = raw.into();
        (raw.len() == 32 && raw.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some(DocumentId(raw.to_lowercase()))
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for DocumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocumentId({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let a = DocumentId::from_identity(&["pkg", "kind", "sym", "0", "0"]);
        let b = DocumentId::from_identity(&["pkg", "kind", "sym", "0", "0"]);
        assert_eq!(a, b);
    }

    #[test]
    fn part_boundaries_are_unambiguous() {
        let a = DocumentId::from_identity(&["ab", "c"]);
        let b = DocumentId::from_identity(&["a", "bc"]);
        assert_ne!(a, b);
    }

    #[test]
    fn changes_with_any_part() {
        let base = ["pkg", "kind", "sym", "0", "0"];
        for i in 0..base.len() {
            let mut parts = base.to_vec();
            parts[i] = "different";
            let refs: Vec<&str> = parts.to_vec();
            assert_ne!(
                DocumentId::from_identity(&base),
                DocumentId::from_identity(&refs),
                "changing part {i} must change the id"
            );
        }
    }

    #[test]
    fn stable_hex_shape() {
        let id =
            DocumentId::from_identity(&["tokio", "rustdoc_item", "task::spawn_blocking", "", ""]);
        assert_eq!(id.as_str().len(), 32);
        assert!(id.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
