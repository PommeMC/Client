//! Vanilla `Crypt` as the client uses it: the login key exchange, and the
//! AES/CFB8 stream every packet goes through once the server asks for
//! encryption.

use aes::Aes128;
use aes::cipher::inout::InOutBuf;
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use rand::TryRng;
use rand::rngs::SysRng;

pub type Encryptor = cfb8::Encryptor<Aes128>;
pub type Decryptor = cfb8::Decryptor<Aes128>;

/// What `ClientHandshakePacketListenerImpl.handleHello` makes of the server's
/// hello: a fresh shared secret, and it and the server's challenge encrypted
/// to the server's public key.
pub struct KeyExchange {
    pub secret_key: [u8; 16],
    pub encrypted_secret_key: Vec<u8>,
    pub encrypted_challenge: Vec<u8>,
}

/// `Crypt.generateSecretKey` and `Crypt.encryptUsingKey`: a 128-bit AES key,
/// with RSA and PKCS#1 v1.5 padding to the DER-encoded `public_key`.
pub fn key_exchange(public_key: &[u8], challenge: &[u8]) -> Result<KeyExchange, String> {
    let mut secret_key = [0u8; 16];
    SysRng
        .try_fill_bytes(&mut secret_key)
        .map_err(|e| e.to_string())?;
    Ok(KeyExchange {
        secret_key,
        encrypted_secret_key: rsa_public_encrypt_pkcs1::encrypt(public_key, &secret_key)?,
        encrypted_challenge: rsa_public_encrypt_pkcs1::encrypt(public_key, challenge)?,
    })
}

/// `Crypt.getCipher("AES/CFB8/NoPadding")` for each direction: the secret is
/// both the key and the initial vector.
pub fn ciphers(secret_key: &[u8; 16]) -> (Encryptor, Decryptor) {
    (
        Encryptor::new(secret_key.into(), secret_key.into()),
        Decryptor::new(secret_key.into(), secret_key.into()),
    )
}

/// CFB8 works a byte at a time, so any length is a whole number of blocks.
pub fn encrypt(cipher: &mut Encryptor, bytes: &mut [u8]) {
    let (blocks, _) = InOutBuf::from(bytes).into_chunks();
    cipher.encrypt_blocks_inout(blocks);
}

pub fn decrypt(cipher: &mut Decryptor, bytes: &mut [u8]) {
    let (blocks, _) = InOutBuf::from(bytes).into_chunks();
    cipher.decrypt_blocks_inout(blocks);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes `javax.crypto`'s AES/CFB8/NoPadding gives for the same key
    /// and input, the stream carrying on from one packet to the next.
    #[test]
    fn the_stream_matches_javas_cipher_across_packets() {
        let (mut encryptor, mut decryptor) = ciphers(b"1234567890123456");
        let mut first: Vec<u8> = (0..10).collect();
        let mut second: Vec<u8> = (10..20).collect();
        encrypt(&mut encryptor, &mut first);
        encrypt(&mut encryptor, &mut second);
        assert_eq!(first, FIRST);
        assert_eq!(second, SECOND);

        decrypt(&mut decryptor, &mut first);
        decrypt(&mut decryptor, &mut second);
        assert_eq!(first, (0..10).collect::<Vec<u8>>());
        assert_eq!(second, (10..20).collect::<Vec<u8>>());
    }

    const FIRST: [u8; 10] = [117, 151, 183, 45, 229, 232, 43, 181, 121, 16];
    const SECOND: [u8; 10] = [185, 223, 129, 153, 173, 140, 133, 239, 59, 168];
}
