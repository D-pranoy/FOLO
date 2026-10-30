//! The DTLS transport against an endpoint that decrypts what it is sent.

mod support;

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use folo_provider_hue::dtls::{DtlsError, DtlsSession, DtlsSettings};
use support::dtls_server::DtlsServer;

const IDENTITY: &str = "an-application-id";
const KEY: [u8; 16] = *b"0123456789abcdef";

fn settings(identity: &str, key: &[u8]) -> DtlsSettings {
    let mut settings = DtlsSettings::new(identity, key.to_vec());
    settings.retransmit_min = Duration::from_millis(150);
    settings.retransmit_max = Duration::from_millis(250);
    settings
}

fn address(server: &DtlsServer) -> SocketAddr {
    ([127, 0, 0, 1], server.port).into()
}

#[test]
fn datagrams_arrive_decrypted_and_in_order() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    let mut session = DtlsSession::connect(address(&server), &settings(IDENTITY, &KEY)).unwrap();
    for i in 0..5u8 {
        session.send(&[b'H', b'u', b'e', i]).unwrap();
    }
    server.wait_for("five datagrams", |o| o.datagrams.len() == 5);
    let expected: Vec<Vec<u8>> = (0..5u8).map(|i| vec![b'H', b'u', b'e', i]).collect();
    assert_eq!(server.datagrams(), expected);
    assert_eq!(
        server.observed.lock().unwrap().identities,
        vec![IDENTITY.to_owned()]
    );
}

#[test]
fn a_wrong_key_does_not_shake_hands() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    let mut wrong = settings(IDENTITY, b"fedcba9876543210");
    wrong.attempts = 2;
    let error = DtlsSession::connect(address(&server), &wrong).unwrap_err();
    assert!(matches!(error, DtlsError::Handshake(_)), "{error}");
    assert_eq!(server.datagrams(), Vec::<Vec<u8>>::new());
}

#[test]
fn a_wrong_identity_does_not_shake_hands() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    let mut other = settings("someone-else", &KEY);
    other.attempts = 2;
    assert!(DtlsSession::connect(address(&server), &other).is_err());
    assert!(
        server
            .observed
            .lock()
            .unwrap()
            .identities
            .iter()
            .all(|i| i == "someone-else")
    );
}

#[test]
fn a_handshake_that_gets_no_answer_is_tried_again() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    // The first hello is lost, so the first attempt times out and the second one is answered
    server.drop_first.store(1, Ordering::SeqCst);
    let mut session = DtlsSession::connect(address(&server), &settings(IDENTITY, &KEY)).unwrap();
    session.send(b"after a retransmit").unwrap();
    server.wait_for("the datagram", |o| o.datagrams.len() == 1);
}

#[test]
fn five_silent_attempts_give_up() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    server.drop_first.store(1000, Ordering::SeqCst);
    let mut quick = settings(IDENTITY, &KEY);
    quick.retransmit_min = Duration::from_millis(30);
    quick.retransmit_max = Duration::from_millis(40);
    let started = std::time::Instant::now();
    let error = DtlsSession::connect(address(&server), &quick).unwrap_err();
    assert!(
        matches!(&error, DtlsError::Handshake(m) if m.contains("5 attempts")),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn close_sends_close_notify() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    let session = DtlsSession::connect(address(&server), &settings(IDENTITY, &KEY)).unwrap();
    session.close();
    server.wait_for("close_notify", |o| o.closes == 1);
}

#[test]
fn a_stream_the_bridge_closed_is_noticed_when_sending() {
    let server = DtlsServer::start(IDENTITY, &KEY);
    server.close_after.store(1, Ordering::SeqCst);
    let mut session = DtlsSession::connect(address(&server), &settings(IDENTITY, &KEY)).unwrap();
    // The bridge closes once it has the first datagram, and `send` looks for a close right after
    // writing, so any send from the first on may be the one that notices
    let mut sent = session.send(b"one");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let error = loop {
        match sent {
            Err(e) => break e,
            Ok(()) if std::time::Instant::now() > deadline => panic!("the close was never noticed"),
            Ok(()) => {
                std::thread::sleep(Duration::from_millis(20));
                sent = session.send(b"more");
            }
        }
    };
    assert!(matches!(error, DtlsError::Closed(_)), "{error}");
    assert_eq!(
        server
            .observed
            .lock()
            .unwrap()
            .datagrams
            .first()
            .map(Vec::as_slice),
        Some(&b"one"[..]),
        "the first datagram arrived before the close"
    );
}

#[test]
fn settings_do_not_print_the_key() {
    let shown = format!("{:?}", settings(IDENTITY, &KEY));
    assert!(!shown.contains("0123456789abcdef"));
    assert!(!shown.contains(&format!("{:?}", KEY.to_vec())));
}
