use crate::{Error, MAX_SECRET, Result, Secret};
use age::secrecy::ExposeSecret;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub id: String,
    pub label: String,
    pub signing_key: String,
    pub encryption_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub machine: Machine,
    pub proof: String,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub(crate) struct Identity {
    signing: [u8; 32],
    encryption: String,
}

pub(crate) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| Error::InvalidRecord)
}
pub(crate) fn verify<T: Serialize>(key: &str, value: &T, signature: &str) -> Result<()> {
    let bytes: [u8; 32] = B64
        .decode(key)
        .map_err(|_| Error::InvalidRecord)?
        .try_into()
        .map_err(|_| Error::InvalidRecord)?;
    let sig = Signature::from_slice(&B64.decode(signature).map_err(|_| Error::InvalidRecord)?)
        .map_err(|_| Error::InvalidRecord)?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|_| Error::InvalidRecord)?
        .verify_strict(&encode(value)?, &sig)
        .map_err(|_| Error::InvalidRecord)
}
impl Enrollment {
    pub fn validate(&self) -> Result<()> {
        let m = &self.machine;
        if m.id != hey_boss_sync::digest(m.signing_key.as_bytes())
            || m.label.is_empty()
            || m.label.len() > 80
            || m.label.chars().any(char::is_control)
        {
            return Err(Error::InvalidRecord);
        }
        m.encryption_key
            .parse::<age::x25519::Recipient>()
            .map_err(|_| Error::InvalidRecord)?;
        verify(&m.signing_key, m, &self.proof)
    }
}
impl Identity {
    pub fn generate() -> Self {
        let signing = SigningKey::generate(&mut rand::rngs::OsRng).to_bytes();
        let encryption = age::x25519::Identity::generate()
            .to_string()
            .expose_secret()
            .to_string();
        Self {
            signing,
            encryption,
        }
    }
    pub fn enrollment(&self, label: &str) -> Result<Enrollment> {
        let key = SigningKey::from_bytes(&self.signing);
        let signing_key = B64.encode(key.verifying_key().to_bytes());
        let encryption_key = self.age()?.to_public().to_string();
        let machine = Machine {
            id: hey_boss_sync::digest(signing_key.as_bytes()),
            label: label.into(),
            signing_key,
            encryption_key,
        };
        let proof = self.sign(&machine)?;
        let e = Enrollment { machine, proof };
        e.validate()?;
        Ok(e)
    }
    pub fn sign<T: Serialize>(&self, value: &T) -> Result<String> {
        Ok(B64.encode(
            SigningKey::from_bytes(&self.signing)
                .sign(&encode(value)?)
                .to_bytes(),
        ))
    }
    fn age(&self) -> Result<age::x25519::Identity> {
        self.encryption.parse().map_err(|_| Error::CryptoFailure)
    }
    pub fn decrypt(&self, ciphertext: &str) -> Result<Secret> {
        let bytes = B64.decode(ciphertext).map_err(|_| Error::InvalidRecord)?;
        let identity = self.age()?;
        let decryptor = age::Decryptor::new(bytes.as_slice()).map_err(|_| Error::CryptoFailure)?;
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .map_err(|_| Error::CryptoFailure)?;
        let mut clear = zeroize::Zeroizing::new(Vec::new());
        reader
            .by_ref()
            .take(MAX_SECRET as u64 + 1)
            .read_to_end(&mut clear)
            .map_err(|_| Error::CryptoFailure)?;
        if clear.len() > MAX_SECRET {
            return Err(Error::TooLarge);
        }
        Ok(Secret::new(std::mem::take(&mut *clear)))
    }
}
pub(crate) fn encrypt(bytes: &[u8], recipients: &[Enrollment]) -> Result<String> {
    let recipients: Vec<age::x25519::Recipient> = recipients
        .iter()
        .map(|r| {
            r.machine
                .encryption_key
                .parse()
                .map_err(|_| Error::InvalidRecord)
        })
        .collect::<Result<_>>()?;
    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|_| Error::CryptoFailure)?;
    let mut encrypted = Vec::new();
    let mut stream = encryptor
        .wrap_output(&mut encrypted)
        .map_err(|_| Error::CryptoFailure)?;
    stream.write_all(bytes).map_err(|_| Error::CryptoFailure)?;
    stream.finish().map_err(|_| Error::CryptoFailure)?;
    Ok(B64.encode(encrypted))
}
