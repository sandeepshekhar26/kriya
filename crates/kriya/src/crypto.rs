//! The A4 FIPS-lane crypto facade (doc 27 A4 / `docs/design/a4-fips-lane.md`, D1).
//!
//! Every production Ed25519 sign/verify/keygen site in this crate routes through here instead of
//! naming `ed25519_dalek::*` directly. The public shape is **feature-selected but not
//! feature-visible**: callers see the same [`SigningKey`]/[`verify`]/[`active_module`] surface
//! whether the default `ed25519-dalek` lane or the opt-in `fips-crypto` (`aws-lc-rs` + its `fips`
//! feature, backed by `aws-lc-fips-sys`, AWS-LC-FIPS 3.x, CMVP cert #5298) lane is compiled in.
//!
//! **Load-bearing fact (design §1):** Ed25519 is deterministic (RFC 8032) — the 64 signature bytes
//! produced over a given message by a given 32-byte seed are identical under either backend. That is
//! what makes a FIPS-signed receipt verify under a default-build verifier and vice versa (design
//! acceptance #2); it also means a signature can never itself prove which module produced it. The
//! [`CryptoModule`] record returned by [`active_module`] is a **host self-attestation** of
//! build/runtime configuration, not a cryptographic proof — see `kriya.crypto.module` in `audit.rs`
//! and `docs/samples/fips-module-boundary.md` in the Console repo.
//!
//! This crate's copy of the facade is signature-identical to the Console's
//! `kriya_verify::crypto` (design D1) so the two collapse into one module without call-site churn
//! when `kriya-verify` is eventually published as the shared open crate.

#[cfg(not(feature = "fips-crypto"))]
mod backend {
    //! Default lane — `ed25519-dalek` 2. Behavior is byte-identical to the pre-A4 code paths this
    //! replaces; this module only relocates the calls behind the facade.
    use ed25519_dalek::{Signer as _, SigningKey as DalekKey, Signature, Verifier as _, VerifyingKey};

    use super::CryptoModule;

    pub struct SigningKeyImpl(DalekKey);

    impl SigningKeyImpl {
        pub fn from_seed(seed: &[u8; 32]) -> Self {
            SigningKeyImpl(DalekKey::from_bytes(seed))
        }

        pub fn generate() -> ([u8; 32], Self) {
            let seed: [u8; 32] = rand::random();
            let key = Self::from_seed(&seed);
            (seed, key)
        }

        pub fn public_key(&self) -> [u8; 32] {
            self.0.verifying_key().to_bytes()
        }

        pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
            self.0.sign(msg).to_bytes()
        }
    }

    pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        let Ok(vk) = VerifyingKey::from_bytes(pubkey) else {
            return false;
        };
        vk.verify(msg, &Signature::from_bytes(sig)).is_ok()
    }

    pub fn active_module() -> CryptoModule {
        CryptoModule {
            backend: "ed25519-dalek",
            fips_module: None,
            cmvp_cert: None,
            fips_mode_active: false,
            operational_environment: "not-fips",
        }
    }
}

#[cfg(feature = "fips-crypto")]
mod backend {
    //! FIPS lane (opt-in, off by default) — `aws-lc-rs` with its `fips` feature
    //! (`aws-lc-fips-sys`, AWS-LC-FIPS 3.x, CMVP cert #5298). Ed25519 is in the 3.x approved
    //! boundary (design doc §2 V3 fact-check).
    use aws_lc_rs::rand as lc_rand;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _, UnparsedPublicKey, ED25519};

    use super::CryptoModule;

    pub struct SigningKeyImpl(Ed25519KeyPair);

    impl SigningKeyImpl {
        pub fn from_seed(seed: &[u8; 32]) -> Self {
            // Any 32-byte value is a valid RFC-8032 Ed25519 seed, so this cannot fail in practice —
            // matching the default lane's infallible `from_bytes`.
            let kp = Ed25519KeyPair::from_seed_unchecked(seed)
                .expect("a 32-byte seed is always a valid Ed25519 seed");
            SigningKeyImpl(kp)
        }

        /// The **only** keygen entry point under the FIPS lane: the seed is sourced from the
        /// module's approved DRBG (`aws_lc_rs::rand::fill`), so a FIPS-signed receipt is also
        /// signed under a key minted in the module boundary (design D5-item-3 / RT2.3).
        pub fn generate() -> ([u8; 32], Self) {
            let mut seed = [0u8; 32];
            lc_rand::fill(&mut seed).expect("aws-lc-rs module DRBG fill");
            let key = Self::from_seed(&seed);
            (seed, key)
        }

        pub fn public_key(&self) -> [u8; 32] {
            let pk = self.0.public_key();
            let bytes = pk.as_ref();
            let mut out = [0u8; 32];
            out.copy_from_slice(bytes);
            out
        }

        pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
            let sig = self.0.sign(msg);
            let bytes = sig.as_ref();
            let mut out = [0u8; 64];
            out.copy_from_slice(bytes);
            out
        }
    }

    pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        UnparsedPublicKey::new(&ED25519, &pubkey[..])
            .verify(msg, &sig[..])
            .is_ok()
    }

    pub fn active_module() -> CryptoModule {
        // Runtime check, not a build flag (design §4: `fips_mode_active` is "live try_fips_mode()
        // result at startup"). This is the build/test gate design D5-item-4 also asserts.
        let fips_mode_active = aws_lc_rs::try_fips_mode().is_ok();
        CryptoModule {
            backend: "aws-lc-rs",
            fips_module: Some("AWS-LC-FIPS 3.x"),
            cmvp_cert: Some("5298"),
            fips_mode_active,
            operational_environment: detect_operational_environment(),
        }
    }

    /// The per-OS honesty matrix (design D3), refined to the operational-environment granularity
    /// (design §4): cert #5298's *tested* OE is Amazon Linux 2023 only; other Linux distros and
    /// macOS run the identical validated module code outside that tested OE. Never upgraded for
    /// macOS; never claims `validated-oe` without positive AL2023 detection.
    fn detect_operational_environment() -> &'static str {
        if cfg!(target_os = "linux") {
            if is_amazon_linux_2023() {
                "validated-oe"
            } else {
                "validated-module-untested-oe"
            }
        } else if cfg!(target_os = "macos") {
            "outside-cmvp-oe"
        } else {
            // No platform outside Linux/macOS is a build target for the fips-crypto feature
            // (design D4: Windows FIPS is explicitly out of scope), but a live check that never
            // over-claims is cheaper than a compile-time platform ban.
            "validated-module-untested-oe"
        }
    }

    #[cfg(target_os = "linux")]
    fn is_amazon_linux_2023() -> bool {
        std::fs::read_to_string("/etc/os-release")
            .map(|s| s.contains("Amazon Linux") && s.contains("2023"))
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "linux"))]
    fn is_amazon_linux_2023() -> bool {
        false
    }
}

/// The signing keypair — `ed25519_dalek::SigningKey` under the default lane, an
/// `aws_lc_rs::signature::Ed25519KeyPair` under `fips-crypto`. Same RFC-8032 seed ⇒ same public key
/// ⇒ same signatures on both lanes (design §1, §3).
pub struct SigningKey(backend::SigningKeyImpl);

impl SigningKey {
    /// Load a keypair from an RFC-8032 32-byte seed. Identical keypair on both lanes.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        SigningKey(backend::SigningKeyImpl::from_seed(seed))
    }

    /// Mint a fresh keypair. The **only** keygen entry point (design §3 migration note) — under
    /// `fips-crypto` the seed comes from the module's approved DRBG; under the default lane it
    /// keeps the existing `rand`-crate source. Returns the raw seed alongside the key so callers
    /// that persist identity (R20) can write it to disk exactly as before.
    pub fn generate() -> ([u8; 32], Self) {
        let (seed, inner) = backend::SigningKeyImpl::generate();
        (seed, SigningKey(inner))
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.0.public_key()
    }

    /// Deterministic — no RNG at sign time (RFC 8032). Identical bytes across lanes for the same
    /// seed + message.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.0.sign(msg)
    }
}

/// Verify a detached Ed25519 signature. Identical result across lanes for honest inputs (design §6
/// RT2.5 notes a documented, bounded difference only on adversarial edge-case inputs, never on
/// honest receipts or the 1-byte-tamper case).
pub fn verify(pubkey: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    backend::verify(pubkey, msg, sig)
}

/// The running lane's identity — the facts a [`kriya.crypto.module`](crate::audit::ATTESTATION_CRYPTO_MODULE)
/// attestation receipt records. A **host self-attestation**, not a cryptographic property of any
/// signature it neighbors (design §1, §4): trustworthy exactly to the extent the host is trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoModule {
    /// Crypto backend crate: `"ed25519-dalek"` or `"aws-lc-rs"`.
    pub backend: &'static str,
    /// Validated module name when the FIPS lane is compiled in and active, else `None`.
    pub fips_module: Option<&'static str>,
    /// CMVP certificate — a shared class id (identical across every deployment on the lane), never
    /// a per-instance serial (design RT2.2).
    pub cmvp_cert: Option<&'static str>,
    /// Live `try_fips_mode()` result at startup — a runtime check, not a build flag.
    pub fips_mode_active: bool,
    /// `"validated-oe"` | `"validated-module-untested-oe"` | `"outside-cmvp-oe"` | `"not-fips"`
    /// (design §4 / D3).
    pub operational_environment: &'static str,
}

/// The running lane's identity, computed fresh on every call (cheap; no caching, so a build that
/// somehow toggles FIPS mode mid-run is still reported honestly).
pub fn active_module() -> CryptoModule {
    backend::active_module()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_pubkey_and_signature() {
        let seed = [42u8; 32];
        let key = SigningKey::from_seed(&seed);
        let pk = key.public_key();
        let sig = key.sign(b"hello kriya");
        assert!(verify(&pk, b"hello kriya", &sig));
        assert!(!verify(&pk, b"tampered", &sig));
    }

    #[test]
    fn active_module_reports_something_sane() {
        let m = active_module();
        assert!(!m.backend.is_empty());
    }
}
