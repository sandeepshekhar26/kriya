//! Governed MCP-server mode (roadmap **R1**).
//!
//! Exposes a verb app's registered actions as a Model Context Protocol server over stdio so
//! an *external* agent (Claude Desktop, Cursor, …) can drive the app — but **every
//! `tools/call` is routed through the same policy → approval → budget → signed-audit gates**
//! the in-process agent host enforces. That governed routing, not the raw tool exposure, is
//! the differentiator over a vanilla MCP server: the external agent gets capability, the host
//! keeps control.
//!
//! Layering:
//! - [`jsonrpc`] — wire framing and the MCP message shapes (no policy).
//! - the governor (next commit) — wraps the gates around a pluggable executor.
//! - the stdio serve loop (next commit) — reads JSON-RPC lines and dispatches.

pub mod jsonrpc;
