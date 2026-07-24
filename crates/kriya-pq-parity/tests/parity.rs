//! A5 (design D7): cross-implementation ML-DSA-87 parity — the production `aws-lc-rs`
//! signer/verifier (via `kriya::crypto`, `pq-crypto` feature) checked against a SECOND,
//! INDEPENDENT implementation (RustCrypto's `ml-dsa`, test-only, unaudited — V4). Two
//! independent ML-DSA-87 implementations agreeing is the strongest cross-impl assurance
//! available without a JS build of aws-lc (D7's rationale).
//!
//! These tests were moved verbatim (in intent) out of `crates/kriya/src/crypto.rs`'s inline
//! `#[cfg(test)] mod tests` into this standalone sibling crate so that `crates/kriya` carries
//! NO `[dev-dependencies]` (the `ml-dsa` dev-dep was the only one) — which is required for
//! `cargo test -p kriya` to work from the `apps/note-app/src-tauri` workspace, where `kriya`
//! is a non-member path dependency. The public PQ API used here is re-exported by kriya as
//! `kriya::crypto::{PqSigningKey, pq_verify}` (behind the `pq-crypto` feature, always on for
//! this crate). No `#[cfg(feature = ...)]` gating needed here since the dep pins `pq-crypto`.

use kriya::crypto::{pq_verify, PqSigningKey};

/// A5 (design D7): cross-implementation parity — `aws-lc-rs` (production) signs, RustCrypto's
/// `ml-dsa` (test-only, unaudited — V4) independently verifies. See the reverse direction in
/// [`pq_cross_impl_rustcrypto_signs_aws_lc_verifies`]. Two independent ML-DSA-87
/// implementations agreeing is the strongest cross-impl assurance available without a JS build
/// of aws-lc (D7's rationale).
#[test]
fn pq_cross_impl_aws_lc_signs_rustcrypto_verifies() {
    use ml_dsa::{EncodedVerifyingKey, MlDsa87, Verifier as _, VerifyingKey};

    let seed = [11u8; 32];
    let key = PqSigningKey::from_seed(&seed).unwrap();
    let pk_bytes = key.public_key();
    let msg = b"kriya A5 cross-impl parity fixture (crates/kriya)";
    let sig_bytes = key.sign(msg);
    assert!(pq_verify(&pk_bytes, msg, &sig_bytes));

    let encoded_vk = EncodedVerifyingKey::<MlDsa87>::try_from(pk_bytes.as_slice())
        .expect("aws-lc-rs public key decodes as a valid RustCrypto verifying key");
    let vk = VerifyingKey::<MlDsa87>::decode(&encoded_vk);
    let sig = ml_dsa::Signature::<MlDsa87>::try_from(sig_bytes.as_slice())
        .expect("aws-lc-rs signature decodes as a valid RustCrypto signature");
    assert!(
        vk.verify(msg, &sig).is_ok(),
        "RustCrypto must independently verify an aws-lc-rs-produced ML-DSA-87 signature"
    );
}

#[test]
fn pq_cross_impl_rustcrypto_signs_aws_lc_verifies() {
    use ml_dsa::{Generate, Keypair as _, MlDsa87, Signer as _, SigningKey};

    let sk = SigningKey::<MlDsa87>::generate();
    let msg = b"kriya A5 cross-impl parity fixture, reverse direction (crates/kriya)";
    let sig = sk.sign(msg);
    let pk_bytes: Vec<u8> = sk.verifying_key().encode().as_slice().to_vec();
    let sig_bytes: Vec<u8> = sig.encode().as_slice().to_vec();
    assert!(
        pq_verify(&pk_bytes, msg, &sig_bytes),
        "aws-lc-rs must independently verify a RustCrypto-produced ML-DSA-87 signature"
    );
}
