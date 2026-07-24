//! F1 (doc 28 §F1) — Attested Local Inference: a governed reverse proxy in front of a local
//! OpenAI-compatible / Ollama-native inference server (Ollama, llama.cpp's server, vLLM's
//! OpenAI-compat endpoint, LM Studio, …). v1 scope is **observe + gate on model identity** —
//! deterministic re-execution / strong-mode determinism is doc 28 phase 3, not this item.
//!
//! Layering:
//! - [`manifest`] — resolve a served model NAME to a DIGEST, from where it already lives (an
//!   Ollama manifest, or an operator-precomputed file-hash cache) — never by hashing on the
//!   request path.
//! - [`receipts`] — the `kriya.model.{identity,gate,serve}` vocabulary, built on the existing
//!   signer path.
//! - [`proxy`] — the HTTP reverse proxy itself: a hand-rolled std `TcpListener` server (same idiom
//!   as [`crate::mcp::contain::ConnectProxy`] — no async runtime, no HTTP-server dependency),
//!   streaming passthrough for SSE/chunked responses, hashing bytes as they pass rather than
//!   buffering full bodies.
//!
//! Model-identity POLICY (the allow/warn/approval-required/deny decision) lives in
//! [`crate::permissions::ModelPolicy`] — the SAME `agent-policy.yaml` every other governed binary
//! loads, under an optional `model:` key, per the codebase's established policy-parity pattern.
//!
//! **Bypass honesty** (say this everywhere this module is described, per FRONTIER-EXECUTE-PROMPT.md
//! §3 F1's bypass-honesty requirement): this proxy governs ONLY clients that are pointed at it. A
//! process that connects straight to the upstream inference server (e.g. `http://localhost:11434`
//! directly) is invisible to it — exactly like `kriya-gateway run --`'s egress containment, this is
//! a channel a client must be configured to use, not a host-wide interception. Coverage for
//! direct-connection bypass is a watcher-ladder item (WATCHER-ROADMAP), not this one.

pub mod manifest;
pub mod proxy;
pub mod receipts;
