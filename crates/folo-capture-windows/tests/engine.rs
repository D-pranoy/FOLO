//! The backend's recovery paths with injected faults (ADR-0037 D22): a fake
//! factory scripts what opening and grabbing return, and a fake timer
//! advances only when the backend sleeps.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use folo_capture_core::backend::CaptureBackend;
use folo_capture_core::error::CaptureError;
use folo_capture_core::types::{
    Bars, BlackBarMode, BlackBarPreset, BlackBarSettings, CaptureRequest, FrameFlags, Picture,
    Profile, Screen,
};
use folo_capture_windows::engine::{Engine, Grabber, GrabberFactory, PRIVACY_SETTINGS_URI, Timer};
use folo_capture_windows::fault::Fault;
use folo_capture_windows::recovery::{BackendKind, Policy};

type Grabs = Vec<Result<Option<Picture>, Fault>>;
type OpenResult = Result<Grabs, Fault>;
type Script<T> = Arc<Mutex<VecDeque<T>>>;

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, line: String) {
        self.0.lock().unwrap().push(line);
    }
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct FakeGrabber {
    grabs: VecDeque<Result<Option<Picture>, Fault>>,
    log: Log,
    kind: BackendKind,
    closed: Arc<Mutex<u32>>,
}

impl Grabber for FakeGrabber {
    fn grab(&mut self, _timeout: Duration) -> Result<Option<Picture>, Fault> {
        self.grabs.pop_front().unwrap_or(Ok(None))
    }
    fn apply(&mut self, request: &CaptureRequest) {
        self.log.push(format!(
            "apply {} {}x{}",
            self.kind.name(),
            request.width,
            request.height
        ));
    }
}

impl Drop for FakeGrabber {
    fn drop(&mut self) {
        *self.closed.lock().unwrap() += 1;
    }
}

struct FakeFactory {
    screens: Arc<Mutex<Vec<Screen>>>,
    /// One entry per `open`: the fault, or the grabs of the grabber it opens.
    opens: Script<OpenResult>,
    log: Log,
    closed: Arc<Mutex<u32>>,
}

impl GrabberFactory for FakeFactory {
    fn screens(&mut self) -> Result<Vec<Screen>, CaptureError> {
        Ok(self.screens.lock().unwrap().clone())
    }

    fn open(
        &mut self,
        kind: BackendKind,
        screen_id: &str,
        _request: &CaptureRequest,
    ) -> Result<Box<dyn Grabber>, Fault> {
        self.log.push(format!("open {} {screen_id}", kind.name()));
        let next = self
            .opens
            .lock()
            .unwrap()
            .pop_front()
            .expect("an unscripted open");
        next.map(|grabs| {
            Box::new(FakeGrabber {
                grabs: grabs.into(),
                log: self.log.clone(),
                kind,
                closed: Arc::clone(&self.closed),
            }) as Box<dyn Grabber>
        })
    }
}

#[derive(Clone, Default)]
struct FakeTimer {
    now_us: Arc<Mutex<u64>>,
    slept: Arc<Mutex<Vec<Duration>>>,
}

impl Timer for FakeTimer {
    fn now_us(&self) -> u64 {
        *self.now_us.lock().unwrap()
    }
    fn sleep(&self, duration: Duration) {
        *self.now_us.lock().unwrap() += u64::try_from(duration.as_micros()).unwrap();
        self.slept.lock().unwrap().push(duration);
    }
}

struct Rig {
    backend: Engine<FakeFactory, FakeTimer>,
    log: Log,
    timer: FakeTimer,
    closed: Arc<Mutex<u32>>,
    opens: Script<OpenResult>,
    screens: Arc<Mutex<Vec<Screen>>>,
}

fn screen(id: &str) -> Screen {
    Screen {
        id: id.to_owned(),
        name: id.to_owned(),
        width: 1920,
        height: 1080,
        primary: id == "a",
    }
}

fn picture() -> Picture {
    Picture {
        screen_index: 9,
        width: 2,
        height: 1,
        flags: FrameFlags::default(),
        bars: Bars::default(),
        pixels: vec![1, 2, 3, 4, 5, 6],
    }
}

fn request(screen: &str) -> CaptureRequest {
    CaptureRequest {
        screen: screen.to_owned(),
        width: 480,
        height: 270,
        rate_hz: 30.0,
        black_bars: BlackBarSettings {
            preset: BlackBarPreset::Auto,
            mode: BlackBarMode::Default,
            threshold: 0.05,
        },
        profile: Profile::Video,
        restore_token: None,
    }
}

fn rig(order: Vec<BackendKind>, opens: Vec<OpenResult>) -> Rig {
    let (log, timer) = (Log::default(), FakeTimer::default());
    let closed = Arc::new(Mutex::new(0));
    let screens = Arc::new(Mutex::new(vec![screen("a"), screen("b")]));
    let opens: Script<_> = Arc::new(Mutex::new(opens.into()));
    let factory = FakeFactory {
        screens: Arc::clone(&screens),
        opens: Arc::clone(&opens),
        log: log.clone(),
        closed: Arc::clone(&closed),
    };
    Rig {
        backend: Engine::new(factory, timer.clone(), order, Policy::default()),
        log,
        timer,
        closed,
        opens,
        screens,
    }
}

const SECOND: Duration = Duration::from_secs(1);

#[test]
fn a_picture_gets_the_screens_position_and_the_status_is_not_paused() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![Ok(vec![Ok(Some(picture()))])],
    );
    rig.backend.start(&request("b")).unwrap();
    let got = rig.backend.next_picture(SECOND).unwrap().unwrap();
    assert_eq!(got.screen_index, 1);
    assert_eq!(got.pixels, picture().pixels);
    assert!(!rig.backend.status().paused);
    assert_eq!(rig.backend.name(), "dda");
}

#[test]
fn an_unknown_screen_and_a_bad_request_are_refused_before_anything_opens() {
    let mut rig = rig(BackendKind::auto_order(), vec![]);
    assert_eq!(
        rig.backend.start(&request("zzz")),
        Err(CaptureError::UnknownScreen("zzz".to_owned()))
    );
    let mut wide = request("a");
    wide.width = 481;
    assert!(matches!(
        rig.backend.start(&wide),
        Err(CaptureError::InvalidRequest(_))
    ));
    assert_eq!(rig.log.lines().len(), 0);
    assert_eq!(
        rig.backend.next_picture(SECOND),
        Err(CaptureError::NotStarted)
    );
}

#[test]
fn unsupported_at_start_falls_through_wgc_and_gdi_in_one_call() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![Err(Fault::Unsupported), Err(Fault::Unsupported), Ok(vec![])],
    );
    rig.backend.start(&request("a")).unwrap();
    assert_eq!(rig.log.lines(), ["open dda a", "open wgc a", "open gdi a"]);
    assert_eq!(rig.backend.name(), "gdi");
    assert!(!rig.backend.status().paused);
}

#[test]
fn nothing_left_to_try_fails_the_start_and_names_why() {
    let mut rig = rig(vec![BackendKind::Dda], vec![Err(Fault::Unsupported)]);
    let Err(CaptureError::Failed(why)) = rig.backend.start(&request("a")) else {
        panic!("start must fail");
    };
    assert!(why.contains("doesn't work for this screen"), "{why}");
}

#[test]
fn access_lost_pauses_closes_the_duplication_and_reopens_after_the_backoff() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![
            Ok(vec![Ok(Some(picture())), Err(Fault::AccessLost)]),
            Ok(vec![Ok(Some(picture()))]),
        ],
    );
    rig.backend.start(&request("a")).unwrap();
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());

    // The fault: no picture, paused, the grabber closed, a retry due in 100 ms.
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    let status = rig.backend.status();
    assert!(status.paused);
    assert!(status.message.unwrap().contains("mode change"));
    assert_eq!(*rig.closed.lock().unwrap(), 1);

    // The retry waits out the backoff, opens again, and the next call grabs.
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    assert_eq!(
        rig.timer.slept.lock().unwrap().clone(),
        [Duration::from_millis(100)]
    );
    // Reopened, but paused until the first picture arrives.
    assert!(rig.backend.status().paused);
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
    assert!(!rig.backend.status().paused);
    assert_eq!(rig.log.lines(), ["open dda a", "open dda a"]);
}

#[test]
fn a_short_timeout_returns_before_the_retry_is_due_and_waits_only_that_long() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![Ok(vec![Err(Fault::SecureDesktop)]), Ok(vec![])],
    );
    rig.backend.start(&request("a")).unwrap();
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    // 30 ms of the 100 ms backoff.
    assert_eq!(
        rig.backend.next_picture(Duration::from_millis(30)).unwrap(),
        None
    );
    assert_eq!(
        rig.timer.slept.lock().unwrap().clone(),
        [Duration::from_millis(30)]
    );
    assert!(rig.backend.status().paused);
    assert_eq!(rig.log.lines(), ["open dda a"]);
    // The rest of it, and the reopen.
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    assert_eq!(rig.log.lines(), ["open dda a", "open dda a"]);
}

#[test]
fn the_secure_desktop_keeps_the_agent_paused_while_opening_keeps_failing() {
    let mut script = vec![Ok(vec![Err(Fault::AccessLost)])];
    script.extend((0..20).map(|_| Err(Fault::SecureDesktop)));
    script.push(Ok(vec![Ok(Some(picture()))]));
    let mut rig = rig(BackendKind::auto_order(), script);
    rig.backend.start(&request("a")).unwrap();
    // The timeout is longer than the longest delay, so each call ends at an open.
    while rig.log.lines().len() < 22 {
        assert_eq!(
            rig.backend.next_picture(Duration::from_secs(3)).unwrap(),
            None
        );
        assert!(rig.backend.status().paused || rig.log.lines().len() == 22);
    }
    // Back on the normal desktop: the last open worked, and the next call grabs.
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
    assert!(!rig.backend.status().paused);
    // The delays doubled up to the two-second cap.
    let slept = rig.timer.slept.lock().unwrap().clone();
    assert_eq!(slept.iter().max(), Some(&Duration::from_secs(2)));
    // The first fault was another class (AccessLost), which started its own count.
    assert_eq!(
        slept[..3],
        [
            Duration::from_millis(100),
            Duration::from_millis(100),
            Duration::from_millis(200)
        ]
    );
}

#[test]
fn a_disconnected_session_pauses_with_its_own_words() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![Ok(vec![Err(Fault::SessionDisconnected)])],
    );
    rig.backend.start(&request("a")).unwrap();
    rig.backend.next_picture(SECOND).unwrap();
    let status = rig.backend.status();
    assert!(status.paused);
    assert!(status.message.unwrap().contains("session is disconnected"));
    assert_eq!(status.settings_uri, None);
}

#[test]
fn the_process_limit_is_retried_three_times_and_then_wgc_is_tried() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![
            Ok(vec![Err(Fault::ProcessLimit)]),
            Err(Fault::ProcessLimit),
            Err(Fault::ProcessLimit),
            Err(Fault::ProcessLimit),
            Ok(vec![Ok(Some(picture()))]),
        ],
    );
    rig.backend.start(&request("a")).unwrap();
    for _ in 0..8 {
        rig.backend.next_picture(SECOND).unwrap();
        if rig.backend.name() == "wgc" {
            break;
        }
    }
    assert_eq!(rig.backend.name(), "wgc");
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
    assert_eq!(
        rig.log.lines(),
        [
            "open dda a",
            "open dda a",
            "open dda a",
            "open dda a",
            "open wgc a"
        ]
    );
    assert!(rig.opens.lock().unwrap().is_empty());
}

#[test]
fn unsupported_while_capturing_switches_backend_without_waiting() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![
            Ok(vec![Err(Fault::Unsupported)]),
            Ok(vec![Ok(Some(picture()))]),
        ],
    );
    rig.backend.start(&request("a")).unwrap();
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    assert_eq!(rig.backend.name(), "wgc");
    assert!(rig.timer.slept.lock().unwrap().is_empty());
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
}

#[test]
fn the_privacy_setting_pauses_with_a_link_and_no_other_backend_is_tried() {
    let mut script = Vec::new();
    script.extend((0..30).map(|_| Err(Fault::PrivacyDenied)));
    let mut rig = rig(vec![BackendKind::Wgc, BackendKind::Gdi], script);
    rig.backend.start(&request("a")).unwrap();
    for _ in 0..29 {
        rig.backend.next_picture(SECOND).unwrap();
    }
    let status = rig.backend.status();
    assert!(status.paused);
    assert_eq!(status.settings_uri.as_deref(), Some(PRIVACY_SETTINGS_URI));
    assert!(rig.log.lines().iter().all(|line| line == "open wgc a"));
}

#[test]
fn a_screen_that_is_unplugged_is_waited_for() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![Err(Fault::ScreenGone), Ok(vec![Ok(Some(picture()))])],
    );
    rig.backend.start(&request("a")).unwrap();
    assert!(rig.backend.status().paused);
    assert_eq!(rig.backend.next_picture(SECOND).unwrap(), None);
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
}

#[test]
fn giving_up_mid_capture_is_an_error_every_call_repeats() {
    let mut rig = rig(
        vec![BackendKind::Gdi],
        vec![Ok(vec![Err(Fault::Unsupported)])],
    );
    rig.backend.start(&request("a")).unwrap();
    let first = rig.backend.next_picture(SECOND);
    assert!(matches!(first, Err(CaptureError::Failed(_))), "{first:?}");
    assert_eq!(rig.backend.next_picture(SECOND), first);
}

#[test]
fn the_same_start_again_after_a_failure_tries_again() {
    let mut rig = rig(
        vec![BackendKind::Gdi],
        vec![
            Ok(vec![Err(Fault::Unsupported)]),
            Ok(vec![Ok(Some(picture()))]),
        ],
    );
    rig.backend.start(&request("a")).unwrap();
    assert!(rig.backend.next_picture(SECOND).is_err());
    rig.backend.start(&request("a")).unwrap();
    assert!(rig.backend.next_picture(SECOND).unwrap().is_some());
}

#[test]
fn a_reopen_after_the_screens_moved_in_the_list_updates_the_pictures_screen_index() {
    let mut rig = rig(
        BackendKind::auto_order(),
        vec![
            Ok(vec![Err(Fault::AccessLost)]),
            Ok(vec![Ok(Some(picture()))]),
        ],
    );
    rig.backend.start(&request("b")).unwrap();
    rig.backend.next_picture(SECOND).unwrap();
    *rig.screens.lock().unwrap() = vec![screen("b"), screen("a")];
    rig.backend.next_picture(SECOND).unwrap();
    let got = rig.backend.next_picture(SECOND).unwrap().unwrap();
    assert_eq!(got.screen_index, 0);
}

#[test]
fn a_repeated_start_for_the_same_screen_applies_in_place_and_another_screen_restarts() {
    let mut rig = rig(BackendKind::auto_order(), vec![Ok(vec![]), Ok(vec![])]);
    rig.backend.start(&request("a")).unwrap();
    let mut smaller = request("a");
    smaller.width = 240;
    smaller.height = 135;
    rig.backend.start(&smaller).unwrap();
    assert_eq!(rig.log.lines(), ["open dda a", "apply dda 240x135"]);
    assert_eq!(*rig.closed.lock().unwrap(), 0);

    rig.backend.start(&request("b")).unwrap();
    assert_eq!(rig.log.lines().last().unwrap(), "open dda b");
    assert_eq!(*rig.closed.lock().unwrap(), 1);
}

#[test]
fn stop_closes_the_grabber_and_a_stopped_backend_has_nothing_to_report() {
    let mut rig = rig(BackendKind::auto_order(), vec![Ok(vec![])]);
    rig.backend.start(&request("a")).unwrap();
    rig.backend.stop();
    assert_eq!(*rig.closed.lock().unwrap(), 1);
    assert_eq!(
        rig.backend.next_picture(SECOND),
        Err(CaptureError::NotStarted)
    );
    assert!(!rig.backend.status().paused);
    rig.backend.stop();
}

#[test]
fn the_screens_are_the_factorys_and_listing_opens_nothing() {
    let mut rig = rig(BackendKind::auto_order(), vec![]);
    let ids: Vec<String> = rig
        .backend
        .list_screens()
        .unwrap()
        .into_iter()
        .map(|screen| screen.id)
        .collect();
    assert_eq!(ids, ["a", "b"]);
    assert_eq!(rig.log.lines().len(), 0);
}
