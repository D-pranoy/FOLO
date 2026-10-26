//! The sender's timing, on Tokio's paused clock: changed frames only, the keepalive cadence, the
//! token bucket, starting and releasing, reconnecting, and stopping.

use std::sync::Arc;
use std::time::Duration;

use folo_protocol::generated::{ProviderInstance, Target};
use folo_provider::testing::{Event, RecordingProvider};
use folo_provider::{
    Backoff, BucketSpec, Delivery, DeliveryPolicy, ElementColors, ElementFrame, ProviderState,
    SenderConfig, SenderHandle, SenderTarget, TargetOutput, spawn_sender,
};
use tokio::time::{Instant, sleep};

const MS: Duration = Duration::from_millis(1);

fn rgb(level: f32) -> ElementColors {
    ElementColors::Rgb(vec![[level, 0.0, 0.0]; 3])
}

fn frame(targets: &[(&str, f32)]) -> ElementFrame {
    ElementFrame::new(
        0,
        targets
            .iter()
            .map(|(id, level)| TargetOutput {
                target: (*id).to_owned(),
                colors: rgb(*level),
            })
            .collect(),
    )
}

fn no_jitter() -> f64 {
    0.0
}

fn config(rate_hz: f64, policy: DeliveryPolicy, targets: &[(&str, Delivery)]) -> SenderConfig {
    SenderConfig {
        rate_hz,
        targets: targets
            .iter()
            .map(|(id, delivery)| SenderTarget {
                id: (*id).to_owned(),
                delivery: *delivery,
            })
            .collect(),
        policy,
        backoff: Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(8),
            reset_after: Duration::from_secs(60),
            jitter: no_jitter,
        },
        release_timeout: Duration::from_secs(2),
    }
}

fn policy(keepalive_ms: u64) -> DeliveryPolicy {
    DeliveryPolicy {
        keepalive: Duration::from_millis(keepalive_ms),
        bucket: BucketSpec {
            capacity: 2,
            per_second: 1.0,
        },
    }
}

fn start(provider: &Arc<RecordingProvider>, config: SenderConfig) -> SenderHandle {
    spawn_sender(provider.clone(), config).expect("a sender")
}

#[tokio::test(start_paused = true)]
async fn only_changed_frames_are_sent() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("strip", Delivery::Streaming)]),
    );

    sender.submit(frame(&[("strip", 10.0)]));
    sleep(200 * MS).await;
    assert_eq!(
        provider.sends().len(),
        1,
        "an unchanged frame isn't sent again"
    );

    // A different float that truncates to the same byte is the same frame to the device
    sender.submit(frame(&[("strip", 10.9)]));
    sleep(200 * MS).await;
    assert_eq!(provider.sends().len(), 1);

    sender.submit(frame(&[("strip", 11.0)]));
    sleep(200 * MS).await;
    let sends = provider.sends();
    assert_eq!(sends.len(), 2);
    assert_eq!(sends[1].1.targets[0].colors, rgb(11.0));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_newest_frame_wins() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("strip", Delivery::Streaming)]),
    );
    sleep(100 * MS).await;
    for level in 1..=10 {
        #[allow(clippy::cast_precision_loss)]
        sender.submit(frame(&[("strip", level as f32)]));
    }
    sleep(100 * MS).await;
    let sends = provider.sends();
    assert_eq!(
        sends.len(),
        1,
        "frames submitted between two ticks collapse into the newest"
    );
    assert_eq!(sends[0].1.targets[0].colors, rgb(10.0));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_frame_is_repeated_as_a_keepalive() {
    let provider = Arc::new(RecordingProvider::new());
    // 40 Hz: a tick every 25 ms, so a keepalive is due at most one tick late
    let sender = start(
        &provider,
        config(40.0, policy(500), &[("strip", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("strip", 10.0)]));
    sleep(2_100 * MS).await;

    let sends = provider.sends();
    assert_eq!(sends.len(), 5, "at 0, 0.5, 1.0, 1.5 and 2.0 s");
    for pair in sends.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!(gap >= 500 * MS && gap < 525 * MS, "keepalive gap {gap:?}");
    }
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_change_does_not_wait_for_the_keepalive() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(500), &[("strip", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("strip", 10.0)]));
    sleep(100 * MS).await;
    let before = Instant::now();
    sender.submit(frame(&[("strip", 20.0)]));
    sleep(100 * MS).await;
    let sends = provider.sends();
    assert_eq!(sends.len(), 2);
    assert!(sends[1].0 - before <= 25 * MS, "sent on the next tick");
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn state_targets_are_paced_by_the_token_bucket() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(500), &[("lamp", Delivery::State)]),
    );
    let begun = Instant::now();

    // A different colour every tick for 3.1 s: the bucket (2 tokens, 1 a second) paces them
    for step in 0..125u16 {
        sender.submit(frame(&[("lamp", f32::from(step % 200))]));
        sleep(25 * MS).await;
    }
    let sends = provider.sends();
    let times: Vec<Duration> = sends.iter().map(|(at, _)| *at - begun).collect();
    assert_eq!(
        times.len(),
        5,
        "two from the full bucket, then one a second: {times:?}"
    );
    assert!(
        times[0] < 50 * MS && times[1] < 75 * MS,
        "the burst: {times:?}"
    );
    for (index, expected) in [(2, 1_000u64), (3, 2_000), (4, 3_000)] {
        let ms = u64::try_from(times[index].as_millis()).unwrap();
        assert!(
            (expected..expected + 60).contains(&ms),
            "send {index} at {ms} ms"
        );
    }
    // The device is told to take as long as the interval between updates
    assert_eq!(sends[2].1.transition, Duration::from_secs(1));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn state_targets_have_no_keepalive() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(100), &[("lamp", Delivery::State)]),
    );
    sender.submit(frame(&[("lamp", 50.0)]));
    sleep(3_000 * MS).await;
    assert_eq!(provider.sends().len(), 1, "sent only on a visible change");
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_held_back_update_is_sent_when_a_token_comes() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(500), &[("lamp", Delivery::State)]),
    );
    let begun = Instant::now();
    // Three changes in a row: two take the tokens, the third waits and is sent a second later
    for level in [10.0, 20.0, 30.0] {
        sender.submit(frame(&[("lamp", level)]));
        sleep(30 * MS).await;
    }
    sleep(1_200 * MS).await;
    let sends = provider.sends();
    assert_eq!(sends.len(), 3);
    assert_eq!(sends[2].1.targets[0].colors, rgb(30.0));
    assert!(sends[2].0 - begun >= 1_000 * MS);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn targets_start_before_their_first_frame_and_release_when_dropped() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(
            40.0,
            policy(10_000),
            &[("a", Delivery::Streaming), ("b", Delivery::Streaming)],
        ),
    );
    sender.submit(frame(&[("a", 1.0), ("b", 2.0)]));
    sleep(100 * MS).await;
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    sender.shutdown().await;

    let names: Vec<String> = provider
        .events()
        .iter()
        .map(|e| match e {
            Event::Connect => "connect".to_owned(),
            Event::Describe => "describe".to_owned(),
            Event::Start(t) => format!("start {t}"),
            Event::Send { frame, .. } => format!("send {}", frame.targets.len()),
            Event::Release(t) => format!("release {t}"),
            Event::Disconnect => "disconnect".to_owned(),
            Event::Admin { .. } => "admin".to_owned(),
        })
        .collect();
    assert_eq!(
        names,
        [
            "connect",
            "describe",
            "start a",
            "start b",
            "send 2",
            "release b",
            "release a",
            "disconnect"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn an_empty_frame_releases_every_target() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("a", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    sender.submit(frame(&[]));
    sleep(100 * MS).await;
    assert!(provider.events().contains(&Event::Release("a".to_owned())));
    // Driven again, it starts and sends again, whatever it showed before
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    let starts = provider
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Start(_)))
        .count();
    assert_eq!(starts, 2);
    assert_eq!(provider.sends().len(), 2);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_state_follows_the_connection() {
    let provider = Arc::new(RecordingProvider::new());
    provider.fail_connects(2);
    let sender = start(&provider, config(40.0, policy(10_000), &[]));
    let mut status = sender.status();
    assert_eq!(status.borrow().state, ProviderState::Connecting);

    sleep(10 * MS).await;
    {
        let now = status.borrow_and_update();
        assert_eq!(now.state, ProviderState::Error);
        assert!(now.message.as_deref().is_some_and(|m| m.contains("down")));
    }
    // Retries after 1 s and then 2 s (doubling, no jitter in this test): connected at 3 s
    sleep(990 * MS).await;
    assert_eq!(status.borrow().state, ProviderState::Error);
    sleep(2_100 * MS).await;
    assert_eq!(status.borrow().state, ProviderState::Connected);
    assert_eq!(status.borrow().message, None);
    let connects = provider
        .events()
        .iter()
        .filter(|e| matches!(e, Event::Connect))
        .count();
    assert_eq!(connects, 3);

    sender.shutdown().await;
    assert_eq!(status.borrow().state, ProviderState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn describe_is_published_after_each_connection() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(&provider, config(40.0, policy(10_000), &[]));
    let described = sender.described();
    assert!(described.borrow().is_none());
    sleep(10 * MS).await;
    assert_eq!(described.borrow().as_ref().map(Vec::len), Some(0));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_send_reconnects_and_starts_the_targets_again() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("a", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    assert_eq!(provider.sends().len(), 1);

    provider.set_fail_sends(true);
    sender.submit(frame(&[("a", 2.0)]));
    sleep(100 * MS).await;
    assert_eq!(sender.status().borrow().state, ProviderState::Error);

    provider.set_fail_sends(false);
    sleep(1_200 * MS).await;
    assert_eq!(sender.status().borrow().state, ProviderState::Connected);
    let events = provider.events();
    let connects = events
        .iter()
        .filter(|e| matches!(e, Event::Connect))
        .count();
    let starts = events
        .iter()
        .filter(|e| matches!(e, Event::Start(_)))
        .count();
    assert_eq!(
        (connects, starts),
        (2, 2),
        "the target starts again on the new connection"
    );
    // What was sent last isn't remembered: the newest frame goes out in full
    let sends = provider.sends();
    assert_eq!(sends.last().unwrap().1.targets[0].colors, rgb(2.0));
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_failing_start_is_an_error_and_is_retried() {
    let provider = Arc::new(RecordingProvider::new());
    provider.set_fail_starts(true);
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("a", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    assert_eq!(sender.status().borrow().state, ProviderState::Error);
    assert_eq!(provider.sends().len(), 0);
    provider.set_fail_starts(false);
    sleep(1_200 * MS).await;
    assert_eq!(provider.sends().len(), 1);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_backoff_doubles_up_to_its_limit() {
    let backoff = Backoff {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(8),
        reset_after: Duration::from_secs(60),
        jitter: no_jitter,
    };
    let delays: Vec<u64> = (0..6).map(|n| backoff.delay(n).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 8, 8]);
    // Jitter cuts each delay by up to a quarter
    let cut = Backoff {
        jitter: || 1.0,
        ..backoff
    };
    assert_eq!(cut.delay(2), Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn shutdown_during_a_backoff_returns_at_once() {
    let provider = Arc::new(RecordingProvider::new());
    provider.fail_connects(100);
    let sender = start(&provider, config(40.0, policy(10_000), &[]));
    sleep(10 * MS).await;
    let begun = Instant::now();
    sender.shutdown().await;
    assert!(begun.elapsed() < 100 * MS);
    assert!(provider.events().contains(&Event::Disconnect));
}

#[tokio::test(start_paused = true)]
async fn dropping_the_handle_stops_the_sender_like_a_shutdown() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(10_000), &[("a", Delivery::Streaming)]),
    );
    sender.submit(frame(&[("a", 1.0)]));
    sleep(100 * MS).await;
    drop(sender);
    sleep(100 * MS).await;
    let events = provider.events();
    assert!(events.ends_with(&[Event::Release("a".to_owned()), Event::Disconnect]));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_sent_before_a_frame_and_nothing_starts() {
    let provider = Arc::new(RecordingProvider::new());
    let sender = start(
        &provider,
        config(40.0, policy(100), &[("a", Delivery::Streaming)]),
    );
    sleep(2_000 * MS).await;
    let events = provider.events();
    assert_eq!(
        events,
        [Event::Connect, Event::Describe],
        "an idle instance opens no output"
    );
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_config_of_an_instance_follows_its_targets_capabilities() {
    let instance: ProviderInstance = serde_json::from_value(serde_json::json!({
        "id": "hue-1", "kind": "hue", "rate_hz": 50
    }))
    .unwrap();
    let target = |id: &str, bound_to: &str, caps: serde_json::Value| -> Target {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "provider": {"instance": bound_to},
            "capabilities": caps
        }))
        .unwrap()
    };
    let targets = vec![
        // Streaming, but the lamp takes 25 Hz at most
        target(
            "slow",
            "hue-1",
            serde_json::json!({
                "discovered": {"limits": {"max_rate_hz": 25}}
            }),
        ),
        // The user's override makes this one a state target, and the rate limit no longer applies
        target(
            "lamp",
            "hue-1",
            serde_json::json!({
                "discovered": {"limits": {"max_rate_hz": 2}},
                "overrides": {"delivery": "state"}
            }),
        ),
        // Another instance's target
        target("other", "wled-1", serde_json::json!({})),
    ];
    let config = SenderConfig::from_instance(&instance, DeliveryPolicy::default(), &targets);
    assert!((config.rate_hz - 25.0).abs() < 1e-9);
    assert_eq!(
        config.targets,
        [
            SenderTarget {
                id: "slow".to_owned(),
                delivery: Delivery::Streaming
            },
            SenderTarget {
                id: "lamp".to_owned(),
                delivery: Delivery::State
            },
        ]
    );
}
