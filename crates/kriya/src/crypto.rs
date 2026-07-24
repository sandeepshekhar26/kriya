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

// ─── A5 (doc 27, PQ dual-signature receipts — `docs/design/a5-pq-dual-sig.md`) ───────────────
//
// A SEPARATE, independent opt-in feature `pq-crypto` extends this facade with an ML-DSA-87
// (FIPS 204) countersignature type, parallel to (not nested in) the `fips-crypto` lane above
// (design D4). ML-DSA-87 is **post-quantum-ready, NOT FIPS-validated** — it is outside CMVP cert
// #5298's approved boundary (ML-KEM is in it, ML-DSA is not — design D6). Never claim
// "FIPS-validated PQ" / "quantum-proof" anywhere this facade's output is surfaced.
//
// **Documented deviation from the design (D4 / RT2.5 "compose" claim).** The design states
// `pq-crypto` and `fips-crypto` "compose" (both may be enabled together) and calls for a CI
// matrix cell exercising both. Verified against the real, pinned `aws-lc-rs` 1.17.x source
// (`src/lib.rs`): `#[cfg(all(feature = "unstable", not(feature = "fips")))] pub mod unstable;` —
// the `unstable` module, which is where `PqdsaKeyPair`/`ML_DSA_87` live, is compiled OUT of
// `aws-lc-rs` whenever ITS `fips` feature is active. So `pq-crypto` and `fips-crypto` are
// mutually exclusive at the dependency level, not composable, in this aws-lc-rs release line.
// This is a narrow, mechanical correction (the D4 "compose" sentence and the RT2.5/§8 CI
// matrix-cell plan) — it does not touch D1-D3/D5-D9, the §4 schema, §5 verification matrix, §6
// honesty wording, or the §7 red-team findings, none of which depend on simultaneous fips+pq. The
// `compile_error!` below turns an accidental double-enable into a build failure with this
// explanation rather than a silent `unstable`-module-not-found compile error deep in `audit.rs`.
#[cfg(all(feature = "pq-crypto", feature = "fips-crypto"))]
compile_error!(
    "pq-crypto and fips-crypto cannot be enabled together in this build: aws-lc-rs's `unstable` \
     module (PqdsaKeyPair / ML-DSA, what pq-crypto needs) is compiled out whenever aws-lc-rs's \
     own `fips` feature is active (aws-lc-rs 1.17.x: `#[cfg(all(feature = \"unstable\", \
     not(feature = \"fips\")))] pub mod unstable;`). This is a real upstream constraint, not a \
     kriya restriction — see the deviation note in crates/kriya/src/crypto.rs and \
     docs/design/a5-pq-dual-sig.md (kriya-console repo) D4/RT2.5."
);

/// The PQ lane's status — the facts a `kriya.crypto.pq_key`/`pq_checkpoint` receipt and the
/// Console Settings row read (design §6). Always constructible (both lanes), unlike
/// [`PqSigningKey`] which only exists when `pq-crypto` is compiled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PqStatus {
    /// `true` iff this build was compiled with `pq-crypto`. There is no runtime on/off switch
    /// for the algorithm itself (unlike the FIPS lane's live `try_fips_mode()` probe) — ML-DSA
    /// has no FIPS-validated mode to probe (design §10 honest ceiling).
    pub enabled: bool,
    /// `Some("ML-DSA-87")` when enabled, else `None`.
    pub alg: Option<&'static str>,
}

#[cfg(feature = "pq-crypto")]
pub fn pq_active() -> PqStatus {
    PqStatus {
        enabled: true,
        alg: Some("ML-DSA-87"),
    }
}
#[cfg(not(feature = "pq-crypto"))]
pub fn pq_active() -> PqStatus {
    PqStatus {
        enabled: false,
        alg: None,
    }
}

#[cfg(feature = "pq-crypto")]
pub use pq::{pq_verify, PqError, PqSigningKey};

#[cfg(feature = "pq-crypto")]
mod pq {
    use aws_lc_rs::rand as lc_rand;
    use aws_lc_rs::signature::{KeyPair as _, UnparsedPublicKey};
    use aws_lc_rs::unstable::signature::{PqdsaKeyPair, ML_DSA_87, ML_DSA_87_SIGNING};

    /// An ML-DSA-87 (FIPS 204) keypair — post-quantum-ready, **not** FIPS-validated (design D6).
    /// Unlike [`super::SigningKey`] (Ed25519, RFC 8032 deterministic), ML-DSA-87 signing is
    /// **randomized** (FIPS 204 hedged signing): two signatures over the same message with the
    /// same key differ. Callers must not assert byte-identity across signs — only that a
    /// signature verifies (design D4/RT2.4). The keypair itself IS seed-deterministic: the same
    /// 32-byte seed always derives the same keypair (design D3), which is what
    /// [`PqSigningKey::from_seed`] relies on for persisted-identity reload.
    pub struct PqSigningKey(PqdsaKeyPair);

    /// A PQ (ML-DSA-87) facade error — seed rejected, sign/verify round-trip failure, etc.
    #[derive(Debug)]
    pub struct PqError(pub String);

    impl std::fmt::Display for PqError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "PQ (ML-DSA-87) error: {}", self.0)
        }
    }
    impl std::error::Error for PqError {}

    impl PqSigningKey {
        /// Load a keypair from a 32-byte seed (FIPS 204 seed-deterministic — design D3). Mirrors
        /// the Ed25519 [`super::SigningKey::from_seed`] discipline: the on-disk secret is always
        /// the 32-byte seed, never the expanded private key.
        pub fn from_seed(seed: &[u8; 32]) -> Result<Self, PqError> {
            PqdsaKeyPair::from_seed(&ML_DSA_87_SIGNING, seed)
                .map(PqSigningKey)
                .map_err(|e| PqError(format!("from_seed: {e:?}")))
        }

        /// Mint a fresh keypair: the seed is sourced from the active lane's DRBG
        /// (`aws_lc_rs::rand::fill` — the same RNG A4's FIPS lane uses for Ed25519 keygen), then
        /// expanded deterministically into the ML-DSA-87 keypair (design D3). Returns the raw
        /// seed alongside the key so callers persist it at `~/.kriya/pq-signing.seed` (0600)
        /// exactly as the Ed25519 identity is persisted at `signing.key`.
        pub fn generate() -> ([u8; 32], Self) {
            let mut seed = [0u8; 32];
            lc_rand::fill(&mut seed).expect("aws-lc-rs DRBG fill");
            let key =
                Self::from_seed(&seed).expect("a freshly-filled 32-byte seed is always valid");
            (seed, key)
        }

        /// The raw ML-DSA-87 public key — 2592 bytes (design §2 D1 size table; 5184 hex chars on
        /// the wire). Raw octets, not DER/PKCS#8 — the wire format design D2's `pq_public_key`
        /// sibling uses.
        pub fn public_key(&self) -> Vec<u8> {
            self.0.public_key().as_ref().to_vec()
        }

        /// Sign `msg` with ML-DSA-87 — 4627 bytes (design §2 D1). **Randomized**: repeated calls
        /// over the same message produce different signature bytes (FIPS 204 hedged signing);
        /// every one of them verifies. Do not assert byte-identity in tests (design D4/RT2.4) —
        /// assert `pq_verify` instead.
        pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
            let mut sig = vec![0u8; ML_DSA_87_SIGNING.signature_len()];
            let n = self
                .0
                .sign(msg, &mut sig)
                .expect("ML-DSA-87 sign (fixed-size output buffer, cannot fail on a valid key)");
            sig.truncate(n);
            sig
        }
    }

    /// Verify a detached ML-DSA-87 signature. `pubkey` is the raw 2592-byte octet form
    /// [`PqSigningKey::public_key`] returns (never DER/PKCS#8 on the wire — design D2).
    pub fn pq_verify(pubkey: &[u8], msg: &[u8], sig: &[u8]) -> bool {
        UnparsedPublicKey::new(&ML_DSA_87, pubkey)
            .verify(msg, sig)
            .is_ok()
    }
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

    #[test]
    fn pq_active_reports_something_sane() {
        let s = pq_active();
        assert_eq!(s.enabled, cfg!(feature = "pq-crypto"));
        assert_eq!(s.alg.is_some(), cfg!(feature = "pq-crypto"));
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn pq_same_seed_same_pubkey_verify_roundtrip() {
        let seed = [7u8; 32];
        let key = PqSigningKey::from_seed(&seed).unwrap();
        let pk1 = key.public_key();
        let key2 = PqSigningKey::from_seed(&seed).unwrap();
        let pk2 = key2.public_key();
        // Seed-deterministic keypair (FIPS 204): same seed -> same public key, even though
        // signing itself is randomized (design D4/RT2.4).
        assert_eq!(pk1, pk2);
        assert_eq!(pk1.len(), 2592);

        let sig = key.sign(b"hello pq kriya");
        assert_eq!(sig.len(), 4627);
        assert!(pq_verify(&pk1, b"hello pq kriya", &sig));
        assert!(!pq_verify(&pk1, b"tampered", &sig));
    }

    #[cfg(feature = "pq-crypto")]
    #[test]
    fn pq_signing_is_randomized_but_both_verify() {
        let seed = [9u8; 32];
        let key = PqSigningKey::from_seed(&seed).unwrap();
        let pk = key.public_key();
        let sig_a = key.sign(b"same message");
        let sig_b = key.sign(b"same message");
        // ML-DSA-87 hedged/randomized signing (design D4/RT2.4): NOT byte-identical across calls.
        assert_ne!(sig_a, sig_b);
        assert!(pq_verify(&pk, b"same message", &sig_a));
        assert!(pq_verify(&pk, b"same message", &sig_b));
    }

    // A5 (design D7): the two cross-implementation ML-DSA-87 parity tests
    // (`pq_cross_impl_aws_lc_signs_rustcrypto_verifies` and its reverse) that pit the production
    // `aws-lc-rs` signer against RustCrypto's independent `ml-dsa` implementation live in the
    // standalone sibling crate `crates/kriya-pq-parity` (integration tests). They were moved out
    // of here so `crates/kriya` carries NO `[dev-dependencies]` — a package with any dev-deps
    // cannot be `cargo test -p`'d from the `apps/note-app/src-tauri` workspace, where `kriya` is a
    // non-member path dependency. CI runs the sibling crate in the `pq-crypto` job.
}
