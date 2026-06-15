//! Signed audit trail. The host holds an Ed25519 key the agent never sees and signs a
//! receipt for every executed action. Receipts are appended to a JSONL log and can be
//! verified offline by anyone holding the public key.

use ed25519_dalek::{Signer as _, SigningKey};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub step_id: String,
    pub action_id: String,
    pub params: Value,
    pub success: bool,
    pub ts_ms: u128,
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
        let bytes: [u8; 32] = rand::random();
        let key = SigningKey::from_bytes(&bytes);
        let public_hex = hex::encode(key.verifying_key().to_bytes());
        let log_path = std::env::temp_dir().join("kriya-audit.jsonl");
        Self { key, public_hex, log_path }
    }

    pub fn public_key(&self) -> &str {
        &self.public_hex
    }

    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }

    /// Sign a receipt and append it to the audit log. Returns the signed receipt.
    pub fn record(&self, receipt: Receipt) -> SignedReceipt {
        // Canonical bytes = JSON of the unsigned receipt.
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
