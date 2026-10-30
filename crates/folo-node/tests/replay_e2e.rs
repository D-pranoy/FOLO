//! The replay end to end (RB-14 step 9, ADR-0037 D22 and D23): the node, the
//! real `folo-capture` agent on the replay backend and the real effect
//! engine. A zone with a canvas surface and an enabled Screen layer makes
//! the node start the agent; the replayed frames travel agent to node to
//! engine, and the canvas the engine publishes on `surface:wall` is checked
//! against what the Screen effect should draw, cell for cell. The marker
//! pixels in the held, protected, bar and margin areas of those frames
//! must be in no log, status, answer, live message or file of the node.
//!
//! The end-to-end test runs when `FOLO_E2E=1` with `folo-source` on the
//! `PATH` (CI's Rust jobs set both up with uv) and `folo-capture` built beside
//! the test binaries (`cargo test` builds it); otherwise it says so and
//! passes. The fixture is synthetic and committed; the test beside it checks
//! that it is what the replay writer builds (`FOLO_WRITE_FIXTURES=1`
//! rewrites it).

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use folo_capture_core::replay;
use folo_capture_core::types::{Bars, FrameFlags, Picture, ScreenFrame};
use folo_node::cli::{LocationArgs, RunArgs};
use folo_node::client;
use folo_supervisor::capture::CaptureCommand;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;

// ------------------------------------------------------------------ the fixture

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/screen-replay.folr"
);
const WIDTH: u16 = 32;
const HEIGHT: u16 = 18;
/// The black bars of every frame, in rows. They hold [`BAR_MARKER`].
const BARS: u16 = 2;
/// The Screen layer's left margin, in cells of the picture: 4 of 32.
const MARGIN_LEFT: u16 = 4;
const MARGIN_SHARE: f64 = 0.125;
/// What is left is 28 × 14 cells, two per canvas cell each way: the canvas
/// is 14 × 7 and each of its cells is exactly the mean of a 2 × 2 block.
const CANVAS_WIDTH: usize = 14;
const CANVAS_HEIGHT: usize = 7;
const GAP_US: u64 = 100_000;

const HELD_MARKER: [u8; 3] = [0xA1, 0x11, 0x91];
const PROTECTED_MARKER: [u8; 3] = [0xB2, 0x22, 0x82];
const BAR_MARKER: [u8; 3] = [0xC3, 0x33, 0x73];
const MARGIN_MARKER: [u8; 3] = [0xD4, 0x44, 0x64];

/// Which normal frame: the colours of a canvas cell are a gradient of its place.
#[derive(Clone, Copy)]
enum Look {
    Morning,
    Evening,
}

fn cell_colour(look: Look, column: usize, row: usize) -> [u8; 3] {
    // The column and row are below 14 and 7, so these stay below 256
    let (column, row) = (u8::try_from(column).unwrap(), u8::try_from(row).unwrap());
    match look {
        Look::Morning => [10 + 15 * column, 10 + 30 * row, 40],
        Look::Evening => [240 - 15 * column, 240 - 30 * row, 200],
    }
}

/// A frame of the replay: normal ones hold a look's gradient in 2 × 2 blocks
/// between the bars and right of the margin, `markers` everywhere else.
fn frame(sequence: u32, look: Option<Look>, flags: FrameFlags, filler: [u8; 3]) -> ScreenFrame {
    let (width, height) = (usize::from(WIDTH), usize::from(HEIGHT));
    let mut pixels = filler.repeat(width * height);
    if let Some(look) = look {
        for y in 0..height {
            for x in 0..width {
                let (top, bottom) = (usize::from(BARS), height - usize::from(BARS));
                let at = (y * width + x) * 3;
                let colour = if y < top || y >= bottom {
                    BAR_MARKER
                } else if x < usize::from(MARGIN_LEFT) {
                    MARGIN_MARKER
                } else {
                    cell_colour(
                        look,
                        (x - usize::from(MARGIN_LEFT)) / 2,
                        (y - usize::from(BARS)) / 2,
                    )
                };
                pixels[at..at + 3].copy_from_slice(&colour);
            }
        }
    }
    ScreenFrame {
        sequence,
        timestamp_us: u64::from(sequence) * GAP_US,
        picture: Picture {
            screen_index: 0,
            width: WIDTH,
            height: HEIGHT,
            flags,
            bars: Bars {
                top: BARS,
                bottom: BARS,
                left: 0,
                right: 0,
            },
            pixels,
        },
    }
}

/// A normal frame, a held one, a protected one and another normal one: the
/// effect must show the two normal ones alone.
fn frames() -> Vec<ScreenFrame> {
    let flags = FrameFlags::default();
    vec![
        frame(0, Some(Look::Morning), flags, [0; 3]),
        frame(
            1,
            None,
            FrameFlags {
                held: true,
                ..flags
            },
            HELD_MARKER,
        ),
        frame(
            2,
            None,
            FrameFlags {
                protected: true,
                ..flags
            },
            PROTECTED_MARKER,
        ),
        frame(3, Some(Look::Evening), flags, [0; 3]),
    ]
}

/// The canvas the Screen effect should draw for a normal frame: the mean of
/// each 2 × 2 block of what the bars and the margin leave.
fn expected_canvas(frame: &ScreenFrame) -> Vec<u8> {
    let picture = &frame.picture;
    let width = usize::from(picture.width);
    let (first_row, first_column) = (usize::from(picture.bars.top), usize::from(MARGIN_LEFT));
    let mut canvas = Vec::new();
    for row in 0..CANVAS_HEIGHT {
        for column in 0..CANVAS_WIDTH {
            for channel in 0..3 {
                let mut sum = 0u32;
                for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let (y, x) = (first_row + 2 * row + dy, first_column + 2 * column + dx);
                    sum += u32::from(picture.pixels[(y * width + x) * 3 + channel]);
                }
                assert_eq!(sum % 4, 0, "the mean of a block is whole");
                canvas.push(u8::try_from(sum / 4).unwrap());
            }
        }
    }
    canvas
}

#[test]
fn the_fixture_is_what_the_replay_writer_builds() {
    let built = replay::write(&frames()).unwrap();
    if std::env::var("FOLO_WRITE_FIXTURES").as_deref() == Ok("1") {
        std::fs::write(FIXTURE, &built).unwrap();
    }
    let committed = std::fs::read(FIXTURE).expect("tests/fixtures/screen-replay.folr");
    assert!(
        committed == built,
        "the fixture is out of date: FOLO_WRITE_FIXTURES=1 cargo test -p folo-node --test replay_e2e"
    );
    assert_eq!(replay::parse(&committed).unwrap(), frames());
}

// ------------------------------------------------------------------ the run

/// Removes the node's home when the test ends, however it ends.
struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn every_file(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            every_file(&path, out);
        } else {
            // An unreadable file would be a vacuous pass, so it fails the test
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push((path, bytes));
        }
    }
}

/// `folo-capture` beside the test binaries (`target/<profile>/`).
fn agent_program() -> PathBuf {
    let name = if cfg!(windows) {
        "folo-capture.exe"
    } else {
        "folo-capture"
    };
    let exe = std::env::current_exe().unwrap();
    let beside = exe.parent().and_then(Path::parent).unwrap().join(name);
    // `cargo test -p folo-node` doesn't build another package's binary, and an
    // old one in target/ must not be used
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let built = std::process::Command::new(cargo)
        .args(["build", "--locked", "-p", "folo-capture"])
        .status()
        .expect("cargo runs");
    assert!(built.success(), "cargo build -p folo-capture failed");
    assert!(
        beside.is_file(),
        "{} isn't built: cargo build -p folo-capture",
        beside.display()
    );
    beside
}

fn capture_command() -> CaptureCommand {
    let backend = format!("replay:{FIXTURE}");
    let args = |last: &[&str]| -> Vec<std::ffi::OsString> {
        ["--backend", backend.as_str()]
            .into_iter()
            .chain(last.iter().copied())
            .map(Into::into)
            .collect()
    };
    CaptureCommand {
        program: agent_program(),
        run_args: args(&["run"]),
        list_program: agent_program(),
        list_args: args(&["screens", "--json"]),
        scope_launcher: None,
    }
}

async fn call(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, String) {
    let (method, path) = (method.to_owned(), path.to_owned());
    let body = body.map(|b| b.to_string());
    tokio::task::spawn_blocking(move || {
        client::request(port, &method, &path, body.as_deref()).unwrap()
    })
    .await
    .unwrap()
}

async fn status(port: u16) -> Option<Value> {
    let (code, text) = tokio::task::spawn_blocking(move || {
        client::request(port, "GET", "/api/v1/system/status", None)
    })
    .await
    .unwrap()
    .ok()?;
    (code == 200).then(|| serde_json::from_str(&text).ok())?
}

async fn wait_for_status(port: u16, what: &str, wanted: impl Fn(&Value) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = status(port).await
            && wanted(&status)
        {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn subscribe(port: u16, topics: &[&str]) -> Socket {
    let url = format!("ws://127.0.0.1:{port}/api/v1/live");
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let topics = json!({"subscribe": topics});
    socket
        .send(Message::Text(topics.to_string().into()))
        .await
        .unwrap();
    socket
}

/// What a live connection was sent.
#[derive(Default)]
struct Received {
    texts: Vec<String>,
    binary: Vec<Vec<u8>>,
}

/// Reads until told to stop.
async fn read_until(mut socket: Socket, stop: oneshot::Receiver<()>) -> Received {
    let mut received = Received::default();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => return received,
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => received.texts.push(text.as_str().to_owned()),
                Some(Ok(Message::Binary(bytes))) => received.binary.push(bytes.to_vec()),
                Some(Ok(_)) => {}
                _ => return received,
            },
        }
    }
}

/// The payload of a published canvas frame (`FOLS`, rgb8, uncompressed).
fn canvas_payload(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(b"FOLS") {
        return None;
    }
    assert_eq!(bytes[5], 1, "rgb8");
    assert_eq!(bytes[6] & 1, 0, "not LZ4-compressed");
    let width = usize::from(u16::from_le_bytes([bytes[10], bytes[11]]));
    let height = usize::from(u16::from_le_bytes([bytes[12], bytes[13]]));
    assert_eq!((width, height), (CANVAS_WIDTH, CANVAS_HEIGHT));
    assert_eq!(bytes.len(), 28 + width * height * 3);
    Some(&bytes[28..])
}

/// Makes capture wanted: a canvas surface `wall` with a Screen layer (no
/// smoothing, a left margin), the local source rendering it, and the replay's
/// screen enabled.
async fn want_capture(port: u16) {
    let zone = json!({"name": "Wall", "surfaces": [
        {"id": "wall", "kind": "canvas", "resolution": {"width": CANVAS_WIDTH, "height": CANVAS_HEIGHT}, "role": "canvas.room"},
    ]});
    let (code, text) = call(port, "POST", "/api/v1/zones", Some(zone)).await;
    assert_eq!(code, 201, "{text}");
    let zone: Value = serde_json::from_str(&text).unwrap();
    let layer = json!({
        "id": "screen1",
        "effect": {"id": "screen"},
        "params": {"smoothing": 0, "margins": {"left": MARGIN_SHARE}},
    });
    let show = json!({
        "active": {zone["id"].as_str().unwrap(): "s1"},
        "scenes": [{"id": "s1", "name": "Screen", "layers_by_role": {"canvas.room": [layer]}}],
    });
    assert_eq!(call(port, "PUT", "/api/v1/show", Some(show)).await.0, 200);
    let source = json!({"surfaces": ["wall"], "inputs": {"screens": [
        {"id": replay::SCREEN_ID, "enabled": true},
    ]}});
    let (code, text) = call(port, "PUT", "/api/v1/sources/local", Some(source)).await;
    assert_eq!(code, 200, "{text}");
}

/// Reads the canvas frames: black until the first picture, then the two normal
/// frames' canvases in turn, never the held or protected ones'. Returns everything
/// the socket was sent.
async fn read_canvas(canvas: &mut Socket, normal: &[Vec<u8>]) -> Received {
    let mut frames_seen: Vec<Vec<u8>> = Vec::new();
    let mut texts = Vec::new();
    let mut binary = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut shown = (false, false);
    while !(shown.0 && shown.1 && frames_seen.len() >= 40) {
        assert!(
            Instant::now() < deadline,
            "the canvases seen: {} frames, morning {}, evening {}",
            frames_seen.len(),
            shown.0,
            shown.1
        );
        let message = tokio::time::timeout(Duration::from_secs(20), canvas.next())
            .await
            .expect("canvas frames in time")
            .unwrap()
            .unwrap();
        match message {
            Message::Binary(bytes) => {
                binary.push(bytes.to_vec());
                let Some(payload) = canvas_payload(&bytes) else {
                    continue;
                };
                let started = shown.0 || shown.1;
                if payload == normal[0] {
                    shown.0 = true;
                } else if payload == normal[1] {
                    shown.1 = true;
                } else {
                    assert!(
                        !started && payload.iter().all(|&b| b == 0),
                        "a canvas that is neither black before the first picture nor a normal frame's: {payload:?}"
                    );
                }
                if started {
                    frames_seen.push(payload.to_vec());
                }
            }
            Message::Text(text) => texts.push(text.as_str().to_owned()),
            _ => {}
        }
    }
    assert!(
        frames_seen
            .iter()
            .all(|f| *f == normal[0] || *f == normal[1]),
        "once drawn the canvas is always a normal frame's"
    );

    Received { texts, binary }
}

/// Raw bytes the effect can't copy to a canvas: every marker, the whole payload of the
/// marker-only frames, a row of each frame's pixels and the markers as hex
/// and as a printed list.
fn forbidden_bytes() -> Vec<(String, Vec<u8>)> {
    let mut forbidden: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, marker, cells) in [
        ("held", HELD_MARKER, 32),
        ("protected", PROTECTED_MARKER, 32),
        ("bar", BAR_MARKER, 32),
        ("margin", MARGIN_MARKER, usize::from(MARGIN_LEFT)),
    ] {
        forbidden.push((format!("{name} marker"), marker.repeat(cells)));
        // A leak written as text: hex, or a printed list of the bytes
        let hex: String = marker.map(|b| format!("{b:02x}")).join("");
        forbidden.push((format!("{name} marker in hex"), hex.into_bytes()));
        let list = marker.map(|b| b.to_string()).join(", ");
        forbidden.push((format!("{name} marker as a list"), list.into_bytes()));
    }
    // The whole payload of the frames that are only markers
    for frame in &frames()[1..3] {
        forbidden.push((
            "a marker frame's payload".to_owned(),
            frame.picture.pixels.clone(),
        ));
    }
    for (index, frame) in frames().iter().enumerate() {
        let row = &frame.picture.pixels[usize::from(BARS) * 96..(usize::from(BARS) + 1) * 96];
        forbidden.push((format!("a row of frame {index}"), row.to_vec()));
    }
    forbidden.push(("a frame header".to_owned(), b"FOLC".to_vec()));
    forbidden
}

/// What the client API answers to every read.
async fn api_answers(port: u16) -> Vec<(String, Vec<u8>)> {
    let mut seen = Vec::new();
    for path in [
        "/api/v1/health",
        "/api/v1/info",
        "/api/v1/state",
        "/api/v1/system",
        "/api/v1/system/status",
        "/api/v1/system/logs?lines=10000",
        "/api/v1/zones",
        "/api/v1/show",
        "/api/v1/sources",
        "/api/v1/sources/local",
        "/api/v1/sources/local/inputs",
        "/api/v1/sources/local/media",
        "/api/v1/effects",
        "/api/v1/providers",
    ] {
        seen.push((
            path.to_owned(),
            call(port, "GET", path, None).await.1.into_bytes(),
        ));
    }
    seen
}

/// The status messages said the screen was capturing and that protected
/// content was on it (the engine's newest-wins slot may skip any one frame,
/// so this is the proof the protected frames were sent).
fn assert_screen_reported(texts: &[String]) {
    let screens: Vec<Value> = texts
        .iter()
        .map(|text| serde_json::from_str::<Value>(text).unwrap())
        .filter(|m| m["type"] == "status")
        .map(|m| m["data"]["sources"]["local"]["screen"].clone())
        .collect();
    assert!(
        screens.iter().any(|s| s["state"] == "capturing"),
        "no status message said the screen was capturing"
    );
    assert!(
        screens.iter().any(|s| s["protected"] == true),
        "no status message reported protected content"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_canvas_the_engine_publishes_is_what_the_replayed_frames_draw() {
    if std::env::var("FOLO_E2E").as_deref() != Ok("1") {
        eprintln!("skipped: set FOLO_E2E=1 with folo-source on the PATH");
        return;
    }
    let normal: Vec<Vec<u8>> = frames()
        .iter()
        .filter(|f| f.picture.flags == FrameFlags::default())
        .map(expected_canvas)
        .collect();
    assert_eq!(normal.len(), 2);
    assert_ne!(normal[0], normal[1]);

    let home = std::env::temp_dir().join(format!("folo-replay-e2e-{}", folo_store::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let _cleanup = RemoveOnDrop(home.clone());
    let port = StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let location = LocationArgs {
        home: Some(home.clone()),
        service: false,
    };
    let args = RunArgs {
        port: Some(port.to_string()),
        ..RunArgs::default()
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let node = tokio::spawn(async move {
        folo_node::node::run_with_workers(&location, &args, None, Some(capture_command()), async {
            let _ = stopped.await;
        })
        .await
    });
    wait_for_status(port, "the engine to link", |s| {
        s["workers"]["folo-source"]["state"] == "running"
    })
    .await;

    // Two connections: the canvas, and everything else the node can show
    let mut canvas = subscribe(port, &["surface:wall"]).await;
    let (stop_others, others_stop) = oneshot::channel();
    let others = tokio::spawn(read_until(
        subscribe(
            port,
            &[
                "status",
                "input:local",
                "beat:local",
                "media:local",
                "spectrum:local",
                "levels:wall",
            ],
        )
        .await,
        others_stop,
    ));
    want_capture(port).await;

    let canvas_received = read_canvas(&mut canvas, &normal).await;

    // The agent is capturing, and the status says so without pixels
    let node_status = status(port).await.unwrap();
    assert_eq!(node_status["workers"]["folo-capture"]["state"], "running");

    let mut seen = api_answers(port).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let _ = stop_others.send(());
    let others = others.await.unwrap();
    assert_screen_reported(&others.texts);
    for (index, text) in others.texts.iter().enumerate() {
        seen.push((format!("live message {index}"), text.clone().into_bytes()));
    }
    for (index, bytes) in others.binary.iter().enumerate() {
        seen.push((format!("live frame {index}"), bytes.clone()));
    }

    let _ = stop.send(());
    let code = tokio::time::timeout(Duration::from_secs(30), node)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(code, 0);

    // The canvas connection carried only the canvas: none of the markers
    for (index, text) in canvas_received.texts.iter().enumerate() {
        seen.push((format!("canvas message {index}"), text.clone().into_bytes()));
    }
    for (index, bytes) in canvas_received.binary.iter().enumerate() {
        seen.push((format!("canvas frame {index}"), bytes.clone()));
    }
    let mut files = Vec::new();
    every_file(&home, &mut files);
    assert!(files.iter().any(|(p, _)| p.ends_with("folo.log")));
    for (path, bytes) in files {
        seen.push((path.display().to_string(), bytes));
    }

    let forbidden = forbidden_bytes();
    for (what, bytes) in &seen {
        for (name, needle) in &forbidden {
            assert!(!contains(bytes, needle), "{name} in {what}");
        }
    }
}
