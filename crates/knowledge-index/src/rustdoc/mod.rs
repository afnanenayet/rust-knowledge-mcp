//! Rustdoc JSON: generation (behind a provider interface) and normalization.

pub mod normalize;
pub mod provider;
pub mod render;

pub use normalize::{RustdocNormalized, normalize};
pub use provider::{
    GeneratedRustdocProvider, GeneratedRustdocs, PrebuiltRustdocProvider, RustdocArtifact,
    RustdocProvider,
};
