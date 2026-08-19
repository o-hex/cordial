//! Checking an index's signature before anything in it is trusted.
//!
//! [ADR-014](../../../docs/adr/ADR-014-plugin-registry-and-unpacking.md) names
//! the scheme and says plainly that nothing implements it: "the only entry
//! point is `Index::parse_unverified`... an index is exactly as trustworthy
//! as the transport it arrived over." This module is that implementation — a
//! detached minisign (Ed25519) signature, checked against the raw bytes
//! **before** `serde_json` ever sees them, exactly as the ADR specifies.
//!
//! It does not, by itself, make that quoted sentence untrue. What it changes
//! is that a signature *can* now be checked; it does not decide *which key*
//! Cordial should check one against. That is still nobody's to answer here —
//! picking a key means picking whose index gets to be trusted by default,
//! which is the same policy question ADR-014 leaves to whoever ends up
//! hosting one. Nothing in this crate ships a key, hardcodes one, or falls
//! back to trusting an index that has none configured; see [`crate::marketplace`],
//! which is the only thing that ever calls [`verify`] and which refuses to
//! install anything when no key has been given to it.

use crate::registry::Index;
use minisign_verify::{PublicKey, Signature};
use std::fmt;

/// Why a signature did not check out. Every one of these is a refusal with a
/// name, in the house style — an index that fails to verify has to say why,
/// not just that it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The text handed in as a public key is not a minisign key at all.
    MalformedKey(String),
    /// The text handed in as a signature does not parse as one.
    MalformedSignature(String),
    /// It parsed, but does not verify against this key and these bytes.
    Invalid(String),
    /// The signature checked out, but the bytes it vouches for are not a
    /// readable index. A signature proves who published the bytes, not that
    /// the bytes are the format this Cordial understands — see
    /// `Index::parse_unverified`, which is what performs that second check.
    Parse(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::MalformedKey(m) => write!(f, "not a usable minisign public key: {m}"),
            Refusal::MalformedSignature(m) => write!(f, "not a usable minisign signature: {m}"),
            Refusal::Invalid(m) => write!(f, "signature does not verify: {m}"),
            Refusal::Parse(m) => write!(f, "signature verified, but the index is not readable: {m}"),
        }
    }
}

/// Parse a base64 minisign public key — the string `minisign -G` prints, and
/// what a `.pub` file's second line holds.
pub fn parse_key(base64: &str) -> Result<PublicKey, Refusal> {
    PublicKey::from_base64(base64.trim()).map_err(|e| Refusal::MalformedKey(e.to_string()))
}

/// Verify `json` against `signature_text` and `key`, and only then parse it.
///
/// The order is the whole point, and it is the order ADR-014 specifies: the
/// signature is checked against the *bytes* before `serde_json` ever runs
/// over them, so a parser bug is never reachable from data nobody has vouched
/// for. Swapping the two — parse first, verify second — would still be
/// memory-safe in Rust, but it would mean every future format quirk this
/// module has to tolerate for parsing is now reachable from anonymous bytes,
/// which is exactly the exposure signing exists to remove.
pub fn verify(json: &str, signature_text: &str, key: &PublicKey) -> Result<Index, Refusal> {
    let signature =
        Signature::decode(signature_text.trim()).map_err(|e| Refusal::MalformedSignature(e.to_string()))?;
    key.verify(json.as_bytes(), &signature, false).map_err(|e| Refusal::Invalid(e.to_string()))?;
    Index::parse_unverified(json).map_err(Refusal::Parse)
}

/// Building minisign key pairs and signatures for tests, since
/// `minisign-verify` is deliberately verify-only and ships no signer.
///
/// This reimplements just enough of the wire format — read from the crate's
/// own `Signature::decode`/`PublicKey::from_base64` above — to produce
/// fixtures `verify` will accept: a prehashed (BLAKE2b-512) message, signed
/// with Ed25519, in the same 42-byte key / 74-byte signature layout minisign
/// itself writes. It exists only so the tests below run against real
/// signatures rather than ones this module merely asserts are shaped right.
#[cfg(test)]
pub(crate) mod fixtures {
    use base64::Engine as _;
    use blake2::{Blake2b512, Digest};
    use ed25519_dalek::{Signer, SigningKey};

    /// Both bytes minisign-verify's `decode` accepts for a prehashed
    /// signature. See the match arm in `Signature::decode`: `(0x45, 0x44)`
    /// maps to `is_prehashed = true`, which is the branch `verify(_, _,
    /// false)` — no legacy allowance — takes.
    const ALG: [u8; 2] = [0x45, 0x44];
    const KEY_ID: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A keypair from a fixed 32-byte seed, so a test that wants two
    /// different keys can just pass two different seeds and stay
    /// deterministic — no `rand` dependency, and no chance of two test runs
    /// disagreeing about whether a signature was valid.
    pub(crate) struct Keypair {
        signing: SigningKey,
    }

    pub(crate) fn keypair(seed: u8) -> Keypair {
        Keypair { signing: SigningKey::from_bytes(&[seed; 32]) }
    }

    impl Keypair {
        /// The base64 text `sign::parse_key` accepts.
        pub(crate) fn public_base64(&self) -> String {
            let mut blob = Vec::with_capacity(42);
            blob.extend_from_slice(&ALG);
            blob.extend_from_slice(&KEY_ID);
            blob.extend_from_slice(self.signing.verifying_key().as_bytes());
            b64(&blob)
        }

        /// A full minisign signature document over `message`, in the
        /// four-line format `Signature::decode` parses: untrusted comment,
        /// the signature blob, trusted comment, the global signature.
        pub(crate) fn sign(&self, message: &[u8]) -> String {
            let mut hasher = Blake2b512::new();
            hasher.update(message);
            let digest = hasher.finalize();
            let sig = self.signing.sign(&digest);

            let mut sig_blob = Vec::with_capacity(74);
            sig_blob.extend_from_slice(&ALG);
            sig_blob.extend_from_slice(&KEY_ID);
            sig_blob.extend_from_slice(&sig.to_bytes());

            let trusted_comment = "timestamp:0\tfile:index.json";
            let mut global_message = Vec::with_capacity(64 + trusted_comment.len());
            global_message.extend_from_slice(&sig.to_bytes());
            global_message.extend_from_slice(trusted_comment.as_bytes());
            let global_sig = self.signing.sign(&global_message);

            format!(
                "untrusted comment: test fixture\n{}\ntrusted comment: {trusted_comment}\n{}\n",
                b64(&sig_blob),
                b64(&global_sig.to_bytes()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{"format":1,"plugins":[{"id":"flag-inspector","name":"Flag Inspector","version":"1.0.0","capabilities":["flags.read"],"dependencies":{},"url":"https://example.invalid/flag-inspector-1.0.0.tar.zst","hash":"sha256:0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8"}]}"#;

    #[test]
    fn a_correctly_signed_index_verifies_and_parses() {
        let pair = fixtures::keypair(1);
        let key = parse_key(&pair.public_base64()).unwrap();
        let signature = pair.sign(INDEX.as_bytes());
        let index = verify(INDEX, &signature, &key).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].id, "flag-inspector");
    }

    #[test]
    fn a_signature_from_the_wrong_key_is_refused() {
        // The refusal that matters most: a key configured for one publisher
        // must not accept bytes signed by somebody else, which is exactly
        // what "checked against a key shipped with Cordial" is for.
        let signer = fixtures::keypair(1);
        let other = fixtures::keypair(2);
        let wrong_key = parse_key(&other.public_base64()).unwrap();
        let signature = signer.sign(INDEX.as_bytes());
        let e = verify(INDEX, &signature, &wrong_key).unwrap_err();
        assert!(matches!(e, Refusal::Invalid(_)), "{e}");
    }

    #[test]
    fn a_signature_over_different_bytes_is_refused() {
        // The index text itself must be exactly what was signed — this is
        // what stops a mirror from serving a signed index for one release and
        // silently attaching it to a JSON body that lists a different one.
        let pair = fixtures::keypair(1);
        let key = parse_key(&pair.public_base64()).unwrap();
        let signature = pair.sign(INDEX.as_bytes());
        let tampered = INDEX.replace("1.0.0", "9.9.9");
        let e = verify(&tampered, &signature, &key).unwrap_err();
        assert!(matches!(e, Refusal::Invalid(_)), "{e}");
    }

    #[test]
    fn a_signature_that_verifies_but_is_not_json_is_refused_as_a_parse_error_not_a_pass() {
        // The two checks are genuinely separate: proving who signed the bytes
        // is not proving the bytes are a readable index, and a caller that
        // only checked "did verify() return Ok" without looking at what it
        // returned would treat this as success.
        let pair = fixtures::keypair(1);
        let key = parse_key(&pair.public_base64()).unwrap();
        let signature = pair.sign(b"not json at all");
        let e = verify("not json at all", &signature, &key).unwrap_err();
        assert!(matches!(e, Refusal::Parse(_)), "{e}");
    }

    #[test]
    fn a_malformed_signature_document_is_refused_rather_than_panicking() {
        let pair = fixtures::keypair(1);
        let key = parse_key(&pair.public_base64()).unwrap();
        let e = verify(INDEX, "not a minisign signature", &key).unwrap_err();
        assert!(matches!(e, Refusal::MalformedSignature(_)), "{e}");
    }

    #[test]
    fn a_malformed_key_is_refused_rather_than_panicking() {
        assert!(matches!(parse_key("not a key"), Err(Refusal::MalformedKey(_))));
        assert!(matches!(parse_key(""), Err(Refusal::MalformedKey(_))));
    }
}
