//! The run book's reference endpoint: `openssl s_server -dtls1_2 -nocert -psk ... -cipher
//! PSK-AES128-GCM-SHA256`. The client must shake hands with an OpenSSL command line server and
//! its datagrams must arrive as the encoder made them.
//!
//! The test is skipped on Windows and where there is no `openssl` command. CI's Ubuntu job sets
//! `FOLO_OPENSSL_TESTS=1`, which turns a missing command into a failure.

mod support;

use std::io::Read;
use std::net::{SocketAddr, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use folo_provider_hue::dtls::{DtlsSession, DtlsSettings};
use folo_provider_hue::encoder::Encoder;

const IDENTITY: &str = "b8e6c5d0-0f6a-4f8e-9a52-7c1d3a4e5f60";
const KEY_HEX: &str = "00112233445566778899aabbccddeeff";
const AREA: &str = "11111111-2222-3333-4444-555555555555";

struct Server {
    child: Child,
    output: Arc<Mutex<Vec<u8>>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn openssl_works() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(port: u16) -> Server {
    let mut child = Command::new("openssl")
        .args([
            "s_server",
            "-dtls1_2",
            "-nocert",
            "-psk",
            KEY_HEX,
            "-psk_identity",
            IDENTITY,
        ])
        .args([
            "-cipher",
            "PSK-AES128-GCM-SHA256",
            "-accept",
            &format!("127.0.0.1:{port}"),
            "-quiet",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut stdout = child.stdout.take().unwrap();
    let sink = Arc::clone(&output);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdout.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&buf[..n]);
        }
    });
    // s_server has no ready signal; its socket is bound when a datagram to it isn't refused
    std::thread::sleep(Duration::from_millis(500));
    Server { child, output }
}

#[test]
fn the_client_shakes_hands_with_openssl_s_server_and_the_payload_arrives_intact() {
    // The run book scopes this endpoint to Linux runners: the `openssl` on a Windows machine is
    // Git's bundled build, and its s_server doesn't answer a DTLS client on the runner
    if cfg!(windows) {
        eprintln!("skipped on Windows");
        return;
    }
    if !openssl_works() {
        assert!(
            std::env::var("FOLO_OPENSSL_TESTS").is_err(),
            "FOLO_OPENSSL_TESTS is set but there is no openssl command"
        );
        eprintln!("skipped: no openssl command (set FOLO_OPENSSL_TESTS=1 to require it)");
        return;
    }
    let port = free_udp_port();
    let server = start(port);
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut settings = DtlsSettings::new(IDENTITY, hex::decode(KEY_HEX).unwrap());
    settings.retransmit_min = Duration::from_millis(700);
    settings.retransmit_max = Duration::from_millis(900);
    let mut session = DtlsSession::connect(address, &settings).unwrap();

    let mut encoder = Encoder::new(AREA).unwrap();
    let packet = encoder
        .encode(&[(0, [255, 0, 128]), (1, [1, 2, 3])])
        .unwrap();
    session.send(&packet).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    // s_server writes what it decrypts to its standard output
    while !contains(&server.output.lock().unwrap(), &packet) {
        assert!(
            Instant::now() < deadline,
            "s_server never printed the packet: {:?}",
            server.output.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    session.close();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
