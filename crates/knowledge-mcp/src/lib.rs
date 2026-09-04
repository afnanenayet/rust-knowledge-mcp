//! MCP adapter over the rust-knowledge retrieval engine.
//!
//! The retrieval core lives in `knowledge-index`; this crate only wraps
//! the same [KnowledgeRetriever] calls behind three MCP tools. Search is
//! cheap; rustdoc/cargo are never invoked at request time.

pub mod server;

pub use server::KnowledgeServer;
