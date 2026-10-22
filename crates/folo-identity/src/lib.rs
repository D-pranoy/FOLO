//! The node's identity (RB-06 step 5, docs/specs/pairing-and-connect.md#identity):
//! a static X25519 key pair for Noise, and the node id and fingerprint
//! derived from its public key. The keys are stored base64url in
//! `secrets.json`; storing them is the store's job.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use blake2::{Blake2s256, Digest as _};
use x25519_dalek::{PublicKey, StaticSecret};

/// Bytes of the public key's hash that make the node id.
const ID_BYTES: usize = 10;
/// RFC 4648 base32, lower-cased.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Why stored keys can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// The private key isn't 32 bytes of base64url.
    BadPrivateKey,
    /// The stored public key doesn't belong to the private key.
    KeyMismatch,
    /// The operating system's random source failed.
    Random(String),
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPrivateKey => f.write_str("the private key isn't 32 bytes of base64url"),
            Self::KeyMismatch => f.write_str("the public key doesn't belong to the private key"),
            Self::Random(why) => write!(f, "no random bytes: {why}"),
        }
    }
}

impl std::error::Error for IdentityError {}

/// A node's static X25519 key pair.
pub struct Identity {
    secret: StaticSecret,
    public: PublicKey,
}

impl Identity {
    /// A new key pair from the operating system's random source.
    ///
    /// # Errors
    ///
    /// [`IdentityError::Random`] when the random source fails.
    pub fn generate() -> Result<Self, IdentityError> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| IdentityError::Random(e.to_string()))?;
        Ok(Self::from_secret(bytes))
    }

    /// The key pair stored as base64url. `public_key` may be empty, and is
    /// otherwise checked against the private key.
    ///
    /// # Errors
    ///
    /// [`IdentityError::BadPrivateKey`] or [`IdentityError::KeyMismatch`].
    pub fn from_stored(private_key: &str, public_key: &str) -> Result<Self, IdentityError> {
        let bytes: [u8; 32] = URL_SAFE_NO_PAD
            .decode(private_key)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or(IdentityError::BadPrivateKey)?;
        let identity = Self::from_secret(bytes);
        if !public_key.is_empty() && public_key != identity.public_key() {
            return Err(IdentityError::KeyMismatch);
        }
        Ok(identity)
    }

    fn from_secret(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    /// The public key, base64url without padding.
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.public.as_bytes())
    }

    /// The private key, base64url without padding, for `secrets.json` only.
    #[must_use]
    pub fn private_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.secret.as_bytes())
    }

    /// The node id.
    #[must_use]
    pub fn node_id(&self) -> String {
        node_id(self.public.as_bytes())
    }
}

/// The node id of a public key: the lowercase base32 (RFC 4648, no padding)
/// of the first 10 bytes of its BLAKE2s-256 hash, 16 characters.
#[must_use]
pub fn node_id(public_key: &[u8; 32]) -> String {
    let hash = Blake2s256::digest(public_key);
    base32(&hash[..ID_BYTES])
}

/// The fingerprint shown to users: the node id in groups of four, joined by
/// hyphens.
#[must_use]
pub fn fingerprint(node_id: &str) -> String {
    let chars: Vec<char> = node_id.chars().collect();
    chars
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_follows_rfc_4648() {
        // RFC 4648 section 10, lower-cased and without padding
        let cases = [
            ("", ""),
            ("f", "my"),
            ("fo", "mzxq"),
            ("foo", "mzxw6"),
            ("foob", "mzxw6yq"),
            ("fooba", "mzxw6ytb"),
            ("foobar", "mzxw6ytboi"),
        ];
        for (input, output) in cases {
            assert_eq!(base32(input.as_bytes()), output);
        }
    }

    #[test]
    fn node_id_is_16_characters_of_the_key_hash() {
        // hashlib.blake2s(bytes(range(1, 33))).digest()[:10], base32 lower-cased
        let key: [u8; 32] = std::array::from_fn(|i| u8::try_from(i + 1).unwrap());
        let id = node_id(&key);
        assert_eq!(id.len(), 16);
        assert_eq!(id, KNOWN_ID);
        assert!(id.chars().all(|c| BASE32.contains(&(c as u8))));
    }

    const KNOWN_ID: &str = "oknigd64uvdeg5jb";

    #[test]
    fn fingerprint_groups_by_four() {
        assert_eq!(fingerprint("k3xq2mfaq7tc4pzr"), "k3xq-2mfa-q7tc-4pzr");
    }

    #[test]
    fn keys_round_trip_through_storage() {
        let identity = Identity::generate().unwrap();
        let stored =
            Identity::from_stored(&identity.private_key(), &identity.public_key()).unwrap();
        assert_eq!(stored.public_key(), identity.public_key());
        assert_eq!(stored.node_id(), identity.node_id());
        assert_eq!(identity.public_key().len(), 43);
    }

    #[test]
    fn a_missing_public_key_is_derived() {
        let identity = Identity::generate().unwrap();
        let stored = Identity::from_stored(&identity.private_key(), "").unwrap();
        assert_eq!(stored.public_key(), identity.public_key());
    }

    #[test]
    fn bad_keys_are_refused() {
        let other = Identity::generate().unwrap();
        let identity = Identity::generate().unwrap();
        assert_eq!(
            Identity::from_stored("not base64!", "").err(),
            Some(IdentityError::BadPrivateKey)
        );
        assert_eq!(
            Identity::from_stored("AAAA", "").err(),
            Some(IdentityError::BadPrivateKey)
        );
        assert_eq!(
            Identity::from_stored(&identity.private_key(), &other.public_key()).err(),
            Some(IdentityError::KeyMismatch)
        );
    }
}
