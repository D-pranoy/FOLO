//! Test certificates: an authority standing in for Signify's, and bridge certificates.

use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::ec::{EcGroup, EcKey};
use openssl::hash::MessageDigest;
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::x509::extension::{BasicConstraints, KeyUsage, SubjectAlternativeName};
use openssl::x509::{X509, X509Builder, X509NameBuilder};

pub struct Authority {
    pub certificate: X509,
    key: PKey<Private>,
}

fn key() -> PKey<Private> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
    PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
}

fn name(common_name: &str) -> openssl::x509::X509Name {
    let mut builder = X509NameBuilder::new().unwrap();
    builder
        .append_entry_by_nid(Nid::COMMONNAME, common_name)
        .unwrap();
    builder.build()
}

fn builder(
    subject: &str,
    issuer: &openssl::x509::X509NameRef,
    public: &PKey<Private>,
    days: u32,
) -> X509Builder {
    let mut b = X509Builder::new().unwrap();
    b.set_version(2).unwrap();
    let mut serial = BigNum::new().unwrap();
    serial.rand(64, MsbOption::MAYBE_ZERO, false).unwrap();
    b.set_serial_number(&serial.to_asn1_integer().unwrap())
        .unwrap();
    b.set_subject_name(&name(subject)).unwrap();
    b.set_issuer_name(issuer).unwrap();
    b.set_pubkey(public).unwrap();
    b.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    b.set_not_after(&Asn1Time::days_from_now(days).unwrap())
        .unwrap();
    b
}

impl Authority {
    pub fn new(common_name: &str) -> Self {
        let key = key();
        let mut b = builder(common_name, &name(common_name), &key, 30);
        b.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        b.append_extension(
            KeyUsage::new()
                .critical()
                .key_cert_sign()
                .digital_signature()
                .build()
                .unwrap(),
        )
        .unwrap();
        b.sign(&key, MessageDigest::sha256()).unwrap();
        Self {
            certificate: b.build(),
            key,
        }
    }

    pub fn der(&self) -> Vec<u8> {
        self.certificate.to_der().unwrap()
    }

    /// A certificate for `common_name`, signed by this authority, and its private key (PKCS#8 DER).
    pub fn issue(&self, common_name: &str) -> (Vec<u8>, Vec<u8>) {
        let leaf = key();
        let mut b = builder(common_name, self.certificate.subject_name(), &leaf, 30);
        // Real bridge certificates have no alternative names; the discovery service's does
        let san = SubjectAlternativeName::new()
            .ip("127.0.0.1")
            .build(&b.x509v3_context(Some(&self.certificate), None))
            .unwrap();
        b.append_extension(san).unwrap();
        b.sign(&self.key, MessageDigest::sha256()).unwrap();
        (
            b.build().to_der().unwrap(),
            leaf.private_key_to_pkcs8().unwrap(),
        )
    }
}

/// A certificate nobody signed.
pub fn self_signed(common_name: &str) -> (Vec<u8>, Vec<u8>) {
    let leaf = key();
    let mut b = builder(common_name, &name(common_name), &leaf, 30);
    b.sign(&leaf, MessageDigest::sha256()).unwrap();
    (
        b.build().to_der().unwrap(),
        leaf.private_key_to_pkcs8().unwrap(),
    )
}
