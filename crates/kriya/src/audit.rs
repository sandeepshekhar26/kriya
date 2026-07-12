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

/// Reserved `action_id` for a **retention epoch-checkpoint** receipt (doc 24 §6-P2 / EG-2). Signed
/// like any receipt and part of the chain, it seals a pruned prefix: its `params` attest
/// `{pruned_before_ts_ms, policy, prior_head_hash, pruned_count}` — "receipts before T were pruned
/// per policy P; the prior head hash was H." Verifiers accept it as a legitimate sealed chain point
/// (not a head-truncation break), so compliant deletion (GDPR erasure, retention limits) stays
/// tamper-evident instead of reading as tampering. See [`prune_and_seal`].
pub const RETENTION_CHECKPOINT: &str = "kriya.retention.checkpoint";

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
    /// The chain head (R20): hash of the last line written to `log_path` plus the log's byte length
    /// as of that observation. Seeded at construction so a new process continues the chain; behind a
    /// `Mutex` for threads sharing one signer. The length is the cheap staleness probe for
    /// CONCURRENT writers (W1-6): under the file lock in [`Signer::record`], a length mismatch means
    /// another process appended since we last looked, and the head is re-seeded from disk before
    /// chaining — so parallel hook invocations extend one chain instead of forking it.
    chain: Mutex<ChainHead>,
}

/// See [`Signer::chain`].
struct ChainHead {
    hash: Option<String>,
    len: u64,
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
        let chain = Mutex::new(seed_chain_head(&log_path));
        Self {
            key,
            public_hex,
            log_path,
            chain,
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
        let chain = Mutex::new(seed_chain_head(&log_path));
        Ok(Self {
            key,
            public_hex,
            log_path,
            chain,
        })
    }

    pub fn public_key(&self) -> &str {
        &self.public_hex
    }

    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    /// Sign a receipt and append it to the audit log. Returns the signed receipt — infallibly, for
    /// the fail-OPEN default: a receipt that could not be persisted is still returned (and the
    /// failure is swallowed), so a lost log line never takes down the caller. Callers that need to
    /// know whether the line durably hit disk — the fail-CLOSED "no receipt, no egress" mode
    /// (doc 24 B3) — use [`Signer::record_persisted`] instead.
    ///
    /// Concurrency (W1-6): the [seed tail → chain → append] window is serialized against OTHER
    /// PROCESSES by an exclusive advisory lock on the log file (Unix `flock`, auto-released on fd
    /// close — so a crashed writer never wedges the chain). Under the lock the on-disk length is
    /// compared with our last observation and the head re-seeded if someone else appended — the
    /// exact race parallel hook invocations hit (two fresh processes both seeded from the same
    /// tail would otherwise both claim the same `prev_hash`, forking the chain into what a
    /// verifier must read as tampering). Off-unix the lock is a no-op and the contract remains
    /// single-writer-per-log-at-a-time.
    pub fn record(&self, receipt: Receipt) -> SignedReceipt {
        // Fail-open: on a write failure return the signed receipt anyway (the historical contract).
        match self.record_persisted(receipt) {
            Ok(signed) => signed,
            Err(e) => e.signed,
        }
    }

    /// Sign a receipt and append it, reporting whether the **append write succeeded**. `Ok` iff the
    /// `writeln!` returned Ok (note: not `fsync`-durable — a crash before the OS flushes could still
    /// lose it); `Err(NotPersisted)` (still carrying the signed receipt) when the log was
    /// unopenable/unwritable. This is the seam for fail-closed receipt-precondition mode (doc 24 B3):
    /// the governor signs the `kriya.io.*` receipt through this before performing the egress, and a
    /// non-persisted receipt denies the egress — the receipt becomes a control, not just a record.
    // The Err intentionally carries the whole signed receipt (so the fail-OPEN wrapper can still
    // surface it), which is what makes the error type "large" — by design, not an oversight.
    #[allow(clippy::result_large_err)]
    pub fn record_persisted(&self, mut receipt: Receipt) -> Result<SignedReceipt, NotPersisted> {
        // Canonicalize before signing (R21): recursively sort `params` object keys so the signed
        // bytes never depend on a consumer's serde_json `preserve_order` feature. The offline
        // `tools/verify-receipts` applies the identical sort before re-deriving the bytes.
        receipt.params = canonical_value(&receipt.params);
        // Hash-chain (R20): link this receipt to the previous LINE so whole-receipt deletion or
        // truncation is detectable. The in-process Mutex orders threads sharing this signer; the
        // file lock below orders separate processes sharing the log.
        let mut chain = self.chain.lock().unwrap_or_else(|e| e.into_inner());
        let locked = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .ok()
            .inspect(|f| {
                lock_exclusive(f);
                // Staleness probe: a length change means another writer appended since our last
                // observation — re-seed the head from the true on-disk tail before chaining.
                let disk_len = f.metadata().map(|m| m.len()).unwrap_or(chain.len);
                if disk_len != chain.len {
                    *chain = seed_chain_head(&self.log_path);
                }
            });
        receipt.prev_hash = chain.hash.clone(); // None on the genesis receipt
                                                // Canonical bytes = compact JSON of the unsigned (key-sorted, now chained) receipt.
        let msg = serde_json::to_vec(&receipt).unwrap_or_default();
        let signature = hex::encode(self.key.sign(&msg).to_bytes());
        let signed = SignedReceipt {
            receipt,
            public_key: self.public_hex.clone(),
            signature,
        };
        let line = serde_json::to_string(&signed).unwrap_or_default();
        let persisted = if let Some(mut f) = locked {
            if writeln!(f, "{line}").is_ok() {
                // The exact line just written becomes the next receipt's `prev_hash`; advance the
                // observed length by what we appended (we hold the lock, nothing interleaves).
                chain.hash = Some(sha256_hex(line.as_bytes()));
                chain.len += line.len() as u64 + 1;
                true
            } else {
                false
            }
            // Lock released when `f` drops.
        } else {
            // Unopenable: the head deliberately does NOT advance — advancing it would make the next
            // successful receipt chain to a line that never hit disk (a self-inflicted verifier
            // break on top of a lost receipt).
            false
        };
        if persisted {
            Ok(signed)
        } else {
            Err(NotPersisted {
                reason: format!("audit log {} could not be written", self.log_path.display()),
                signed,
            })
        }
    }

    /// Sign a receipt **without appending** — for callers that assemble their own file (the
    /// retention [`prune_and_seal`] re-chains retained receipts onto a checkpoint). `prev_hash` is
    /// taken from the receipt as passed (the caller sets the chain link); `params` are canonicalized
    /// exactly as [`Signer::record`] does, so a re-signed line is byte-identical to a natively
    /// recorded one.
    pub fn sign_only(&self, mut receipt: Receipt) -> SignedReceipt {
        receipt.params = canonical_value(&receipt.params);
        let msg = serde_json::to_vec(&receipt).unwrap_or_default();
        let signature = hex::encode(self.key.sign(&msg).to_bytes());
        SignedReceipt {
            receipt,
            public_key: self.public_hex.clone(),
            signature,
        }
    }
}

/// A receipt was signed but could not be durably written to the audit log. Carries the signed
/// receipt (so a fail-open caller can still surface it) plus a human-readable reason.
#[derive(Debug, Clone)]
pub struct NotPersisted {
    pub signed: SignedReceipt,
    pub reason: String,
}

/// The result of a retention prune (doc 24 §6-P2).
#[derive(Debug, Clone)]
pub struct PruneReport {
    /// How many receipts were pruned (the sealed prefix).
    pub pruned: usize,
    /// How many survived and were re-chained onto the checkpoint.
    pub retained: usize,
    /// The hash of the last pruned line — the "prior head hash H" the checkpoint attests.
    pub prior_head_hash: Option<String>,
    /// The checkpoint receipt's `step_id`, when one was written.
    pub checkpoint_step_id: Option<String>,
}

/// Prune every receipt older than `cutoff_ts_ms` from `log_path` and seal the pruned prefix behind
/// a signed [`RETENTION_CHECKPOINT`] receipt — compliant deletion that stays verifiable (doc 24
/// §6-P2). Without this, deleting old receipts is indistinguishable from tampering, because it
/// breaks the hash chain; the checkpoint records "receipts before T pruned per policy P; prior head
/// hash H" so a verifier accepts the seal instead of flagging a truncation.
///
/// The pruned set is the leading, time-ordered run of receipts with `ts_ms < cutoff_ts_ms` (the log
/// is append-ordered, so this is a clean "everything before T" epoch). Survivors are **re-chained
/// onto the checkpoint** (re-signed by `signer`) so the chain is unbroken from the checkpoint
/// forward. `signer`'s key MUST match every retained receipt's `public_key`: a prune never silently
/// re-attributes a receipt to a different signer — a mismatch is a hard error and the log is left
/// untouched. A no-op (nothing older than the cutoff) writes nothing.
pub fn prune_and_seal(
    log_path: &Path,
    cutoff_ts_ms: u128,
    policy_label: &str,
    signer: &Signer,
) -> Result<PruneReport, String> {
    let content = std::fs::read_to_string(log_path)
        .map_err(|e| format!("reading {}: {e}", log_path.display()))?;
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();

    // The log is append-ordered by time, so the pruned set is the leading run of lines older than
    // the cutoff; stop at the first line at/after the cutoff (or unparseable — treat as retained).
    let split = lines
        .iter()
        .position(|l| line_ts(l).map(|ts| ts >= cutoff_ts_ms).unwrap_or(true))
        .unwrap_or(lines.len());
    if split == 0 {
        return Ok(PruneReport {
            pruned: 0,
            retained: lines.len(),
            prior_head_hash: None,
            checkpoint_step_id: None,
        });
    }
    let (pruned, retained) = lines.split_at(split);
    let prior_head_hash = sha256_hex(pruned[pruned.len() - 1].as_bytes());

    // The checkpoint receipt, sealed to the prior head H and attesting the prune.
    let step_id = uuid::Uuid::new_v4().to_string();
    let mut checkpoint = Receipt::new(
        step_id.clone(),
        RETENTION_CHECKPOINT.to_string(),
        serde_json::json!({
            "pruned_before_ts_ms": cutoff_ts_ms as u64,
            "policy": policy_label,
            "prior_head_hash": prior_head_hash,
            "pruned_count": pruned.len(),
        }),
        true,
        now_ms(),
    );
    checkpoint.prev_hash = Some(prior_head_hash.clone());
    let checkpoint_signed = signer.sign_only(checkpoint);
    let checkpoint_line = serde_json::to_string(&checkpoint_signed)
        .map_err(|e| format!("serializing checkpoint: {e}"))?;

    // Re-chain the survivors onto the checkpoint (re-signed by this signer; same key required).
    let mut out_lines = vec![checkpoint_line.clone()];
    let mut prev = sha256_hex(checkpoint_line.as_bytes());
    for l in retained {
        let (receipt, public_key) =
            parse_stored_receipt(l).map_err(|e| format!("re-chaining retained receipt: {e}"))?;
        if public_key != signer.public_key() {
            return Err(format!(
                "retained receipt {} was signed by a different key — refusing to re-attribute it to the pruning signer",
                receipt.step_id
            ));
        }
        let mut r = receipt;
        r.prev_hash = Some(prev.clone());
        let signed = signer.sign_only(r);
        let line = serde_json::to_string(&signed)
            .map_err(|e| format!("serializing retained receipt: {e}"))?;
        prev = sha256_hex(line.as_bytes());
        out_lines.push(line);
    }

    // Rewrite via a temp file + rename so a crash mid-prune never leaves a half-written log.
    let tmp = log_path.with_extension("jsonl.prune-tmp");
    let body = out_lines.join("\n") + "\n";
    std::fs::write(&tmp, body).map_err(|e| format!("writing pruned log: {e}"))?;
    std::fs::rename(&tmp, log_path).map_err(|e| format!("replacing log: {e}"))?;

    // Re-seed the signer's in-memory chain head from the rewritten log so the NEXT `record()` chains
    // onto the last RETAINED line — never onto the pruned (now-gone) head. record()'s length-probe
    // would usually catch the change, but the pruned-vs-re-chained lengths can coincidentally match,
    // so reset explicitly rather than rely on the heuristic.
    if let Ok(mut chain) = signer.chain.lock() {
        *chain = seed_chain_head(log_path);
    }

    Ok(PruneReport {
        pruned: pruned.len(),
        retained: retained.len(),
        prior_head_hash: Some(prior_head_hash),
        checkpoint_step_id: Some(step_id),
    })
}

/// The `ts_ms` of a stored receipt line (for the retention cutoff split). `None` if unparseable.
fn line_ts(line: &str) -> Option<u128> {
    serde_json::from_str::<Value>(line)
        .ok()?
        .get("ts_ms")
        .and_then(Value::as_u64)
        .map(|t| t as u128)
}

/// Reconstruct the unsigned [`Receipt`] + `public_key` from a stored line. `Receipt` is
/// Serialize-only (its declaration order is the load-bearing signed order), so we read the wire
/// JSON by hand rather than deriving Deserialize on the frozen schema.
fn parse_stored_receipt(line: &str) -> Result<(Receipt, String), String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("parse: {e}"))?;
    let step_id = v
        .get("step_id")
        .and_then(Value::as_str)
        .ok_or("no step_id")?
        .to_string();
    let action_id = v
        .get("action_id")
        .and_then(Value::as_str)
        .ok_or("no action_id")?
        .to_string();
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let success = v
        .get("success")
        .and_then(Value::as_bool)
        .ok_or("no success")?;
    let ts_ms = v.get("ts_ms").and_then(Value::as_u64).ok_or("no ts_ms")? as u128;
    let actor = match v.get("actor") {
        Some(a) if a.is_object() => Some(Actor {
            agent: a
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            user: a
                .get("user")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        _ => None,
    };
    let public_key = v
        .get("public_key")
        .and_then(Value::as_str)
        .ok_or("no public_key")?
        .to_string();
    let mut r = Receipt::new(step_id, action_id, params, success, ts_ms);
    r.actor = actor;
    Ok((r, public_key))
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

/// Seed the chain head from an existing log: hash of the last non-empty line (so a new process
/// CONTINUES the chain instead of starting a fresh one a verifier would read as head-truncation)
/// plus the log's byte length (the staleness probe [`Signer::record`] uses under the file lock).
/// `hash: None` + `len: 0` for an absent/empty log — a genuine genesis.
fn seed_chain_head(log_path: &Path) -> ChainHead {
    match std::fs::read_to_string(log_path) {
        Ok(content) => ChainHead {
            hash: content
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| sha256_hex(l.as_bytes())),
            len: content.len() as u64,
        },
        Err(_) => ChainHead { hash: None, len: 0 },
    }
}

/// Exclusive advisory lock on the audit log (blocking). Unix `flock`: released automatically when
/// the fd closes — including on process death, so a crashed hook invocation can never wedge the
/// chain. Best-effort by design: on failure (exotic filesystems) behavior degrades to the previous
/// unlocked append rather than dropping the receipt. Off-unix this is a no-op and the log's
/// contract stays "one writer at a time".
#[cfg(unix)]
fn lock_exclusive(f: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    // SAFETY: flock on a valid, open fd; no memory is passed. Advisory only.
    unsafe {
        let _ = libc::flock(f.as_raw_fd(), libc::LOCK_EX);
    }
}
#[cfg(not(unix))]
fn lock_exclusive(_f: &std::fs::File) {}

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

    /// W1-6: many CONCURRENT signer instances over one log + one persisted key — the parallel
    /// hook-invocation model (each instance owns its fd, so the flock path is exercised exactly as
    /// it is between processes). The chain must come out fork-free: every line's `prev_hash` equals
    /// the hash of the exact line before it, every line parses (no torn writes), every signature
    /// verifies, and nothing is lost. Before the record()-time lock + re-seed, two writers seeded
    /// from the same tail would both claim the same parent — a fork a verifier must flag.
    #[test]
    #[cfg(unix)]
    fn concurrent_signers_extend_one_chain_without_forking() {
        let dir = std::env::temp_dir().join(format!("kriya-flock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("hook.key");
        let log = dir.join("claude-code.jsonl");

        // Mint the shared identity first (parallel first-ever key creation is out of scope here).
        drop(Signer::with_identity(&key, log.clone()).unwrap());

        let n_threads = 4;
        let per_thread = 25;
        let handles: Vec<_> = (0..n_threads)
            .map(|t| {
                let key = key.clone();
                let log = log.clone();
                std::thread::spawn(move || {
                    // A FRESH Signer per thread — like a fresh process, it seeds its chain head
                    // once (possibly mid-hammering) and must reconcile under the lock thereafter.
                    let s = Signer::with_identity(&key, log).unwrap();
                    for i in 0..per_thread {
                        let signed = s.record(Receipt::new(
                            format!("t{t}-s{i}"),
                            "claude-code__bash".into(),
                            json!({ "thread": t, "seq": i }),
                            true,
                            now_ms(),
                        ));
                        assert!(verifies(&signed), "receipt t{t}-s{i} must verify");
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            n_threads * per_thread,
            "no receipt lost, no line torn"
        );
        let mut prev: Option<String> = None;
        for (i, line) in lines.iter().enumerate() {
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("line {} is torn/unparseable: {e}", i + 1));
            let declared = v
                .get("prev_hash")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            assert_eq!(
                declared,
                prev,
                "chain fork at line {} — a receipt claims a stale parent",
                i + 1
            );
            prev = Some(sha256_hex(line.as_bytes()));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─── EG-2: fail-closed record + retention checkpoint (doc 24 §6-P2, B3) ──────────────────────

    #[test]
    fn record_persisted_reports_write_success_and_failure() {
        // A writable log: record_persisted succeeds and the line hits disk.
        let s = signer();
        let ok = s.record_persisted(Receipt::new("s".into(), "a".into(), json!({}), true, 1));
        assert!(ok.is_ok(), "a writable log must persist");
        assert!(verifies(&ok.unwrap()));

        // An UNWRITABLE log (the path is a directory) — the append can never open it. Fail-closed
        // mode reads this Err as "no receipt" and denies the egress (doc 24 B3).
        let dir = std::env::temp_dir().join(format!("kriya-b3-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let blocked = Signer::with_log_path(dir.clone()); // log_path IS a directory
        let err =
            blocked.record_persisted(Receipt::new("s".into(), "a".into(), json!({}), true, 1));
        assert!(
            err.is_err(),
            "an unwritable log must be reported, not swallowed"
        );
        // The infallible wrapper still returns the signed receipt (fail-open default).
        let signed = blocked.record(Receipt::new("s".into(), "a".into(), json!({}), true, 1));
        assert!(!signed.signature.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_and_seal_produces_a_verifiable_sealed_chain() {
        // A durable key so the retained receipts can be re-chained onto the checkpoint by the SAME
        // signer (a prune never re-attributes across keys).
        let dir = std::env::temp_dir().join(format!("kriya-retention-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("s.key");
        let log = dir.join("audit.jsonl");
        let s = Signer::with_identity(&key, log.clone()).unwrap();

        // Four receipts at t = 100, 200, 300, 400.
        for (i, ts) in [100u128, 200, 300, 400].into_iter().enumerate() {
            s.record(Receipt::new(
                format!("s{i}"),
                "kriya.io.egress.mcp.allow".into(),
                json!({"seq": i}),
                true,
                ts,
            ));
        }
        let pruned_head = {
            let lines: Vec<String> = std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect();
            sha256_hex(lines[1].as_bytes()) // the ts=200 line is the last pruned
        };

        // Prune everything before t = 300 (seals the first two).
        let report = prune_and_seal(&log, 300, "io-30d", &s).expect("prune");
        assert_eq!(report.pruned, 2);
        assert_eq!(report.retained, 2);
        assert_eq!(
            report.prior_head_hash.as_deref(),
            Some(pruned_head.as_str())
        );

        // The sealed log: checkpoint first (sealing to H), then the two survivors re-chained onto it.
        let lines: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        assert_eq!(lines.len(), 3, "checkpoint + 2 retained");

        let cp: SignedReceipt = {
            // Parse the checkpoint via the verify helper's shape by round-tripping the fields.
            let v: Value = serde_json::from_str(&lines[0]).unwrap();
            assert_eq!(v["action_id"], RETENTION_CHECKPOINT);
            assert_eq!(
                v["prev_hash"],
                Value::String(pruned_head.clone()),
                "checkpoint seals to prior head H"
            );
            assert_eq!(
                v["params"]["prior_head_hash"],
                Value::String(pruned_head.clone())
            );
            assert_eq!(v["params"]["pruned_count"], json!(2));
            reparse_signed(&lines[0])
        };
        assert!(verifies(&cp), "the checkpoint itself must verify");

        // Every line verifies, and the chain is contiguous FROM the checkpoint forward.
        let mut prev = sha256_hex(lines[0].as_bytes());
        for l in &lines[1..] {
            let signed = reparse_signed(l);
            assert!(verifies(&signed), "re-chained retained receipt must verify");
            let v: Value = serde_json::from_str(l).unwrap();
            assert_eq!(
                v["prev_hash"],
                Value::String(prev.clone()),
                "retained receipt chains onto the checkpoint"
            );
            prev = sha256_hex(l.as_bytes());
        }

        // A second prune with nothing older is a clean no-op.
        let noop = prune_and_seal(&log, 50, "io-30d", &s).expect("noop prune");
        assert_eq!(noop.pruned, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Re-hydrate a stored line into the local `SignedReceipt` shape so the test's `verifies` helper
    /// can re-derive its canonical bytes. Mirrors what `parse_stored_receipt` does internally.
    fn reparse_signed(line: &str) -> SignedReceipt {
        let v: Value = serde_json::from_str(line).unwrap();
        let actor = v.get("actor").and_then(|a| {
            Some(Actor::new(
                a.get("agent")?.as_str()?,
                a.get("user")?.as_str()?,
            ))
        });
        let mut r = Receipt::new(
            v["step_id"].as_str().unwrap().to_string(),
            v["action_id"].as_str().unwrap().to_string(),
            v["params"].clone(),
            v["success"].as_bool().unwrap(),
            v["ts_ms"].as_u64().unwrap() as u128,
        );
        r.actor = actor;
        r.prev_hash = v
            .get("prev_hash")
            .and_then(Value::as_str)
            .map(str::to_string);
        SignedReceipt {
            receipt: r,
            public_key: v["public_key"].as_str().unwrap().to_string(),
            signature: v["signature"].as_str().unwrap().to_string(),
        }
    }
}
