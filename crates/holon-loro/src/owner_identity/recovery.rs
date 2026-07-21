//! One-time recovery code for the owner-identity key (ADR 0028 D1).
//!
//! The owner key is a 32-byte Ed25519 seed. The recovery code is a
//! **BIP39-style mnemonic** over that seed: at first enrollment Holon shows the
//! words once; the user writes them down; re-entering them on a new founding
//! device re-derives the identical seed → the identical owner key.
//!
//! # Derivation (documented precisely — the `bip39` crate is not in the tree,
//! so this is a self-contained, dependency-free encoding)
//!
//! - `entropy` = the 32-byte Ed25519 seed (256 bits).
//! - `checksum` = the first byte (8 bits) of `blake3(entropy)`.
//! - `bits` = `entropy(256) || checksum(8)` = 264 bits.
//! - Split `bits` into 24 groups of 11 bits, **MSB-first** (identical bit order
//!   to BIP39); each group indexes the canonical BIP39 English wordlist
//!   (`bip39_english.txt`, 2048 words).
//! - The mnemonic is the 24 space-separated words.
//!
//! This is *BIP39-style*, not wallet-interoperable: BIP39 uses a SHA-256
//! checksum; we use blake3 (already a first-class dependency) since the code is
//! an internal recovery artifact, not a cross-wallet seed phrase. The wordlist
//! itself is the canonical BIP39 English list so the words stay familiar and
//! transcribable.
//!
//! # Fail-loud
//! Decoding rejects — never silently repairs — an unknown word, a wrong word
//! count, or a checksum mismatch (a transcription error). A bad code yields a
//! clear `Err`, never a wrong-but-plausible key.

use std::collections::HashMap;
use std::sync::OnceLock;

use anyhow::Result;
use anyhow::bail;

const WORDLIST_RAW: &str = include_str!("bip39_english.txt");
const WORD_COUNT: usize = 24;
const SEED_LEN: usize = 32;

fn wordlist() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        let words: Vec<&'static str> = WORDLIST_RAW.lines().map(|l| l.trim()).collect();
        assert_eq!(
            words.len(),
            2048,
            "bip39_english.txt must contain exactly 2048 words, found {}",
            words.len()
        );
        words
    })
}

fn word_index() -> &'static HashMap<&'static str, u16> {
    static INDEX: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    INDEX.get_or_init(|| {
        wordlist()
            .iter()
            .enumerate()
            .map(|(i, w)| (*w, i as u16))
            .collect()
    })
}

/// A one-time owner-key recovery mnemonic. `Debug` is redacted: the words
/// re-derive the owner key, so they must never land in a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryCode(String);

impl std::fmt::Debug for RecoveryCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecoveryCode(<redacted 24-word mnemonic>)")
    }
}

impl RecoveryCode {
    /// Encode a 32-byte owner seed as a 24-word mnemonic.
    pub fn encode(seed: &[u8; SEED_LEN]) -> Self {
        let mut bits = [0u8; SEED_LEN + 1];
        bits[..SEED_LEN].copy_from_slice(seed);
        bits[SEED_LEN] = blake3::hash(seed).as_bytes()[0];

        let words = wordlist();
        let mut out: Vec<&str> = Vec::with_capacity(WORD_COUNT);
        for group in 0..WORD_COUNT {
            let mut idx: usize = 0;
            for b in 0..11 {
                let bit_pos = group * 11 + b;
                let byte = bits[bit_pos / 8];
                let bit = (byte >> (7 - (bit_pos % 8))) & 1;
                idx = (idx << 1) | bit as usize;
            }
            out.push(words[idx]);
        }
        RecoveryCode(out.join(" "))
    }

    /// The mnemonic words, for one-time display to the user. Callers MUST show
    /// this exactly once and never persist or log it.
    pub fn reveal(&self) -> &str {
        &self.0
    }

    /// Parse a user-entered mnemonic back into a `RecoveryCode`. Whitespace is
    /// normalized and case is lowered; the words themselves are validated on
    /// [`Self::decode`].
    pub fn from_phrase(phrase: &str) -> Self {
        let normalized = phrase.split_whitespace().collect::<Vec<_>>().join(" ");
        RecoveryCode(normalized.to_lowercase())
    }

    /// Recover the 32-byte owner seed. Rejects loudly on any transcription
    /// error rather than returning a plausible-but-wrong seed.
    pub fn decode(&self) -> Result<[u8; SEED_LEN]> {
        let words: Vec<&str> = self.0.split_whitespace().collect();
        if words.len() != WORD_COUNT {
            bail!(
                "recovery code must be {WORD_COUNT} words, got {}",
                words.len()
            );
        }
        let index = word_index();
        let mut bits = [0u8; SEED_LEN + 1];
        for (group, w) in words.iter().enumerate() {
            let idx = *index.get(w).ok_or_else(|| {
                anyhow::anyhow!("recovery code word #{} is not in the wordlist", group + 1)
            })?;
            for b in 0..11 {
                let bit = (idx >> (10 - b)) & 1;
                if bit == 1 {
                    let bit_pos = group * 11 + b;
                    bits[bit_pos / 8] |= 1 << (7 - (bit_pos % 8));
                }
            }
        }
        let mut seed = [0u8; SEED_LEN];
        seed.copy_from_slice(&bits[..SEED_LEN]);
        let expected = blake3::hash(&seed).as_bytes()[0];
        if bits[SEED_LEN] != expected {
            bail!("recovery code checksum mismatch — likely a transcription error");
        }
        Ok(seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_arbitrary_seed() {
        let seed = [0x5au8; SEED_LEN];
        let code = RecoveryCode::encode(&seed);
        assert_eq!(code.reveal().split_whitespace().count(), WORD_COUNT);
        assert_eq!(code.decode().unwrap(), seed);
    }

    #[test]
    fn known_all_zero_seed_round_trips() {
        let seed = [0u8; SEED_LEN];
        let code = RecoveryCode::encode(&seed);
        // All-zero entropy → first 23 words "abandon"; last word carries the
        // checksum bits so it is NOT "abandon".
        assert!(code.reveal().starts_with("abandon abandon"));
        assert_eq!(code.decode().unwrap(), seed);
    }

    #[test]
    fn every_bit_survives() {
        for byte in 0..SEED_LEN {
            for bit in 0..8u8 {
                let mut seed = [0u8; SEED_LEN];
                seed[byte] = 1 << bit;
                let code = RecoveryCode::encode(&seed);
                assert_eq!(code.decode().unwrap(), seed, "byte {byte} bit {bit}");
            }
        }
    }

    #[test]
    fn from_phrase_normalizes_whitespace_and_case() {
        let seed = [0x11u8; SEED_LEN];
        let code = RecoveryCode::encode(&seed);
        let messy = format!("  {}  ", code.reveal().to_uppercase().replace(' ', "   "));
        let reparsed = RecoveryCode::from_phrase(&messy);
        assert_eq!(reparsed.decode().unwrap(), seed);
    }

    #[test]
    fn rejects_wrong_word_count() {
        let err = RecoveryCode::from_phrase("abandon abandon abandon")
            .decode()
            .unwrap_err();
        assert!(format!("{err}").contains("24 words"));
    }

    #[test]
    fn rejects_unknown_word() {
        let seed = [0x22u8; SEED_LEN];
        let good = RecoveryCode::encode(&seed);
        let mut words: Vec<&str> = good.reveal().split_whitespace().collect();
        words[5] = "notarealbip39word";
        let tampered = RecoveryCode::from_phrase(&words.join(" "));
        let err = tampered.decode().unwrap_err();
        assert!(format!("{err}").contains("not in the wordlist"));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        // Swap one word for another valid word: indices still decode but the
        // checksum no longer matches the entropy.
        let seed = [0x33u8; SEED_LEN];
        let good = RecoveryCode::encode(&seed);
        let mut words: Vec<String> = good.reveal().split_whitespace().map(String::from).collect();
        // Replace the FIRST word (entropy-bearing) with a different valid word.
        words[0] = if words[0] == "zoo" {
            "zone".into()
        } else {
            "zoo".into()
        };
        let tampered = RecoveryCode::from_phrase(&words.join(" "));
        let err = tampered.decode().unwrap_err();
        assert!(format!("{err}").contains("checksum mismatch"));
    }

    #[test]
    fn debug_is_redacted() {
        let code = RecoveryCode::encode(&[7u8; SEED_LEN]);
        assert_eq!(
            format!("{code:?}"),
            "RecoveryCode(<redacted 24-word mnemonic>)"
        );
        assert!(!format!("{code:?}").contains("abandon"));
    }
}
