use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use mongodb::bson::{Binary, Bson, Document, doc, spec::BinarySubtype};
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

pub struct MasterKey([u8; 32]);

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl MasterKey {
    pub fn from_hex(value: &str) -> Result<Self, &'static str> {
        let bytes = hex::decode(value).map_err(|_| "Invalid BYOK encryption key encoding")?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "BYOK encryption key must be 32 bytes")?;
        Ok(Self(key))
    }

    pub fn seal(
        &self,
        guild_id: u64,
        provider: &str,
        plaintext: &str,
    ) -> Result<Document, &'static str> {
        let cipher = Aes256Gcm::new_from_slice(&self.0).map_err(|_| "Encryption unavailable")?;
        let mut nonce = [0_u8; 12];
        rand::rng().fill_bytes(&mut nonce);
        let aad = format!("omnicore-byok:v1:{guild_id}:{provider}");
        let ciphertext = aes_gcm::aead::Aead::encrypt(
            &cipher,
            Nonce::from_slice(&nonce),
            aes_gcm::aead::Payload {
                msg: plaintext.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| "Encryption failed")?;
        Ok(doc! {
            "key_version": 1,
            "nonce": Bson::Binary(Binary { subtype: BinarySubtype::Generic, bytes: nonce.to_vec() }),
            "ciphertext": Bson::Binary(Binary { subtype: BinarySubtype::Generic, bytes: ciphertext }),
        })
    }

    pub fn open(
        &self,
        guild_id: u64,
        provider: &str,
        sealed: &Document,
    ) -> Result<Zeroizing<String>, &'static str> {
        if sealed.get_i32("key_version").ok() != Some(1) {
            return Err("Unsupported encryption key version");
        }
        let nonce = sealed
            .get_binary_generic("nonce")
            .map_err(|_| "Missing encryption nonce")?;
        if nonce.len() != 12 {
            return Err("Invalid encryption nonce");
        }
        let ciphertext = sealed
            .get_binary_generic("ciphertext")
            .map_err(|_| "Missing encrypted key")?;
        let cipher = Aes256Gcm::new_from_slice(&self.0).map_err(|_| "Decryption unavailable")?;
        let aad = format!("omnicore-byok:v1:{guild_id}:{provider}");
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                aes_gcm::aead::Payload {
                    msg: ciphertext,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "Credential authentication failed")?;
        let text = String::from_utf8(plaintext).map_err(|_| "Credential encoding invalid")?;
        Ok(Zeroizing::new(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authenticated_encryption_is_bound_to_guild_and_provider() {
        let master = MasterKey([9; 32]);
        let sealed = master.seal(1, "openai", "test-secret").unwrap();
        assert_eq!(
            master.open(1, "openai", &sealed).unwrap().as_str(),
            "test-secret"
        );
        assert!(master.open(2, "openai", &sealed).is_err());
        assert!(master.open(1, "gemini", &sealed).is_err());
        assert_ne!(
            master
                .seal(1, "openai", "test-secret")
                .unwrap()
                .get("nonce"),
            sealed.get("nonce")
        );
        assert!(!format!("{sealed:?}").contains("test-secret"));
    }
}
