//! Who a bridge is, and whether to believe it (`docs/integrations/hue.md`, TLS; ADR-0013 D3).
//!
//! A bridge's certificate chains to a Signify root, and its common name is the bridge id. The
//! bridge is reached by IP address, so the name check is the id, not the host. A bridge whose
//! certificate doesn't chain to a Signify root (older firmware, self-signed) is trusted only
//! after a person confirmed its fingerprint (trust on first use): the fingerprint is then pinned.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::stack::Stack;
use openssl::x509::store::X509StoreBuilder;
use openssl::x509::{X509, X509StoreContext};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms, ring};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme};

/// The Signify roots (`root-bridge` and `Hue Root CA 01`), as PEM.
const SIGNIFY_ROOTS_PEM: &str = include_str!("signify_roots.pem");

/// Starts the message of a verifier error that says the bridge wasn't believed, so the REST
/// client can tell it from a bridge that doesn't answer.
pub(crate) const UNTRUSTED_MARKER: &str = "untrusted bridge: ";

/// A SHA-256 fingerprint of a certificate's DER encoding.
pub type Fingerprint = [u8; 32];

/// Writes a fingerprint as lowercase hex, which is how it is stored and shown.
#[must_use]
pub fn fingerprint_hex(fingerprint: &Fingerprint) -> String {
    hex::encode(fingerprint)
}

/// Reads a fingerprint written by [`fingerprint_hex`]. Colons and case don't matter.
#[must_use]
pub fn parse_fingerprint(text: &str) -> Option<Fingerprint> {
    let digits: String = text.chars().filter(|c| *c != ':').collect();
    hex::decode(digits).ok()?.try_into().ok()
}

/// The certificate authorities a bridge's certificate may chain to.
#[derive(Clone, Debug)]
pub enum Anchors {
    /// The bundled Signify roots.
    Signify,
    /// These certificates, DER-encoded: a test double's authority.
    Custom(Vec<Vec<u8>>),
}

impl Anchors {
    fn certificates(&self) -> Result<Vec<X509>, TrustError> {
        match self {
            Self::Signify => X509::stack_from_pem(SIGNIFY_ROOTS_PEM.as_bytes())
                .map_err(|e| TrustError::Setup(e.to_string())),
            Self::Custom(ders) => ders
                .iter()
                .map(|der| X509::from_der(der).map_err(|e| TrustError::Setup(e.to_string())))
                .collect(),
        }
    }
}

/// What a bridge has to be, to be believed.
#[derive(Clone, Debug)]
pub struct BridgeTrust {
    /// The id the certificate's common name must carry, compared without regard to case.
    pub bridge_id: String,
    pub anchors: Anchors,
    /// A certificate a person confirmed: trusted whatever its chain, but the common name still
    /// has to match.
    pub pinned: Option<Fingerprint>,
}

impl BridgeTrust {
    /// Signify's roots, and no pin.
    #[must_use]
    pub fn signify(bridge_id: impl Into<String>) -> Self {
        Self {
            bridge_id: bridge_id.into(),
            anchors: Anchors::Signify,
            pinned: None,
        }
    }

    /// Whether `der` is the bridge.
    ///
    /// # Errors
    /// [`TrustError`] saying which check failed.
    pub fn check(&self, der: &[u8], intermediates: &[Vec<u8>]) -> Result<(), TrustError> {
        let cert = X509::from_der(der).map_err(|e| TrustError::Malformed(e.to_string()))?;
        let found = common_name(&cert).unwrap_or_default();
        if !found.eq_ignore_ascii_case(&self.bridge_id) {
            return Err(TrustError::NameMismatch {
                expected: self.bridge_id.clone(),
                found,
            });
        }
        if self.pinned == Some(fingerprint(&cert)) {
            return Ok(());
        }
        // A bridge whose firmware moved it to a Signify chain is believed again by the chain
        match (self.verify_chain(&cert, intermediates), self.pinned) {
            (Err(_), Some(_)) => Err(TrustError::PinMismatch),
            (result, _) => result,
        }
    }

    fn verify_chain(&self, cert: &X509, intermediates: &[Vec<u8>]) -> Result<(), TrustError> {
        let mut store = X509StoreBuilder::new().map_err(|e| TrustError::Setup(e.to_string()))?;
        for root in self.anchors.certificates()? {
            store
                .add_cert(root)
                .map_err(|e| TrustError::Setup(e.to_string()))?;
        }
        let store = store.build();
        let mut chain = Stack::new().map_err(|e| TrustError::Setup(e.to_string()))?;
        for der in intermediates {
            let intermediate =
                X509::from_der(der).map_err(|e| TrustError::Malformed(e.to_string()))?;
            chain
                .push(intermediate)
                .map_err(|e| TrustError::Setup(e.to_string()))?;
        }
        let mut context = X509StoreContext::new().map_err(|e| TrustError::Setup(e.to_string()))?;
        let valid = context
            .init(&store, cert, &chain, |c| {
                c.verify_cert()
                    .map(|ok| ok.then_some(()).ok_or_else(|| c.error()))
            })
            .map_err(|e| TrustError::Setup(e.to_string()))?;
        valid.map_err(|e| TrustError::Chain(e.to_string()))
    }
}

/// Why a certificate wasn't believed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustError {
    /// The certificate can't be read.
    Malformed(String),
    /// The common name isn't the bridge id: a different bridge, or something in between.
    NameMismatch { expected: String, found: String },
    /// The certificate doesn't chain to an authority we trust.
    Chain(String),
    /// A pinned certificate was expected and this is another.
    PinMismatch,
    /// The checks couldn't be set up.
    Setup(String),
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "the bridge's certificate can't be read: {e}"),
            Self::NameMismatch { expected, found } => write!(
                f,
                "the certificate is for {found:?}, not for the bridge {expected:?}"
            ),
            Self::Chain(e) => write!(f, "the certificate doesn't chain to a Signify root: {e}"),
            Self::PinMismatch => f.write_str("the certificate is not the one that was confirmed"),
            Self::Setup(e) => write!(f, "setting up the certificate checks failed: {e}"),
        }
    }
}

impl std::error::Error for TrustError {}

fn common_name(cert: &X509) -> Option<String> {
    let entry = cert.subject_name().entries_by_nid(Nid::COMMONNAME).next()?;
    entry.data().to_string().ok()
}

fn fingerprint(cert: &X509) -> Fingerprint {
    let mut out = [0u8; 32];
    if let Ok(digest) = cert.digest(MessageDigest::sha256()) {
        out.copy_from_slice(&digest);
    }
    out
}

/// The fingerprint of a DER certificate.
///
/// # Errors
/// When `der` isn't a certificate.
pub fn fingerprint_of(der: &[u8]) -> Result<Fingerprint, TrustError> {
    let cert = X509::from_der(der).map_err(|e| TrustError::Malformed(e.to_string()))?;
    Ok(fingerprint(&cert))
}

/// What a certificate says of itself: shown to a person who decides whether to trust a bridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CertificateSummary {
    pub fingerprint: Fingerprint,
    pub subject: String,
    pub issuer: String,
}

/// Summarises a DER certificate.
///
/// # Errors
/// When `der` isn't a certificate.
pub fn summarize(der: &[u8]) -> Result<CertificateSummary, TrustError> {
    let cert = X509::from_der(der).map_err(|e| TrustError::Malformed(e.to_string()))?;
    Ok(CertificateSummary {
        fingerprint: fingerprint(&cert),
        subject: common_name(&cert).unwrap_or_default(),
        issuer: cert
            .issuer_name()
            .entries_by_nid(Nid::COMMONNAME)
            .next()
            .and_then(|e| e.data().to_string().ok())
            .unwrap_or_default(),
    })
}

/// The certificate a probing connection saw.
/// The certificate a probing connection saw, then the certificates the bridge sent with it.
pub type SeenCertificate = Arc<Mutex<Option<Vec<Vec<u8>>>>>;

enum Policy {
    /// The bridge checks above.
    Bridge(BridgeTrust),
    /// Believe nothing, and remember the certificate: for the one request that asks a bridge who
    /// it is, so a person can be shown its fingerprint. Nothing secret is sent on it.
    Capture(SeenCertificate),
}

struct BridgeVerifier {
    policy: Policy,
    algorithms: WebPkiSupportedAlgorithms,
}

impl fmt::Debug for BridgeVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BridgeVerifier")
    }
}

impl ServerCertVerifier for BridgeVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, Error> {
        match &self.policy {
            Policy::Bridge(trust) => {
                let others: Vec<Vec<u8>> = intermediates.iter().map(|c| c.to_vec()).collect();
                trust
                    .check(end_entity, &others)
                    .map_err(|e| Error::General(format!("{UNTRUSTED_MARKER}{e}")))?;
            }
            Policy::Capture(seen) => {
                let mut chain = vec![end_entity.to_vec()];
                chain.extend(intermediates.iter().map(|c| c.to_vec()));
                *seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(chain);
            }
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn config_with(verifier: BridgeVerifier) -> Result<ClientConfig, TrustError> {
    ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| TrustError::Setup(e.to_string()))
        .map(|builder| {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(verifier))
                .with_no_client_auth()
        })
}

fn verifier(policy: Policy) -> BridgeVerifier {
    BridgeVerifier {
        policy,
        algorithms: ring::default_provider().signature_verification_algorithms,
    }
}

/// The TLS settings of a connection that must reach this bridge.
///
/// # Errors
/// [`TrustError::Setup`] when rustls can't be set up.
pub fn bridge_config(trust: BridgeTrust) -> Result<ClientConfig, TrustError> {
    config_with(verifier(Policy::Bridge(trust)))
}

/// The TLS settings of a connection that only looks at the bridge's certificate.
///
/// # Errors
/// [`TrustError::Setup`] when rustls can't be set up.
pub fn capture_config(seen: SeenCertificate) -> Result<ClientConfig, TrustError> {
    config_with(verifier(Policy::Capture(seen)))
}

/// The TLS settings for a public server (the discovery service): the system's roots, and
/// `extra_roots` (DER) on top.
///
/// # Errors
/// [`TrustError::Setup`] when rustls can't be set up or a root can't be used.
pub fn public_config(extra_roots: &[Vec<u8>]) -> Result<ClientConfig, TrustError> {
    let mut roots = RootCertStore::empty();
    for native in rustls_native_certs::load_native_certs().certs {
        // A system certificate that rustls can't use is of no use to us either
        let _ = roots.add(native);
    }
    for der in extra_roots {
        roots
            .add(CertificateDer::from(der.clone()))
            .map_err(|e| TrustError::Setup(e.to_string()))?;
    }
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| TrustError::Setup(e.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// Where the certificates a person confirmed are kept. A node stores them with the instance;
/// [`MemoryPins`] keeps them for as long as the process lives.
pub trait PinStore: Send + Sync {
    /// The fingerprint confirmed for this bridge id (lowercase).
    fn pinned(&self, bridge_id: &str) -> Option<Fingerprint>;
    /// Remembers a confirmation.
    fn pin(&self, bridge_id: &str, fingerprint: Fingerprint);
    /// Forgets it.
    fn unpin(&self, bridge_id: &str);
}

/// Pins kept in memory.
#[derive(Default)]
pub struct MemoryPins {
    pins: Mutex<std::collections::HashMap<String, Fingerprint>>,
}

impl PinStore for MemoryPins {
    fn pinned(&self, bridge_id: &str) -> Option<Fingerprint> {
        let pins = self.pins.lock().unwrap_or_else(PoisonError::into_inner);
        pins.get(&bridge_id.to_ascii_lowercase()).copied()
    }

    fn pin(&self, bridge_id: &str, fingerprint: Fingerprint) {
        let mut pins = self.pins.lock().unwrap_or_else(PoisonError::into_inner);
        pins.insert(bridge_id.to_ascii_lowercase(), fingerprint);
    }

    fn unpin(&self, bridge_id: &str) {
        let mut pins = self.pins.lock().unwrap_or_else(PoisonError::into_inner);
        pins.remove(&bridge_id.to_ascii_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingerprints Signify's roots had when they were bundled, from `openssl x509`.
    #[test]
    fn the_bundled_roots_are_the_signify_ones() {
        let roots = X509::stack_from_pem(SIGNIFY_ROOTS_PEM.as_bytes()).unwrap();
        let found: Vec<(String, String)> = roots
            .iter()
            .map(|r| (common_name(r).unwrap(), fingerprint_hex(&fingerprint(r))))
            .collect();
        assert_eq!(
            found,
            vec![
                (
                    "root-bridge".to_owned(),
                    "f0bd8e6509e82f774d63bc009d5388c969fe3dcf7d6d541d6351b72b898d8acf".to_owned()
                ),
                (
                    "Hue Root CA 01".to_owned(),
                    "d8b89448b2af8e1676185ac07219ee9dcbc8f01c122a026a2a4b7b5cfe0328b8".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn fingerprints_parse_with_or_without_colons() {
        let text = "F0:BD:8E:65:09:E8:2F:77:4D:63:BC:00:9D:53:88:C9:69:FE:3D:CF:7D:6D:54:1D:63:51:B7:2B:89:8D:8A:CF";
        let parsed = parse_fingerprint(text).unwrap();
        assert_eq!(parse_fingerprint(&fingerprint_hex(&parsed)), Some(parsed));
        assert_eq!(parse_fingerprint("abcd"), None);
    }
}
