//! `agent-native-host` — the Rust half of the agent-native framework.
//!
//! Pairs with the `@agent-native/core` TypeScript SDK to turn a goal + a registry of
//! typed actions into a permission-checked, budget-bounded, cryptographically-audited
//! sequence of action calls against a desktop app.
//!
//! ## Quick start (Tauri 2 backend)
//!
//! ```no_run
//! use std::collections::HashMap;
//! use std::sync::{Arc, Mutex};
//!
//! use agent_native_host::{
//!     audit::Signer,
//!     permissions::Policy,
//!     protocol::{AgentActionResult, AgentApprovalResponse, AgentStartRequest},
//!     run_task, select_backend_with_default, ApprovalMap, HostSink, PendingMap, StepAdvanceMap,
//!     TauriSink,
//! };
//!
//! # struct AppState {
//! #     pending: PendingMap,
//! #     approvals: ApprovalMap,
//! #     advances: StepAdvanceMap,
//! #     policy: Arc<Policy>,
//! #     signer: Arc<Signer>,
//! # }
//! # #[derive(Default)] struct MyDeterministic;
//! # impl agent_native_host::Inference for MyDeterministic {
//! #     fn name(&self) -> &'static str { "deterministic" }
//! #     fn next_step(&mut self, _: &agent_native_host::StepContext) -> Result<agent_native_host::StepDecision, String> {
//! #         Ok(agent_native_host::StepDecision::Done { summary: "ok".into() })
//! #     }
//! # }
//! # fn wire(app: tauri::AppHandle, state: AppState, req: AgentStartRequest) {
//! let backend = select_backend_with_default(Box::new(MyDeterministic::default()));
//! // Wrap the Tauri handle in a HostSink; a sidecar/Electron host passes a stdio sink instead.
//! let sink: Arc<dyn HostSink> = Arc::new(TauriSink::new(app));
//! std::thread::spawn(move || {
//!     let _ = run_task(
//!         sink,
//!         state.pending,
//!         state.approvals,
//!         state.advances,
//!         state.policy,
//!         state.signer,
//!         backend,
//!         req,
//!     );
//! });
//! # }
//! ```

pub mod agent;
pub mod audit;
pub mod budget;
pub mod mcp;
pub mod memory;
pub mod permissions;
pub mod protocol;

pub use agent::inference::{
    select_backend_with_default, Inference, StepContext, StepDecision, StepRecord,
};
pub use agent::{run_task, ApprovalMap, HostSink, PendingMap, StepAdvanceMap, TauriSink};
