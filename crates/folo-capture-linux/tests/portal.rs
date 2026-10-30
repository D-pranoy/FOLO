//! The Linux backend against a fake ScreenCast portal on a private session
//! bus and a real PipeWire daemon (ADR-0037 D22). A test producer plays the
//! compositor's screen node. They run where `dbus-daemon`, `pipewire` and
//! `wireplumber` are installed; CI sets `FOLO_PIPEWIRE_TESTS=1`, which turns
//! their absence into a failure instead of a skip. Nothing of the user's
//! desktop is touched, and no real screen is captured.
#![cfg(target_os = "linux")]

mod support;

use std::time::{Duration, Instant};

use folo_capture_core::backend::{BackendReport, CaptureBackend};
use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{
    BlackBarMode, BlackBarPreset, BlackBarSettings, CaptureRequest, CaptureState, Picture, Profile,
};
use folo_capture_linux::appid::{APP_ID, transient_scope_name};
use folo_capture_linux::{PortalBackend, PortalSettings, SCREEN_ID};
use support::daemons::Desktop;
use support::fake_portal::{Behavior, FakePortal};
use support::producer::{Image, PRODUCER_VARIABLE, Producer, run_producer_process};

const SCREEN: (u32, u32) = (640, 360);
const WAIT: Duration = Duration::from_secs(20);

/// Not a test: when the test binary runs itself as the producer process, this
/// is its entry (see `support::producer::Producer`).
#[test]
fn producer_process() {
    if let Ok(spec) = std::env::var(PRODUCER_VARIABLE) {
        run_producer_process(&spec);
    }
}

/// The fake desktop, in the order its parts go away: the backend first.
struct Rig {
    backend: PortalBackend,
    producer: Producer,
    portal: FakePortal,
    desktop: Desktop,
}

impl Rig {
    fn new(dialog_delay: Duration, change: impl FnOnce(&mut Behavior)) -> Option<Self> {
        let desktop = Desktop::start()?;
        let producer = Producer::start(
            &desktop.pipewire_socket,
            &Image::quadrants(SCREEN.0, SCREEN.1),
        );
        let mut behavior = Behavior {
            version: 5,
            registry: true,
            registry_refuses: false,
            dialog_time: Duration::from_millis(0),
            valid_token: None,
            decline: false,
            node_id: producer.node_id(),
            stream_size: (SCREEN.0.try_into().unwrap(), SCREEN.1.try_into().unwrap()),
            pipewire_socket: desktop.pipewire_socket.clone(),
        };
        change(&mut behavior);
        let portal = FakePortal::start(&desktop.bus_address, behavior);
        let backend = PortalBackend::with_settings(PortalSettings {
            bus_address: Some(desktop.bus_address.clone()),
            dialog_delay,
            cgroup_file: None,
        });
        Some(Self {
            backend,
            producer,
            portal,
            desktop,
        })
    }

    fn desktop_dir(&self) -> std::path::PathBuf {
        self.desktop.pipewire_socket.parent().unwrap().to_owned()
    }

    fn desktop_address(&self) -> String {
        self.desktop.bus_address.clone()
    }

    fn default() -> Option<Self> {
        Self::new(Duration::from_millis(1500), |_| {})
    }

    /// The next picture satisfying `wanted`, within [`WAIT`].
    fn picture_where(&mut self, wanted: impl Fn(&Picture) -> bool) -> Picture {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.backend.next_picture(Duration::from_millis(200)) {
                Ok(Some(picture)) if wanted(&picture) => return picture,
                Ok(_) => {}
                Err(error) => panic!("the capture failed: {error}"),
            }
            assert!(Instant::now() < deadline, "no picture of that kind came");
        }
    }

    /// The first report in `state`, within [`WAIT`].
    fn report_in(&mut self, state: CaptureState) -> BackendReport {
        let deadline = Instant::now() + WAIT;
        loop {
            let report = self.backend.report().expect("the portal backend reports");
            if report.state == state {
                return report;
            }
            assert!(
                Instant::now() < deadline,
                "never reached {state:?}, last {:?} ({:?})",
                report.state,
                report.message
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn request_for(token: Option<&str>) -> CaptureRequest {
    request((64, 36), token)
}

fn request(box_size: (u16, u16), restore_token: Option<&str>) -> CaptureRequest {
    CaptureRequest {
        screen: SCREEN_ID.to_owned(),
        width: box_size.0,
        height: box_size.1,
        rate_hz: 30.0,
        black_bars: BlackBarSettings {
            preset: BlackBarPreset::Auto,
            mode: BlackBarMode::Default,
            threshold: 0.05,
        },
        profile: Profile::Video,
        restore_token: restore_token.map(str::to_owned),
    }
}

/// The colour of cell `(x, y)` of a picture.
fn cell(picture: &Picture, x: usize, y: usize) -> [u8; 3] {
    let at = (y * usize::from(picture.width) + x) * 3;
    [
        picture.pixels[at],
        picture.pixels[at + 1],
        picture.pixels[at + 2],
    ]
}

fn is_quadrants(picture: &Picture) -> bool {
    let (w, h) = (usize::from(picture.width), usize::from(picture.height));
    cell(picture, w / 4, h / 4) == [255, 0, 0]
        && cell(picture, 3 * w / 4, h / 4) == [0, 255, 0]
        && cell(picture, w / 4, 3 * h / 4) == [0, 0, 255]
        && cell(picture, 3 * w / 4, 3 * h / 4) == [255, 255, 255]
}

#[test]
fn it_captures_the_screen_the_portal_names_and_follows_it() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    // Before a capture the one screen has no size (D15) and no dialog came.
    let before = rig.backend.list_screens().unwrap();
    assert_eq!(
        (
            before.len(),
            before[0].id.as_str(),
            before[0].width,
            before[0].height
        ),
        (1, "portal", 0, 0)
    );
    assert_eq!(rig.backend.name(), "portal");

    rig.backend.start(&request_for(None)).unwrap();
    let picture = rig.picture_where(is_quadrants);
    assert_eq!((picture.width, picture.height), (64, 36));
    assert_eq!(picture.pixels.len(), 64 * 36 * 3);
    assert_eq!(picture.screen_index, 0);

    // What the portal was asked for: a monitor, hidden cursor, persistence
    // until revoked, one source, and no token the first time.
    let log = rig.portal.log();
    assert_eq!(log.selections.len(), 1);
    let selection = &log.selections[0];
    assert_eq!(selection.types, Some(1), "monitor");
    assert_eq!(selection.cursor_mode, Some(1), "hidden");
    assert_eq!(selection.persist_mode, Some(2), "until revoked");
    assert_eq!(selection.multiple, Some(false));
    assert_eq!(selection.restore_token, None);
    assert_eq!(log.remotes_opened, 1);

    // The screen's size is known once the stream runs.
    let after = rig.backend.list_screens().unwrap();
    assert_eq!((after[0].width, after[0].height), SCREEN);

    // The new token comes out once (D14), and the state is capturing.
    let report = rig.report_in(CaptureState::Capturing);
    assert_eq!(report.restore_token.as_deref(), Some("token-1"));
    assert!(rig.backend.report().unwrap().restore_token.is_none());

    // A new picture on the screen reaches the reader.
    rig.producer
        .show(&Image::flat(SCREEN.0, SCREEN.1, [0, 255, 0]));
    let green = rig.picture_where(|picture| cell(picture, 10, 10) == [0, 255, 0]);
    assert!(green.pixels.chunks(3).all(|rgb| rgb == [0, 255, 0]));

    rig.backend.stop();
    assert_eq!(
        rig.portal.log().closed_by_client,
        1,
        "the session is closed"
    );
    assert!(matches!(
        rig.backend.next_picture(Duration::from_millis(10)),
        Err(CaptureError::NotStarted)
    ));
}

#[test]
fn the_box_and_the_frame_size_change_in_place_without_a_new_session() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    rig.backend.start(&request((32, 18), None)).unwrap();
    let small = rig.picture_where(|picture| (picture.width, picture.height) == (32, 18));
    assert!(is_quadrants(&small));
    // A box of another shape gets the largest frame of the screen's aspect.
    rig.backend.start(&request((100, 100), None)).unwrap();
    let square_box = rig.picture_where(|picture| picture.width == 100);
    assert_eq!(square_box.height, 56);
    let log = rig.portal.log();
    assert_eq!(
        (log.starts, log.selections.len()),
        (1, 1),
        "no second consent"
    );
}

#[test]
fn a_saved_token_goes_back_and_a_valid_one_shows_no_dialog() {
    // A long dialog delay: a restored choice answers at once, and a slow runner
    // must not make that look like a dialog.
    let Some(mut rig) = Rig::new(Duration::from_secs(10), |behavior| {
        behavior.valid_token = Some("saved".to_owned());
        behavior.dialog_time = Duration::from_millis(800);
    }) else {
        return;
    };
    rig.backend.start(&request_for(Some("saved"))).unwrap();
    rig.picture_where(is_quadrants);
    let log = rig.portal.log();
    assert_eq!(log.selections[0].restore_token.as_deref(), Some("saved"));
    assert_eq!(log.dialogs, 0);
    assert!(log.notifications.is_empty(), "restoring needs no notice");
    // The token the portal gave back is for next time, and it came out once.
    assert_eq!(
        rig.report_in(CaptureState::Capturing)
            .restore_token
            .as_deref(),
        Some("token-1")
    );
}

#[test]
fn a_token_that_no_longer_holds_is_announced_before_the_dialog_is_answered() {
    let Some(mut rig) = Rig::new(Duration::from_millis(200), |behavior| {
        behavior.valid_token = Some("current".to_owned());
        behavior.dialog_time = Duration::from_millis(2500);
    }) else {
        return;
    };
    rig.backend.start(&request_for(Some("stale"))).unwrap();
    // The dialog is up: waiting for consent, and the user was told why (D21).
    rig.report_in(CaptureState::WaitingConsent);
    let log = rig.portal.log();
    assert_eq!(log.notifications, ["screen-sync-reprompt"]);
    assert_eq!(log.dialogs, 1);
    // `start` returned at once, and the capture goes on when the user answers.
    rig.picture_where(is_quadrants);
    assert_eq!(
        rig.report_in(CaptureState::Capturing)
            .restore_token
            .as_deref(),
        Some("token-1")
    );
}

#[test]
fn a_first_dialog_without_a_token_is_waiting_for_consent_with_no_notice() {
    let Some(mut rig) = Rig::new(Duration::from_millis(200), |behavior| {
        behavior.dialog_time = Duration::from_millis(1500);
    }) else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.report_in(CaptureState::WaitingConsent);
    rig.picture_where(is_quadrants);
    assert_eq!(rig.portal.log().notifications.len(), 0);
}

#[test]
fn stopping_while_the_dialog_is_open_closes_the_session_at_once() {
    let Some(mut rig) = Rig::new(Duration::from_millis(100), |behavior| {
        behavior.dialog_time = Duration::from_secs(30);
    }) else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.report_in(CaptureState::WaitingConsent);
    let stopped = Instant::now();
    rig.backend.stop();
    assert!(
        stopped.elapsed() < Duration::from_secs(5),
        "stop waited for the user"
    );
    assert_eq!(rig.portal.log().closed_by_client, 1);
    assert_eq!(rig.backend.report().unwrap().state, CaptureState::Off);
    // And the backend starts again afterwards.
    rig.portal
        .change(|behavior| behavior.dialog_time = Duration::from_millis(0));
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
}

#[test]
fn declining_the_dialog_is_an_error_and_the_next_start_asks_again() {
    let Some(mut rig) = Rig::new(Duration::from_secs(5), |behavior| {
        behavior.decline = true;
    }) else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    let report = rig.report_in(CaptureState::Error);
    let message = report.message.expect("the error says why");
    assert!(message.contains("declined"), "{message}");
    match rig.backend.next_picture(Duration::from_secs(1)) {
        Err(CaptureError::Failed(why)) => assert!(why.contains("declined"), "{why}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    // The user changes their mind: the same backend starts again.
    rig.portal.change(|behavior| behavior.decline = false);
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    assert_eq!(rig.portal.log().starts, 2);
}

#[test]
fn a_portal_older_than_restore_tokens_is_refused_with_a_clear_error() {
    let Some(mut rig) = Rig::new(Duration::from_secs(5), |behavior| behavior.version = 3) else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    let message = rig.report_in(CaptureState::Error).message.unwrap();
    assert!(
        message.contains("version 3") && message.contains("4 or newer"),
        "{message}"
    );
    assert!(
        rig.portal.log().selections.is_empty(),
        "nothing was asked of it"
    );
}

#[test]
fn the_agent_registers_its_app_id_before_it_asks_for_a_screen() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    let report = rig.report_in(CaptureState::Capturing);
    assert_eq!(
        rig.portal.log().registered,
        [APP_ID],
        "message: {:?}",
        report.message
    );
    assert_eq!(rig.report_in(CaptureState::Capturing).message, None);
}

#[test]
fn a_portal_without_the_registry_works_and_says_what_it_costs() {
    let Some(mut rig) = Rig::new(Duration::from_secs(5), |behavior| behavior.registry = false)
    else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    let report = rig.report_in(CaptureState::Capturing);
    let message = report.message.expect("the user is told");
    assert!(
        message.contains("ask which screen to share every time"),
        "{message}"
    );
    assert_eq!(rig.portal.log().registered.len(), 0);
}

#[test]
fn a_registry_that_refuses_the_id_is_reported_in_its_own_words() {
    let Some(mut rig) = Rig::new(Duration::from_secs(5), |behavior| {
        behavior.registry_refuses = true;
    }) else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    let message = rig.report_in(CaptureState::Capturing).message.unwrap();
    assert!(message.contains("Invalid app id"), "{message}");
    assert_eq!(rig.portal.log().registered, [APP_ID], "it was asked once");
}

#[test]
fn an_agent_started_in_its_scope_does_not_register_again() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    let cgroup = rig.desktop_dir().join("cgroup");
    std::fs::write(
        &cgroup,
        format!("0::/user.slice/app.slice/{}\n", transient_scope_name("9f3")),
    )
    .unwrap();
    rig.backend = PortalBackend::with_settings(PortalSettings {
        bus_address: Some(rig.desktop_address()),
        dialog_delay: Duration::from_secs(5),
        cgroup_file: Some(cgroup),
    });
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    assert_eq!(rig.portal.log().registered.len(), 0);
    assert_eq!(rig.report_in(CaptureState::Capturing).message, None);
}

#[test]
fn the_desktop_ending_the_share_is_an_error_not_a_silent_restart() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    rig.backend.start(&request_for(None)).unwrap();
    rig.picture_where(is_quadrants);
    rig.portal.end_share();
    let message = rig.report_in(CaptureState::Error).message.unwrap();
    assert!(message.contains("ended the screen share"), "{message}");
    // Nothing starts again by itself: the user stopped the share.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(rig.portal.log().starts, 1);
}

#[test]
fn a_request_for_another_screen_or_out_of_limits_is_refused() {
    let Some(mut rig) = Rig::default() else {
        return;
    };
    let mut other = request_for(None);
    other.screen = "DISPLAY-1".to_owned();
    assert_eq!(
        rig.backend.start(&other),
        Err(CaptureError::UnknownScreen("DISPLAY-1".to_owned()))
    );
    let mut wide = request_for(None);
    wide.width = 481;
    assert!(matches!(
        rig.backend.start(&wide),
        Err(CaptureError::InvalidRequest(_))
    ));
    assert_eq!(rig.portal.log().starts, 0);
}
