//! Governed MCP-server mode (roadmap **R1**).
//!
//! Exposes a kriya app's registered actions as a Model Context Protocol server over stdio so
//! an *external* agent (Claude Desktop, Cursor, …) can drive the app — but **every
//! `tools/call` is routed through the same policy → approval → budget → signed-audit gates**
//! the in-process agent host enforces. That governed routing, not the raw tool exposure, is
//! the differentiator over a vanilla MCP server: the external agent gets capability, the host
//! keeps control.
//!
//! Layering:
//! - [`jsonrpc`] — wire framing and the MCP message shapes (no policy).
//! - [`executor`] — the trait that actually runs a cleared action (Tauri / sidecar / CLI).
//! - [`approval`] — human-in-the-loop gates for guarded actions in MCP mode.
//! - [`governor`] — wraps policy → approval → budget → audit around the executor.
//! - [`server`] — the stdio JSON-RPC loop that exposes tools and routes calls.
//!
//! The thin `kriya-mcp` binary (`src/bin/kriya-mcp.rs`) wires these together from a policy
//! file + a tool-schema file + an executor command.

pub mod approval;
pub mod executor;
pub mod governor;
pub mod jsonrpc;
pub mod server;

// Front 1 — the stdio governance proxy (D-016). Off-by-default `mcp-client` feature so the library
// stays lean for in-process embedders that never proxy a downstream server.
#[cfg(feature = "mcp-client")]
pub mod client;
#[cfg(feature = "mcp-client")]
pub mod proxy_executor;
#[cfg(feature = "mcp-client")]
pub mod proxy_server;

// Front 2 — the reach-in adapter (service-architecture §5). Off-by-default `reach-in` feature so the
// default build pulls in no macOS AX FFI. The platform-agnostic core (AxBackend trait, tool
// synthesis, AxExecutor, ReachInServer) compiles on any OS for unit testing; the real AX backend
// (`reachin::macos`) is gated `#[cfg(target_os = "macos")]` inside the module.
#[cfg(feature = "reach-in")]
pub mod reachin;

// Front 3 — the computer-use fallback (service-architecture §6). Off-by-default `computer-use`
// feature; deferred / design-partner-gated. The thin governed seam only (no CV deps).
#[cfg(feature = "computer-use")]
pub mod computer_use;

#[cfg(target_os = "macos")]
pub use approval::GuiApproval;
pub use approval::{ApprovalGate, AutoApprove, DenyApproval, TtyApproval};
pub use executor::{
    ActionExecutor, ActionOutcome, FnExecutor, PersistentProcessExecutor, ProcessExecutor,
};
pub use governor::{DispatchOutcome, Governor};
pub use server::Server;

#[cfg(feature = "mcp-client")]
pub use client::McpClient;
#[cfg(feature = "mcp-client")]
pub use proxy_executor::McpProxyExecutor;
#[cfg(feature = "mcp-client")]
pub use proxy_server::ProxyServer;

// Front 2 public surface: the trait + node type, the synthesis entry points, the executor, and the
// serve loop. The macOS backend is re-exported only on macOS.
#[cfg(feature = "reach-in")]
pub use reachin::executor::AxExecutor;
#[cfg(all(feature = "reach-in", target_os = "macos"))]
pub use reachin::macos::MacAxBackend;
#[cfg(feature = "reach-in")]
pub use reachin::{AxBackend, AxNode, ReachInServer};

// Front 3 public surface.
#[cfg(feature = "computer-use")]
pub use computer_use::ComputerUseExecutor;
