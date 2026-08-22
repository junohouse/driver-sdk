//! SHA-1, SHA-256 and AES-128-CBC. See the module doc on [`crate::crypto`] for why these live
//! here rather than in each driver that needs them.
//!
//! Plain functions, not a builder or a trait: every one of these is a value in, a value out,
//! and a driver already owns the state (a session key, an IV that advances) that would
//! otherwise live inside an object here. `tapo`'s `klap.rs` is the shape this was extracted
//! from — read it for how a driver holds that state between calls.

use aes::Aes128;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha1::Sha1;
use sha2::{Digest, Sha256};

type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;

pub fn sha1(data: &[u8]) -> [u8; 20] {
    Sha1::digest(data).into()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// AES-128-CBC, PKCS#7 padded.
pub fn aes128_cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    Encryptor::new(key.into(), iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

/// The reverse. `None` means the padding did not check out — which, for CBC, is only ever
/// meaningful as "wrong key or wrong IV, look there first," never as tamper evidence: every
/// block but the first decodes from the ciphertext before it regardless of whether either side
/// agrees on anything, so a *wrong* key still often produces bytes that pad correctly by
/// chance. A caller that needs to know the sender was authentic needs a MAC over the
/// ciphertext, which this does not compute and does not check — see `tapo::klap::Session`,
/// which treats its own JSON parse as that check because the device sends no MAC to verify.
pub fn aes128_cbc_decrypt(key: &[u8; 16], iv: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    Decryptor::new(key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two vectors this crate can quote from memory without risking a transcription error:
    /// SHA-1 and SHA-256 of the three-byte string every hash spec since FIPS 180 has used as
    /// its own worked example.
    #[test]
    fn sha1_and_sha256_match_the_textbook_vectors_for_abc() {
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// FIPS-197 Appendix B, the reference example for AES-128 — single block, key and
    /// plaintext straight out of the spec. CBC with an all-zero IV reduces to plain AES on the
    /// first block, which is what makes that one example enough to check this against: no
    /// CBC-specific test vector needed to know the block cipher itself is right.
    #[test]
    fn one_block_against_the_fips_197_worked_example() {
        let key: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let plaintext: [u8; 16] = unhex("00112233445566778899aabbccddeeff").try_into().unwrap();
        let iv = [0u8; 16];

        // PKCS#7 pads a full-length message with a whole extra block, so only the first 16
        // bytes of the encryption are the FIPS vector — the second block is the padding.
        let ciphertext = aes128_cbc_encrypt(&key, &iv, &plaintext);
        assert_eq!(hex(&ciphertext[..16]), "69c4e0d86a7b0430d8cdb78070b4c55a");

        assert_eq!(aes128_cbc_decrypt(&key, &iv, &ciphertext).as_deref(), Some(&plaintext[..]));
    }

    /// The property the vector above cannot exercise on its own: a real key derived once, an
    /// IV that moves with a sequence number the way `tapo`'s session does, and a payload that
    /// is not a round 16 bytes.
    #[test]
    fn a_key_and_a_moving_iv_round_trip_an_arbitrary_message() {
        let key = sha256(b"a session key")[..16].try_into().unwrap();
        for seq in 0u8..4 {
            let mut iv = [0u8; 16];
            iv[15] = seq;
            let plain = format!("request number {seq}, a message of no particular length");
            let ciphertext = aes128_cbc_encrypt(&key, &iv, plain.as_bytes());
            assert_eq!(
                aes128_cbc_decrypt(&key, &iv, &ciphertext).as_deref(),
                Some(plain.as_bytes())
            );
            // A different IV must not open it — this is what catches a driver that forgets to
            // advance one and reuses it across requests.
            let mut wrong_iv = iv;
            wrong_iv[0] ^= 1;
            assert_ne!(
                aes128_cbc_decrypt(&key, &wrong_iv, &ciphertext).as_deref(),
                Some(plain.as_bytes())
            );
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
