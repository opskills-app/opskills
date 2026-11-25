//! Encryption for vault secrets: AES-256-GCM with the server's master key.
//!
//! Each value gets a fresh random 96-bit nonce, and the secret's name is bound in as
//! associated data, so a ciphertext can't be moved to another secret.

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use anyhow::{Context, Result, anyhow};

use crate::firststart::random_bytes;

pub struct Vault {
    cipher: Aes256Gcm,
}

pub struct Sealed {
    pub nonce: String,
    pub ciphertext: String,
}

impl Vault {
    pub fn from_key_hex(key_hex: &str) -> Result<Self> {
        let bytes = hex::decode(key_hex.trim()).context("master.key is not valid hex")?;
        anyhow::ensure!(bytes.len() == 32, "master.key must hold 32 bytes, found {}", bytes.len());
        let key = aes_gcm::Key::<Aes256Gcm>::try_from(&bytes[..]).map_err(|_| anyhow!("master.key has the wrong length"))?;
        Ok(Vault { cipher: Aes256Gcm::new(&key) })
    }

    pub fn seal(&self, name: &str, plaintext: &[u8]) -> Result<Sealed> {
        let nonce_bytes = random_bytes::<12>();
        let nonce = aes_gcm::Nonce::try_from(&nonce_bytes[..]).map_err(|_| anyhow!("bad nonce length"))?;
        let ciphertext = self
            .cipher
            .encrypt(&nonce, Payload { msg: plaintext, aad: name.as_bytes() })
            .map_err(|_| anyhow!("encryption failed"))?;
        Ok(Sealed { nonce: hex::encode(nonce_bytes), ciphertext: hex::encode(ciphertext) })
    }

    pub fn open(&self, name: &str, sealed: &Sealed) -> Result<Vec<u8>> {
        let nonce_bytes = hex::decode(&sealed.nonce).context("stored nonce is not hex")?;
        let nonce = aes_gcm::Nonce::try_from(&nonce_bytes[..]).map_err(|_| anyhow!("stored nonce has the wrong length"))?;
        let ciphertext = hex::decode(&sealed.ciphertext).context("stored ciphertext is not hex")?;
        self.cipher
            .decrypt(&nonce, Payload { msg: &ciphertext, aad: name.as_bytes() })
            .map_err(|_| anyhow!("could not decrypt secret {name}: the data or the master key changed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        Vault::from_key_hex(&hex::encode(random_bytes::<32>())).unwrap()
    }

    #[test]
    fn round_trip_and_binding() {
        let v = vault();
        let sealed = v.seal("DB_PASSWORD", b"hunter2-hunter2").unwrap();
        assert!(!sealed.ciphertext.contains(&hex::encode(b"hunter2")));
        assert_eq!(v.open("DB_PASSWORD", &sealed).unwrap(), b"hunter2-hunter2");
        // Bound to its name: the same ciphertext under another name fails.
        assert!(v.open("OTHER", &sealed).is_err());
        // A different key can't read it.
        assert!(vault().open("DB_PASSWORD", &sealed).is_err());
        // Fresh nonce every time.
        assert_ne!(v.seal("A", b"same").unwrap().nonce, v.seal("A", b"same").unwrap().nonce);
    }

    #[test]
    fn rejects_bad_keys() {
        assert!(Vault::from_key_hex("not hex").is_err());
        assert!(Vault::from_key_hex(&hex::encode([0u8; 16])).is_err());
    }
}
