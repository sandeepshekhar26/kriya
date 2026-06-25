//! Signed audit trail. The host holds an Ed25519 key the agent never sees and signs a
//! receipt for every executed action. Receipts are appended to a JSONL log and can be
//! verified offline by anyone holding the public key.

use ed25519_dalek::{Signer as _, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
        Self {
            agent: agent.into(),
            user: user.into(),
        }
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
    /// Hash of the previous receipt LINE in this audit log (R20). Chains receipts so whole-receipt
    /// deletion / truncation / reorder is detectable — turning "no retained receipt was altered"
    /// into "the log is complete." Absent on the genesis receipt (and on pre-R20 receipts), so an
    /// unchained receipt signs **byte-identically** to before. Declared last so the canonical order
    /// of the original fields + `actor` is preserved; `prev_hash` is part of the signed bytes, so
    /// the chain pointer itself can't be rewritten without invalidating the signature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hash: Option<String>,
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
        Self {
            step_id,
            action_id,
            params,
            success,
            ts_ms,
            actor: None,
            prev_hash: None,
        }
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
    /// Hash of the last receipt line written to `log_path` — the chain head (R20). Seeded from the
    /// log's last line at construction so a new process continues the chain. Behind a `Mutex` so the
    /// chain stays consistent if multiple run threads share one signer.
    last_hash: Mutex<Option<String>>,
}

impl Default for Signer {
    fn default() -> Self {
        Self::new()
    }
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
        let last_hash = Mutex::new(seed_last_hash(&log_path));
        Self {
            key,
            public_hex,
            log_path,
            last_hash,
        }
    }

    /// Mint a signer whose Ed25519 identity is **persisted** at `key_path` — loaded if present,
    /// else generated and written (0600 on Unix). Unlike [`Signer::new`] / [`Signer::with_log_path`]
    /// (which mint an *ephemeral* per-process key), this gives a **stable trust anchor across runs**
    /// (R20): the public key an auditor pins stays the same deployment-to-deployment, so the audit
    /// trail is verifiable over months, not just within one session. Errors if the key file exists
    /// but is unreadable/invalid — a signing identity is never silently overwritten.
    pub fn with_identity(key_path: &Path, log_path: PathBuf) -> Result<Self, String> {
        let seed = load_or_create_seed(key_path)?;
        let key = SigningKey::from_bytes(&seed);
        let public_hex = hex::encode(key.verifying_key().to_bytes());
        let last_hash = Mutex::new(seed_last_hash(&log_path));
        Ok(Self {
            key,
            public_hex,
            log_path,
            last_hash,
        })
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
        // Hash-chain (R20): link this receipt to the previous LINE so whole-receipt deletion or
        // truncation is detectable. Hold the lock across the file write so the on-disk order always
        // matches the chain order, even if multiple run threads share this signer.
        let mut last = self.last_hash.lock().unwrap_or_else(|e| e.into_inner());
        receipt.prev_hash = last.clone(); // None on the genesis receipt
                                          // Canonical bytes = compact JSON of the unsigned (key-sorted, now chained) receipt.
        let msg = serde_json::to_vec(&receipt).unwrap_or_default();
        let signature = hex::encode(self.key.sign(&msg).to_bytes());
        let signed = SignedReceipt {
            receipt,
            public_key: self.public_hex.clone(),
            signature,
        };
        let line = serde_json::to_string(&signed).unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
        {
            let _ = writeln!(f, "{line}");
        }
        // The hash of the exact line just written becomes the next receipt's `prev_hash`.
        *last = Some(sha256_hex(line.as_bytes()));
        signed
    }
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// The standard on-device directory for kriya audit logs: `~/.kriya/audit/` (R27 / D-018). The
/// gateway defaults its signed-receipt log here so the control-plane Console can **auto-discover and
/// tail** governance with no manual file import — open the app, see your receipts. It is a shared
/// convention across the gateway (writer) and the Console (reader), so both compute it the same way.
/// The directory is created if missing. Falls back to the OS temp dir when no home directory is
/// resolvable (headless / unusual environments) so a signer always has a writable location rather
/// than silently dropping receipts.
pub fn default_audit_dir() -> PathBuf {
    match home_dir().map(|h| h.join(".kriya").join("audit")) {
        // Best-effort create; on failure (e.g. a read-only home) fall back to temp so the log still
        // lands somewhere writable.
        Some(dir) if std::fs::create_dir_all(&dir).is_ok() => dir,
        _ => std::env::temp_dir(),
    }
}

/// Resolve the user's home directory without pulling in a dependency: `$HOME` on Unix,
/// `%USERPROFILE%` on Windows. `None` if neither is set.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Load a 32-byte Ed25519 seed from `path` (lowercase hex), or generate one and persist it there
/// (creating parent dirs; restricted to 0600 on Unix). An existing-but-invalid key file is an
/// error, never overwritten — losing a durable signing identity must be a deliberate act, not a
/// side effect of a typo'd path (R20).
fn load_or_create_seed(path: &Path) -> Result<[u8; 32], String> {
    if path.exists() {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading signing key {}: {e}", path.display()))?;
        let bytes = hex::decode(text.trim())
            .map_err(|e| format!("signing key {} is not valid hex: {e}", path.display()))?;
        return bytes.try_into().map_err(|_| {
            format!(
                "signing key {} must be 32 bytes (64 hex chars)",
                path.display()
            )
        });
    }
    let seed: [u8; 32] = rand::random();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating key dir {}: {e}", parent.display()))?;
        }
    }
    std::fs::write(path, hex::encode(seed))
        .map_err(|e| format!("writing signing key {}: {e}", path.display()))?;
    restrict_perms(path);
    Ok(seed)
}

#[cfg(unix)]
fn restrict_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn restrict_perms(_path: &Path) {}

/// Lowercase-hex SHA-256 of `bytes`. The hash-chain links each receipt to the SHA-256 of the exact
/// previous LINE on disk, so any whole-receipt deletion/truncation/reorder breaks the chain (R20).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Seed the chain head from an existing log's last non-empty line, so a new host process CONTINUES
/// the chain (its first receipt links to the last line already on disk) instead of starting a fresh
/// chain that a verifier would read as a head-truncation. `None` for an absent/empty log — a genuine
/// genesis.
fn seed_last_hash(log_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(log_path).ok()?;
    let last = content.lines().rev().find(|l| !l.trim().is_empty())?;
    Some(sha256_hex(last.as_bytes()))
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
        // A UNIQUE log per call so each test's first record() is a genuine genesis (no chain seeding
        // from a leftover shared file) and concurrent tests never fight over the audit file.
        Signer::with_log_path(
            std::env::temp_dir().join(format!("kriya-audit-test-{}.jsonl", uuid::Uuid::new_v4())),
        )
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
        assert_eq!(
            signed.receipt.actor,
            Some(Actor::new("claude-desktop", "alice"))
        );
        assert!(verifies(&signed), "actor-bearing receipt must verify");
    }

    #[test]
    fn actorless_receipt_serializes_to_the_original_five_fields() {
        // The whole point of skip_if_none: byte-identical to the pre-R8 format, so the
        // existing verifiers (and the cross-checked real receipts) keep validating.
        let r = Receipt::new("s".into(), "a".into(), json!({}), true, 1);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"step_id":"s","action_id":"a","params":{},"success":true,"ts_ms":1}"#
        );
        assert!(
            !json.contains("actor"),
            "absent actor must not appear in the signed bytes"
        );
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
            Receipt::new(
                "step-3".into(),
                "delete_transaction".into(),
                json!({}),
                true,
                7,
            )
            .with_actor(Some(Actor::new("claude-desktop", "alice"))),
        );
        assert!(verifies(&signed), "control: untampered must verify");
        // Forge a different operator after signing — the attribution is signed, so this fails.
        signed.receipt.actor = Some(Actor::new("claude-desktop", "mallory"));
        assert!(
            !verifies(&signed),
            "swapping the acting user must invalidate the receipt"
        );
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
        assert!(
            !verifies(&signed),
            "tampered params must invalidate the receipt"
        );
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
        assert!(
            !verifies(&signed),
            "rewriting which action ran must invalidate the receipt"
        );
    }

    #[test]
    fn tampering_the_success_flag_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.success = false; // claim a successful action failed (or vice versa)
        assert!(
            !verifies(&signed),
            "flipping the outcome must invalidate the receipt"
        );
    }

    #[test]
    fn tampering_the_step_id_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.step_id = "step-other".into();
        assert!(
            !verifies(&signed),
            "rewriting the step id must invalidate the receipt"
        );
    }

    #[test]
    fn tampering_the_timestamp_breaks_the_signature() {
        let (_s, mut signed) = baseline();
        signed.receipt.ts_ms = 0; // backdate the action
        assert!(
            !verifies(&signed),
            "rewriting when it happened must invalidate the receipt"
        );
    }

    #[test]
    fn adding_an_actor_after_signing_breaks_the_signature() {
        // The inverse of tampering an existing actor: an actor-less receipt was signed over five
        // fields, so attaching attribution afterward changes the canonical bytes and fails.
        let (_s, mut signed) = baseline();
        assert!(signed.receipt.actor.is_none());
        signed.receipt.actor = Some(Actor::new("forged-agent", "mallory"));
        assert!(
            !verifies(&signed),
            "fabricating attribution after signing must fail"
        );
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
        assert!(
            !verifies(&signed),
            "a receipt must not verify against the wrong public key"
        );
    }

    #[test]
    fn malformed_signature_or_pubkey_hex_does_not_verify() {
        let (_s, mut signed) = baseline();
        let good_sig = signed.signature.clone();
        signed.signature = "not-hex".into();
        assert!(
            !verifies(&signed),
            "non-hex signature must be rejected, not panic"
        );
        signed.signature = good_sig;
        signed.public_key = "deadbeef".into(); // valid hex but wrong length (not 32 bytes)
        assert!(
            !verifies(&signed),
            "wrong-length public key must be rejected, not panic"
        );
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
        assert_eq!(
            serialized,
            r#"{"a":{"b":3,"y":2},"m":[{"p":2,"q":1}],"z":1}"#
        );
    }

    #[test]
    fn durable_identity_is_stable_across_runs() {
        // R20: a persisted key means the public identity an auditor pins stays the same run-to-run,
        // unlike the ephemeral with_log_path key. Two signers loading the same key file match.
        let dir = std::env::temp_dir().join(format!("kriya-r20a-{}", uuid::Uuid::new_v4()));
        let key = dir.join("signing.key");
        let log = dir.join("audit.jsonl");

        let s1 = Signer::with_identity(&key, log.clone()).expect("mint identity");
        let pk1 = s1.public_key().to_string();
        let s2 = Signer::with_identity(&key, log.clone()).expect("reload identity");
        assert_eq!(
            pk1,
            s2.public_key(),
            "persisted identity must be stable across runs"
        );

        let signed = s1.record(Receipt::new("s".into(), "a".into(), json!({}), true, 1));
        assert!(verifies(&signed), "durable-key receipt must verify");
        assert_eq!(
            std::fs::read_to_string(&key).unwrap().trim().len(),
            64,
            "key persists as 64 hex"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn receipts_are_hash_chained() {
        // R20: each receipt after the genesis carries prev_hash = SHA-256 of the previous LINE, so
        // whole-receipt deletion/truncation/reorder is detectable. Chained receipts still verify
        // (prev_hash is inside the signed bytes).
        let dir = std::env::temp_dir().join(format!("kriya-r20b-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.jsonl");
        let s = Signer::with_log_path(log.clone());

        let r1 = s.record(Receipt::new("s1".into(), "a".into(), json!({}), true, 1));
        let r2 = s.record(Receipt::new("s2".into(), "b".into(), json!({}), true, 2));
        let r3 = s.record(Receipt::new("s3".into(), "c".into(), json!({}), true, 3));

        assert!(
            r1.receipt.prev_hash.is_none(),
            "genesis must have no prev_hash"
        );
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            r2.receipt.prev_hash.as_deref(),
            Some(sha256_hex(lines[0].as_bytes()).as_str())
        );
        assert_eq!(
            r3.receipt.prev_hash.as_deref(),
            Some(sha256_hex(lines[1].as_bytes()).as_str())
        );
        assert!(
            verifies(&r2) && verifies(&r3),
            "chained receipts must still verify"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_signer_continues_the_chain_on_an_existing_log() {
        // Cross-restart: a second host process appending to the same log links its first receipt to
        // the last line already on disk, so resuming a deployment doesn't read as a truncation.
        let dir = std::env::temp_dir().join(format!("kriya-r20b-cont-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.jsonl");

        let s1 = Signer::with_log_path(log.clone());
        let _ = s1.record(Receipt::new("s1".into(), "a".into(), json!({}), true, 1));
        drop(s1);

        let s2 = Signer::with_log_path(log.clone()); // a fresh "process" seeds from the existing log
        let r2 = s2.record(Receipt::new("s2".into(), "b".into(), json!({}), true, 2));
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(
            r2.receipt.prev_hash.as_deref(),
            Some(sha256_hex(lines[0].as_bytes()).as_str()),
            "the continuation receipt must link to the last line of the prior run"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_key_file_is_an_error_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("kriya-r20a-bad-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("signing.key");
        std::fs::write(&key, "not-valid-hex").unwrap();
        assert!(
            Signer::with_identity(&key, dir.join("a.jsonl")).is_err(),
            "an invalid key file must error, not be silently regenerated"
        );
        assert_eq!(
            std::fs::read_to_string(&key).unwrap(),
            "not-valid-hex",
            "key left untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R27: the standard audit dir resolves to an existing, writable directory; with `$HOME` set
    /// (the normal case, incl. CI) it lands at `~/.kriya/audit/` so the Console can auto-discover it.
    #[test]
    fn default_audit_dir_is_a_writable_directory() {
        let dir = default_audit_dir();
        assert!(
            dir.is_dir(),
            "default audit dir should exist after the call: {}",
            dir.display()
        );
        if std::env::var_os("HOME").is_some() && cfg!(not(windows)) {
            assert!(
                dir.ends_with("audit") && dir.to_string_lossy().contains(".kriya"),
                "with HOME set the default dir should be ~/.kriya/audit, got {}",
                dir.display()
            );
        }
        // Prove it is actually writable (a signer must be able to append a receipt here).
        let probe = dir.join(format!("kriya-r27-probe-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&probe, b"ok").expect("default audit dir must be writable");
        let _ = std::fs::remove_file(&probe);
    }
}
