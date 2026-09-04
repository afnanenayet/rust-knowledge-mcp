# demo-app

The fixture application crate. It exists to make cross-package retrieval
questions answerable: it calls into demo-core, encodes with base64 0.22
(resolved for this package, while demo-core resolves 0.21), and propagates
errors with anyhow.

## Building payloads

Payloads are assembled through demo-core's Writer, flushed once, then base64
encoded. See build_payload.

## Error strategy

The app boundary uses anyhow::Result so any error from demo-core or base64
surfaces with context. Convert to your own error type at the edges.
