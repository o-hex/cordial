//! A `sha256:` content hash, spelled the way ADR-014 spells one.
//!
//! Deliberately the same text form as `cordial_plugins::registry::ContentHash`
//! — `sha256:` followed by 64 lower-case hex digits — and deliberately not that
//! type. `ContentHash::of` takes a slice, which for a plugin archive is a few
//! hundred kilobytes and for an APK is 115 MB resident before the first check
//! can happen. This one is fed a block at a time by [`crate::download`] as the
//! bytes come off the socket, so nothing is ever held whole.
//!
//! If the two ever have to interoperate, they agree on the text: parse one,
//! print the other.

use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sha256Hash([u8; 32]);

impl Sha256Hash {
    pub fn of(bytes: &[u8]) -> Self {
        let mut h = Hasher::new();
        h.update(bytes);
        h.finish()
    }

    /// Refuses a bare hex string rather than assuming an algorithm, for the
    /// reason ADR-014's parser does: the day a second algorithm exists, every
    /// unprefixed hash already written down becomes ambiguous.
    pub fn parse(text: &str) -> Result<Self, String> {
        let hex = text.strip_prefix("sha256:").ok_or_else(|| {
            format!("{text:?} does not name its algorithm; write \"sha256:<64 hex digits>\"")
        })?;
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("{text:?} is not 64 lower-case hex digits after \"sha256:\""));
        }
        let mut digest = [0u8; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("{text:?}: {e}"))?;
        }
        Ok(Sha256Hash(digest))
    }
}

impl fmt::Display for Sha256Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sha256:")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// The running state of a hash over a stream.
pub struct Hasher(Sha256);

impl Hasher {
    pub fn new() -> Self {
        Hasher(Sha256::new())
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(self) -> Sha256Hash {
        let out = self.0.finalize();
        let mut digest = [0u8; 32];
        digest.copy_from_slice(&out);
        Sha256Hash(digest)
    }
}

impl Default for Hasher {
    fn default() -> Self {
        Hasher::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_form_round_trips() {
        let h = Sha256Hash::of(b"cordial");
        assert_eq!(Sha256Hash::parse(&h.to_string()).unwrap(), h);
    }

    #[test]
    fn a_hash_taken_in_pieces_equals_one_taken_whole() {
        // The whole point of the streaming form. If these ever disagreed, a
        // 115 MB download would fail verification for a reason nothing in the
        // message would explain.
        let whole = Sha256Hash::of(b"the quick brown fox");
        let mut h = Hasher::new();
        h.update(b"the quick ");
        h.update(b"brown fox");
        assert_eq!(h.finish(), whole);
    }

    #[test]
    fn the_empty_hash_is_the_published_one() {
        // A fixed vector, so a wrong digest cannot pass by agreeing with itself.
        assert_eq!(
            Sha256Hash::of(b"").to_string(),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn a_hash_without_its_algorithm_is_refused() {
        let bare = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(Sha256Hash::parse(bare).unwrap_err().contains("sha256:"));
        assert!(Sha256Hash::parse("sha256:nope").is_err());
        // Upper case is refused rather than folded: two spellings of one hash
        // is two things a comparison can be wrong about.
        assert!(Sha256Hash::parse(&format!("sha256:{}", bare.to_uppercase())).is_err());
    }
}
