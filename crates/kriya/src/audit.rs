//! Signed audit trail. The host holds an Ed25519 key the agent never sees and signs a
//! receipt for every executed action. Receipts are appended to a JSONL log and can be
//! verified offline by anyone holding the public key.

use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

/// Identity of *who* took an action: the agent that proposed it and the human/operator
/// on whose behalf it ran. Carried **inside** the signed receipt so attribution is
/// tamper-evident — rewriting who-did-what invalidates the signature, exactly like
/// rewriting the params would (R8).
///
/// Both fields are free-form strings the host supplies. `agent` is typically a backend
/// name, an MCP client id, or a model id; `user` is an OS user, an SSO subject, or any
/// app-provided operator identity. This is the *primitive* — richer identity management
/// (SSO/OIDC, RBAC) is a separate, paid concern; the signed field stays in the open core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    /// Which agent drove the action — a backend name, MCP client id, or model id.
    pub agent: String,
    /// The human/operator identity the run acted for — an OS user, SSO subject, etc.
    pub user: String,
}

impl Actor {
    pub fn new(agent: impl Into<String>, user: impl Into<String>) -> Self {
        Self { agent: agent.into(), user: user.into() }
    }
}

/// Reserved `action_id` for the R13 on-device attestation receipt — a signed record that a
/// run was sealed (the inference backend made no remote egress). Recognizable by verifiers
/// and the console as an attestation rather than an app action.
pub const ATTESTATION_ON_DEVICE: &str = "kriya.attestation.on_device";

#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub step_id: String,
    pub action_id: String,
    pub params: Value,
    pub success: bool,
    pub ts_ms: u128,
    /// Who took the action (R8). Optional and **skipped when absent** so a receipt
    /// without attribution signs byte-identically to the original (pre-R8) format —
    /// every existing verifier (the offline CLI, the console's TS verifier, the 20
    /// real receipts cross-checked there) keeps validating unchanged. Declared last so
    /// the canonical serialization order of the original five fields is preserved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
}

impl Receipt {
    /// A receipt with no identity attribution (the pre-R8 shape).
    pub fn new(
        step_id: String,
        action_id: String,
        params: Value,
        success: bool,
        ts_ms: u128,
    ) -> Self {
        Self { step_id, action_id, params, success, ts_ms, actor: None }
    }

    /// Attach (or clear) the acting identity. Chainable on top of [`Receipt::new`].
    pub fn with_actor(mut self, actor: Option<Actor>) -> Self {
        self.actor = actor;
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SignedReceipt {
    #[serde(flatten)]
    pub receipt: Receipt,
    pub public_key: String,
    pub signature: String,
}

pub struct Signer {
    key: SigningKey,
    public_hex: String,
    log_path: PathBuf,
}

impl Signer {
    pub fn new() -> Self {
        Self::with_log_path(std::env::temp_dir().join("kriya-audit.jsonl"))
    }

    /// Mint a signer that appends to a specific log file. Useful for tests, demos, and
    /// any host that wants its audit trail somewhere other than the shared temp file.
    pub fn with_log_path(log_path: PathBuf) -> Self {
        let bytes: [u8; 32] = rand::random();
        let key = SigningKey::from_bytes(&bytes);
        let public_hex = hex::encode(key.verifying_key().to_bytes());
        Self { key, public_hex, log_path }
    }

    pub fn public_key(&self) -> &str {
        &self.public_hex
    }

    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    /// Sign a receipt and append it to the audit log. Returns the signed receipt.
    pub fn record(&self, mut receipt: Receipt) -> SignedReceipt {
        // Canonicalize before signing (R21): recursively sort `params` object keys so the signed
        // bytes never depend on a consumer's serde_json `preserve_order` feature. `serde_json::Value`
        // maps are already key-ordered in the current builds, so this is byte-identical today — it
        // just makes the guarantee explicit and robust if a dependency ever flips that feature. The
        // offline `tools/verify-receipts` applies the identical sort before re-deriving the bytes.
        receipt.params = canonical_value(&receipt.params);
        // Canonical bytes = compact JSON of the unsigned (now key-sorted) receipt.
        let msg = serde_json::to_vec(&receipt).unwrap_or_default();
        let signature = hex::encode(self.key.sign(&msg).to_bytes());
        let signed = SignedReceipt {
            receipt,
            public_key: self.public_hex.clone(),
            signature,
        };
        if let Ok(line) = serde_json::to_string(&signed) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
        signed
    }
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Recursively sort object keys in a JSON value so its serialization is deterministic regardless of
/// serde_json's `preserve_order` feature (R21). Applied to receipt `params` before signing so the
/// signed canonical bytes are reproducible by any verifier without depending on a build flag. Arrays
/// preserve order (semantic) but their object elements are sorted; scalars pass through unchanged.
fn canonical_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), canonical_value(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use serde_json::json;

    /// Re-derive the canonical bytes and verify a signed receipt against its own embedded
    /// public key — the same check the offline CLI and the console's TS verifier perform.
    fn verifies(signed: &SignedReceipt) -> bool {
        let pub_bytes: [u8; 32] = match hex::decode(&signed.public_key)
            .ok()
            .and_then(|b| b.try_into().ok())
        {
            Some(b) => b,
            None => return false,
        };
        let sig_bytes: [u8; 64] = match hex::decode(&signed.signature)
            .ok()
            .and_then(|b| b.try_into().ok())
        {
            Some(b) => b,
            None => return false,
        };
        let key = match VerifyingKey::from_bytes(&pub_bytes) {
            Ok(k) => k,
            Err(_) => return false,
        };
        let sig = Signature::from_bytes(&sig_bytes);
        let msg = serde_json::to_vec(&signed.receipt).unwrap();
        key.verify(&msg, &sig).is_ok()
    }

    fn signer() -> Signer {
        // Isolate the audit file per test process so concurrent tests don't fight over it.
        Signer::with_log_path(std::env::temp_dir().join("kriya-audit-test.jsonl"))
    }

    #[test]
    fn round_trip_without_actor_verifies() {
        let s = signer();
        let signed = s.record(Receipt::new(
            "step-1".into(),
            "create_note".into(),
            json!({ "title": "hi" }),
            true,
            1_700_000_000_000,
        ));
        assert!(signed.receipt.actor.is_none());
        assert!(verifies(&signed), "actor-less receipt must verify");
    }

    #[test]
    fn round_trip_with_actor_verifies() {
        let s = signer();
        let signed = s.record(
            Receipt::new(
                "step-2".into(),
                "categorize_transaction".into(),
                json!({ "id": "txn-1" }),
                true,
                1_700_000_000_001,
            )
            .with_actor(Some(Actor::new("claude-desktop", "alice"))),
        );
        assert_eq!(signed.receipt.actor, Some(Actor::new("claude-desktop", "alice")));
        assert!(verifies(&signed), "actor-bearing receipt must verify");
    }

    #[test]
    fn actorless_receipt_serializes_to_the_original_five_fields() {
        // The whole point of skip_if_none: byte-identical to the pre-R8 format, so the
        // existing verifiers (and the cross-checked real receipts) keep validating.
        let r = Receipt::new("s".into(), "a".into(), json!({}), true, 1);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"step_id":"s","action_id":"a","params":{},"success":true,"ts_ms":1}"#);
        assert!(!json.contains("actor"), "absent actor must not appear in the signed bytes");
    }

    #[test]
    fn actor_appears_last_when_present() {
        let r = Receipt::new("s".into(), "a".into(), json!({}), true, 1)
            .with_actor(Some(Actor::new("agentX", "userY")));
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"step_id":"s","action_id":"a","params":{},"success":true,"ts_ms":1,"actor":{"agent":"agentX","user":"userY"}}"#
        );
    }

    #[test]
    fn tampering_the_actor_breaks_the_signature() {
        let s = signer();
        let mut signed = s.record(
            Receipt::new("step-3".into(), "delete_transaction".into(), json!({}), true, 7)
                .with_actor(Some(Actor::new("claude-desktop", "alice"))),
        );
        assert!(verifies(&signed), "control: untampered must verify");
        // Forge a different operator after signing — the attribution is signed, so this fails.
        signed.receipt.actor = Some(Actor::new("claude-desktop", "mallory"));
        assert!(!verifies(&signed), "swapping the acting user must invalidate the receipt");
    }

    #[test]
    fn tampering_params_breaks_the_signature() {
        let s = signer();
        let mut signed = s.record(Receipt::new(
            "step-4".into(),
            "edit_note".into(),
            json!({ "amount": 10 }),
            true,
            9,
        ));
        signed.receipt.params = json!({ "amount": 1_000_000 });
        assert!(!verifies(&signed), "tampered params must invalidate the receipt");
    }

    /// A fresh, untampered signed receipt to mutate in the tamper tests below.
    fn baseline() -> (Signer, SignedReceipt) {
        let s = signer();
        let signed = s.record(Receipt::new(
            "step-x".into(),
            "delete_transaction".into(),
            json!({ "id": "txn-9", "amount": 250 }),
            true,
            1_700_000_000_123,
        ));
        assert!(verifies(&signed), "control: untampered receipt must verify");
        (s, signed)
    }

    #[test]
    fn tampering_the_action_id_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.action_id = "list_transactions".into(); // disguise a delete as a read
        assert!(!verifies(&signed), "rewriting which action ran must invalidate the receipt");
    }

    #[test]
    fn tampering_the_success_flag_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.success = false; // claim a successful action failed (or vice versa)
        assert!(!verifies(&signed), "flipping the outcome must invalidate the receipt");
    }

    #[test]
    fn tampering_the_step_id_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.step_id = "step-other".into();
        assert!(!verifies(&signed), "rewriting the step id must invalidate the receipt");
    }

    #[test]
    fn tampering_the_timestamp_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.ts_ms = 0; // backdate the action
        assert!(!verifies(&signed), "rewriting when it happened must invalidate the receipt");
    }

    #[test]
    fn adding_an_actor_after_signing_breaks_the_signature() {
        // The inverse of tampering an existing actor: an actor-less receipt was signed over five
        // fields, so attaching attribution afterward changes the canonical bytes and fails.
        let (_s, mut signed) = baseline();
        assert!(signed.receipt.actor.is_none());
        signed.receipt.actor = Some(Actor::new("forged-agent", "mallory"));
        assert!(!verifies(&signed), "fabricating attribution after signing must fail");
    }

    #[test]
    fn a_forged_signature_does_not_verify() {
        let (_s, mut signed) = baseline();
        // Flip the first hex nibble — still well-formed 64-byte hex, but not the real signature.
        let mut chars: Vec<char> = signed.signature.chars().collect();
        chars[0] = if chars[0] == '0' { '1' } else { '0' };
        signed.signature = chars.into_iter().collect();
        assert!(!verifies(&signed), "a forged signature must not verify");
    }

    #[test]
    fn a_mismatched_public_key_does_not_verify() {
        let (_s, mut signed) = baseline();
        // Swap in a *different* signer's public key — the signature was made by the original key,
        // so claiming a different signer produced it must fail (no key-substitution attack).
        let other = signer();
        signed.public_key = other.public_key().to_string();
        assert!(!verifies(&signed), "a receipt must not verify against the wrong public key");
    }

    #[test]
    fn malformed_signature_or_pubkey_hex_does_not_verify() {
        let (_s, mut signed) = baseline();
        let good_sig = signed.signature.clone();
        signed.signature = "not-hex".into();
        assert!(!verifies(&signed), "non-hex signature must be rejected, not panic");
        signed.signature = good_sig;
        signed.public_key = "deadbeef".into(); // valid hex but wrong length (not 32 bytes)
        assert!(!verifies(&signed), "wrong-length public key must be rejected, not panic");
    }

    #[test]
    fn params_are_canonically_key_sorted_before_signing() {
        // R21: params keys are recursively sorted into the signed (and stored) receipt — including
        // nested objects and objects inside arrays — so the canonical bytes don't depend on any
        // consumer's serde_json `preserve_order` feature.
        let s = signer();
        let signed = s.record(Receipt::new(
            "s".into(),
            "a".into(),
            json!({ "z": 1, "a": { "y": 2, "b": 3 }, "m": [ { "q": 1, "p": 2 } ] }),
            true,
            1,
        ));
        assert!(verifies(&signed), "canonicalized receipt must verify");
        let serialized = serde_json::to_string(&signed.receipt.params).unwrap();
        assert_eq!(serialized, r#"{"a":{"b":3,"y":2},"m":[{"p":2,"q":1}],"z":1}"#);
    }
}
