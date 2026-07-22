//! Run correlation (S3) — the **one** vocabulary that links a receipt to its run and its parent
//! action, designed once so doc 26's I2 (source-set / kill-chain edges) and I5 (session ABOM
//! roll-up) extend it instead of growing a second one.
//!
//! ## Where it lives: a reserved `params` key
//!
//! Correlation rides a single reserved object key **inside a receipt's `params`**: [`RESERVED_KEY`]
//! = `"kriya.corr"`. Everything *outside* that key stays the agent's exact input, so "what the agent
//! sent" is always recoverable as `params` minus the reserved key. This honors doc 24 §4.2's
//! fidelity concern (which is why `kriya.io.*` is a *separate* receipt) while still living on the
//! action receipt — because correlation is **intrinsic** to the acting receipt (it *is* this
//! action's run + parent), and doc 26 I2 is written against exactly this shape ("each receipt gains
//! a source-set field"). A single reserved **dotted** `kriya.` key is one no tool argument uses
//! (tool args are bare identifiers like `command` / `url`), so there is no collision with real input.
//!
//! ## Frozen schema (doc 24 §7.0 law 1)
//!
//! Every field is **additive + optional**. [`attach`] with an empty [`Correlation`] returns the
//! params **unchanged**, so a receipt without correlation signs **byte-identically** to the pre-S3
//! format and every existing verifier keeps validating. The receipt struct itself is untouched —
//! this is vocabulary inside `params`, not a format change.
//!
//! ## Authoritative, not agent-forgeable
//!
//! On the hook lane `params` **is** agent-controlled `tool_input`, so an agent could plant a fake
//! `kriya.corr`. [`attach`] therefore treats the reserved key as **seam-authoritative**: it
//! overwrites any pre-existing `kriya.corr` with the seam-derived value, and when the seam has *no*
//! correlation it **strips** any agent-supplied `kriya.corr` — an agent can never forge its own run
//! lineage into a signed receipt.
//!
//! ## Redaction (design law 2)
//!
//! Because correlation is in `params`, the sealed envelope minimizer (which reads only `action_id` +
//! `success`) never sees it — run ids are structurally never in an off-device envelope, at any
//! verbosity. Fidelity on device, metadata off device.

use serde_json::Value;

/// The single reserved `params` object key that holds all run correlation. Dotted `kriya.` so it
/// cannot collide with a bare-identifier tool argument, and matches the reserved `kriya.*`
/// action-id namespace convention.
pub const RESERVED_KEY: &str = "kriya.corr";

/// The correlation a seam knows about an action. All fields optional — a seam records only what it
/// can actually see (doc 24 locus discipline: absent is honest, guessed is not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Correlation {
    /// The run/session this action belongs to — stable across every action in one agent
    /// invocation/session. Pseudonymous but linkable.
    pub run_id: Option<String>,
    /// The `step_id` of the parent action (the enclosing framework tool call). The explicit lineage
    /// edge; `None` for a root action and on seams with no parent pointer (e.g. the hook lane).
    pub parent_step_id: Option<String>,
    /// The sub-agent discriminator within a run, where the seam exposes one (Claude Code's hook
    /// `agent_id` differs for a spawned subagent) — lets the tree nest `run → subagent → actions`
    /// on a seam that has no parent-step pointer. `None` when the payload carries none.
    pub agent_id: Option<String>,
}

impl Correlation {
    /// A run-scoped correlation with no parent (the common root-action case).
    pub fn run(run_id: impl Into<String>) -> Self {
        Self {
            run_id: Some(run_id.into()),
            parent_step_id: None,
            agent_id: None,
        }
    }

    /// Set the parent step id (chainable). Empty strings are treated as absent.
    pub fn with_parent(mut self, parent_step_id: Option<String>) -> Self {
        self.parent_step_id = parent_step_id.filter(|s| !s.is_empty());
        self
    }

    /// Set the sub-agent id (chainable). Empty strings are treated as absent.
    pub fn with_agent_id(mut self, agent_id: Option<String>) -> Self {
        self.agent_id = agent_id.filter(|s| !s.is_empty());
        self
    }

    /// True when the seam knows nothing to correlate — [`attach`] is then a no-op (byte-identical).
    pub fn is_empty(&self) -> bool {
        self.run_id.is_none() && self.parent_step_id.is_none() && self.agent_id.is_none()
    }

    /// Build the reserved-key object value from the present fields. `None` when empty. (Key order
    /// here is irrelevant — `Signer::record` canonically sorts params before signing.)
    fn to_object(&self) -> Option<Value> {
        if self.is_empty() {
            return None;
        }
        let mut map = serde_json::Map::new();
        if let Some(run_id) = &self.run_id {
            map.insert("run_id".to_string(), Value::String(run_id.clone()));
        }
        if let Some(parent) = &self.parent_step_id {
            map.insert("parent_step_id".to_string(), Value::String(parent.clone()));
        }
        if let Some(agent_id) = &self.agent_id {
            map.insert("agent_id".to_string(), Value::String(agent_id.clone()));
        }
        Some(Value::Object(map))
    }
}

/// Parse a `Correlation` from JSON — the wire shape the `kriya-govern` bin receives from the SDK
/// middleware (`{"run_id": "...", "parent_step_id": "..."}`). Unknown fields are ignored; a
/// non-object (or absent) value yields an empty correlation. Empty/whitespace strings are treated as
/// absent so a middleware that always sends the key but sometimes leaves it blank stays honest.
pub fn from_json(v: Option<&Value>) -> Correlation {
    let obj = match v.and_then(Value::as_object) {
        Some(o) => o,
        None => return Correlation::default(),
    };
    let field = |k: &str| {
        obj.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.is_empty())
    };
    Correlation {
        run_id: field("run_id"),
        parent_step_id: field("parent_step_id"),
        agent_id: field("agent_id"),
    }
}

/// Stamp the seam-authoritative correlation into a receipt's `params`, returning the new params.
///
/// - Empty correlation **and** no pre-existing reserved key ⇒ returns `params` **unchanged**
///   (byte-identical — the frozen-schema guarantee).
/// - Empty correlation but a pre-existing (agent-supplied) `kriya.corr` ⇒ **strips** it (anti-forgery).
/// - Non-empty correlation ⇒ sets `params["kriya.corr"]` to the seam value, **overwriting** any
///   agent-supplied one.
///
/// Only attaches to an **object** `params`. A scalar/`null`/array `params` (a tool with no
/// object-shaped input) is returned unchanged — we never reshape the agent's value to carry
/// metadata; that receipt simply records no correlation (a rare edge, documented).
pub fn attach(params: Value, corr: &Correlation) -> Value {
    let obj_value = corr.to_object();
    match params {
        Value::Object(mut map) => {
            match obj_value {
                Some(v) => {
                    map.insert(RESERVED_KEY.to_string(), v);
                }
                None => {
                    // Anti-forgery: an agent must not be able to inject its own correlation.
                    map.remove(RESERVED_KEY);
                }
            }
            Value::Object(map)
        }
        // Non-object params: nothing to attach a key to. Never reshape agent data to carry metadata.
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_correlation_leaves_object_params_byte_identical() {
        let params = json!({ "command": "ls", "cwd": "/tmp" });
        let out = attach(params.clone(), &Correlation::default());
        assert_eq!(out, params, "no correlation must not touch params");
        // And it must not have introduced the reserved key.
        assert!(out.get(RESERVED_KEY).is_none());
    }

    #[test]
    fn run_only_attaches_reserved_key_and_preserves_tool_args() {
        let params = json!({ "command": "echo hi", "flag": true });
        let out = attach(params, &Correlation::run("run-abc"));
        // Tool args untouched.
        assert_eq!(out["command"], json!("echo hi"));
        assert_eq!(out["flag"], json!(true));
        // Reserved key carries run_id, no parent.
        assert_eq!(out[RESERVED_KEY]["run_id"], json!("run-abc"));
        assert!(out[RESERVED_KEY].get("parent_step_id").is_none());
    }

    #[test]
    fn run_and_parent_both_present() {
        let out = attach(
            json!({ "x": 1 }),
            &Correlation::run("run-1").with_parent(Some("step-parent".into())),
        );
        assert_eq!(out[RESERVED_KEY]["run_id"], json!("run-1"));
        assert_eq!(out[RESERVED_KEY]["parent_step_id"], json!("step-parent"));
    }

    #[test]
    fn empty_parent_string_is_treated_as_absent() {
        let out = attach(
            json!({}),
            &Correlation::run("r").with_parent(Some(String::new())),
        );
        assert_eq!(out[RESERVED_KEY]["run_id"], json!("r"));
        assert!(
            out[RESERVED_KEY].get("parent_step_id").is_none(),
            "an empty parent string must not become a real edge"
        );
    }

    #[test]
    fn seam_correlation_overwrites_an_agent_supplied_reserved_key() {
        // The hook lane's params IS agent-controlled tool_input — a forged kriya.corr must lose.
        let forged = json!({ "command": "ls", "kriya.corr": { "run_id": "ATTACKER", "parent_step_id": "ATTACKER" } });
        let out = attach(forged, &Correlation::run("real-run"));
        assert_eq!(out[RESERVED_KEY]["run_id"], json!("real-run"));
        assert!(
            out[RESERVED_KEY].get("parent_step_id").is_none(),
            "the forged parent must be gone, replaced wholesale by the seam value"
        );
        assert_eq!(out["command"], json!("ls"), "real tool args survive");
    }

    #[test]
    fn empty_correlation_strips_an_agent_forged_reserved_key() {
        // No seam correlation, but the agent tried to plant one: it must be removed, not trusted.
        let forged = json!({ "command": "ls", "kriya.corr": { "run_id": "ATTACKER" } });
        let out = attach(forged, &Correlation::default());
        assert!(
            out.get(RESERVED_KEY).is_none(),
            "an agent-forged correlation must be stripped when the seam has none"
        );
        assert_eq!(out["command"], json!("ls"));
    }

    #[test]
    fn non_object_params_are_left_untouched() {
        // A tool whose input is null/scalar/array: we never reshape it to carry metadata.
        for v in [json!(null), json!("a string"), json!([1, 2, 3]), json!(42)] {
            let out = attach(v.clone(), &Correlation::run("r"));
            assert_eq!(out, v, "non-object params must be returned unchanged");
        }
    }

    #[test]
    fn agent_id_is_carried_as_the_subagent_discriminator() {
        let out = attach(
            json!({ "command": "echo hi" }),
            &Correlation::run("sess-1").with_agent_id(Some("subagent-abc".into())),
        );
        assert_eq!(out[RESERVED_KEY]["run_id"], json!("sess-1"));
        assert_eq!(out[RESERVED_KEY]["agent_id"], json!("subagent-abc"));
        assert!(out[RESERVED_KEY].get("parent_step_id").is_none());
        assert_eq!(out["command"], json!("echo hi"));
    }

    /// Cross-version parity (runtime side): a receipt whose params carry `kriya.corr`, signed by the
    /// REAL `audit::Signer`, must re-verify — proving correlation rides the frozen schema and the
    /// canonical signing rules unchanged (the Console's TS verifier re-checks the identical bytes).
    #[test]
    fn a_signed_receipt_carrying_correlation_reverifies() {
        use crate::audit::{Actor, Receipt, Signer};
        use ed25519_dalek::{Signature, Verifier, VerifyingKey};

        let s = Signer::with_log_path(
            std::env::temp_dir().join(format!("kriya-corr-rt-{}.jsonl", uuid::Uuid::new_v4())),
        );
        let params = attach(
            json!({ "command": "ls" }),
            &Correlation::run("sess-1").with_agent_id(Some("sub-A".into())),
        );
        let signed = s.record(
            Receipt::new("s1".into(), "claude-code__bash".into(), params, true, 1)
                .with_actor(Some(Actor::new("claude-code", "ci"))),
        );
        // The correlation is present in the signed receipt.
        assert_eq!(
            signed.receipt.params["kriya.corr"]["run_id"],
            json!("sess-1")
        );

        // Re-derive the canonical bytes and verify against the embedded key (what every verifier does).
        let pk: [u8; 32] = hex::decode(&signed.public_key).unwrap().try_into().unwrap();
        let sig: [u8; 64] = hex::decode(&signed.signature).unwrap().try_into().unwrap();
        let msg = serde_json::to_vec(&signed.receipt).unwrap();
        assert!(
            VerifyingKey::from_bytes(&pk)
                .unwrap()
                .verify(&msg, &Signature::from_bytes(&sig))
                .is_ok(),
            "a real signed correlated receipt must verify"
        );
    }

    #[test]
    fn from_json_round_trips_and_ignores_junk() {
        let c = from_json(Some(
            &json!({ "run_id": "R", "parent_step_id": "P", "agent_id": "A", "extra": 9 }),
        ));
        assert_eq!(c.run_id.as_deref(), Some("R"));
        assert_eq!(c.parent_step_id.as_deref(), Some("P"));
        assert_eq!(c.agent_id.as_deref(), Some("A"));

        // Absent / non-object / blank-string → empty.
        assert!(from_json(None).is_empty());
        assert!(from_json(Some(&json!("nope"))).is_empty());
        assert!(from_json(Some(&json!({ "run_id": "" }))).is_empty());
    }
}
