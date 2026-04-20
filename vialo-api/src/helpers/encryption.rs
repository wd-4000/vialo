use chacha20poly1305::{
    Key, XChaCha20Poly1305,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use std::{fmt, marker::PhantomData, sync::OnceLock};

/// Globally store the encryption key
static KEY: OnceLock<[u8; 32]> = OnceLock::new();

pub fn init(key: Key) {
    KEY.set(key.as_slice().try_into().unwrap())
        .expect("encryption::init called more than once. GO. AWAY.");
}

fn get_cipher() -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(Key::from_slice(KEY.get().expect(
        "Who do you think you are? Call encryption::init before using Encrypted<T>",
    )))
}

pub fn decrypt<T: for<'de> Deserialize<'de>>(blob: &[u8]) -> Result<T, anyhow::Error> {
    Ok(serde_json::from_slice(&decrypt_raw(blob)?)?)
}

pub fn encrypt<T: Serialize>(value: &T) -> Result<Vec<u8>, anyhow::Error> {
    encrypt_raw(&serde_json::to_vec(value)?)
}

fn encrypt_raw(plaintext: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
    let nonce: [u8; 24] = XChaCha20Poly1305::generate_nonce(&mut OsRng).into();
    let mut out = nonce.to_vec();
    out.extend_from_slice(&get_cipher().encrypt(&nonce.into(), plaintext)?);
    Ok(out)
}

fn decrypt_raw(blob: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
    anyhow::ensure!(blob.len() > 24, "encrypted blob too short");
    let nonce: [u8; 24] = blob[..24].try_into().unwrap();
    Ok(get_cipher().decrypt(&nonce.into(), &blob[24..])?)
}

/// Stores encrypted data in `BYTEA` cols (`[24-byte nonce | ciphertext]`).
/// RUn `encryption::init` before using. Also awesome and secret-cy.
pub struct Encrypted<T> {
    inner: SecretBox<Vec<u8>>,
    _phantom: PhantomData<T>,
}

impl<T> Encrypted<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    pub fn new(value: &T) -> Self {
        Self {
            inner: SecretBox::new(Box::new(
                serde_json::to_vec(value).expect("failed to serialize"),
            )),
            _phantom: PhantomData,
        }
    }

    pub fn expose(&self) -> T {
        serde_json::from_slice(self.inner.expose_secret()).expect("failed to deserialize")
    }
}

pub fn serialize_exposed<T, S>(val: &Encrypted<T>, s: S) -> Result<S::Ok, S::Error>
where
    T: Serialize + for<'de> Deserialize<'de>,
    S: serde::Serializer,
{
    val.expose().serialize(s)
}

pub fn serialize_exposed_opt<T, S>(val: &Option<Encrypted<T>>, s: S) -> Result<S::Ok, S::Error>
where
    T: Serialize + for<'de> Deserialize<'de>,
    S: serde::Serializer,
{
    val.as_ref().map(|e| e.expose()).serialize(s)
}

impl<'de, T> Deserialize<'de> for Encrypted<T>
where
    T: Serialize + for<'de2> Deserialize<'de2>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(&T::deserialize(deserializer)?))
    }
}

impl<T> Clone for Encrypted<T> {
    fn clone(&self) -> Self {
        Self {
            inner: SecretBox::new(Box::new(self.inner.expose_secret().clone())),
            _phantom: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Encrypted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Encrypted([this data has been destroyed])")
    }
}

impl<T> sqlx::Type<sqlx::Postgres> for Encrypted<T> {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <Vec<u8> as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

impl<'r, T> sqlx::Decode<'r, sqlx::Postgres> for Encrypted<T>
where
    T: for<'de> Deserialize<'de>,
{
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = <Vec<u8> as sqlx::Decode<'r, sqlx::Postgres>>::decode(value)?;
        Ok(Self {
            inner: SecretBox::new(Box::new(decrypt_raw(&bytes)?)),
            _phantom: PhantomData,
        })
    }
}

impl<'q, T> sqlx::Encode<'q, sqlx::Postgres> for Encrypted<T>
where
    T: Serialize,
{
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        let encrypted = encrypt_raw(self.inner.expose_secret())?;
        <Vec<u8> as sqlx::Encode<'q, sqlx::Postgres>>::encode_by_ref(&encrypted, buf)
    }
}
