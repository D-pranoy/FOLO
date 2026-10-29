//! The provider under the sender, against the fake Hyperion on Tokio's paused clock: changed
//! frames only, the 0.5 s keepalive, `clear` on release and on shutdown, reconnecting with
//! backoff, states, and a target Hyperion doesn't have.

mod common;

use std::time::Duration;

use folo_provider::{Provider, ProviderError, ProviderState};
use folo_provider_hyperion::testing::FakeHyperion;
use folo_provider_hyperion::{HyperionConfig, HyperionProvider};

use common::{frame, output, pass, provider, start_sender};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[tokio::test(start_paused = true)]
async fn only_changed_frames_go_out_and_an_unchanged_one_repeats_every_half_second() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    pass(ms(100)).await;
    sender.submit(frame("0", &[[10.0, 20.0, 30.0]; 3]));
    pass(ms(2100)).await;

    let sent = fake.color_frames();
    assert_eq!(sent.len(), 5, "the first frame, then one every 0.5 s");
    for pair in sent.windows(2) {
        let gap = pair[1].at - pair[0].at;
        assert!(
            gap >= ms(500) && gap <= ms(530),
            "keepalive after {gap:?}, expected 0.5 s (and up to one tick of 25 ms later)"
        );
    }
    assert!(sent.iter().all(|f| f.line == sent[0].line));

    // A change goes out within a tick, and restarts the keepalive count
    sender.submit(frame("0", &[[11.0, 20.0, 30.0]; 3]));
    pass(ms(100)).await;
    let sent = fake.color_frames();
    assert_eq!(sent.len(), 6);
    assert_eq!(sent[5].levels(), [11, 20, 30, 11, 20, 30, 11, 20, 30]);

    // A change below a whole level isn't a change on the wire
    sender.submit(frame("0", &[[11.9, 20.5, 30.1]; 3]));
    pass(ms(200)).await;
    assert_eq!(fake.color_frames().len(), 6);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn every_frame_carries_the_priority_origin_and_duration() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    sender.submit(frame("0", &[[1.0, 2.0, 3.0]]));
    pass(ms(200)).await;
    let sent = fake.color_frames();
    assert_eq!(sent[0].command["priority"], 100);
    assert_eq!(sent[0].command["origin"], "FOLO");
    assert_eq!(sent[0].command["duration"], 2000);
    assert_eq!(sent[0].levels(), [1, 2, 3]);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_state_follows_the_connection() {
    let fake = FakeHyperion::start().await;
    fake.set_accepting(false);
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    let mut status = sender.status();
    pass(ms(100)).await;
    assert_eq!(status.borrow_and_update().state, ProviderState::Error);

    fake.set_accepting(true);
    pass(Duration::from_secs(2)).await;
    assert_eq!(status.borrow_and_update().state, ProviderState::Connected);
    assert_eq!(status.borrow().message, None);

    sender.submit(frame("0", &[[1.0, 1.0, 1.0]]));
    pass(ms(100)).await;
    fake.drop_connections();
    // The next keepalive finds the connection closed
    pass(ms(700)).await;
    let now = status.borrow_and_update().clone();
    assert_eq!(now.state, ProviderState::Error, "{now:?}");
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn it_reconnects_with_backoff_after_the_fake_drops_and_sends_again() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    sender.submit(frame("0", &[[5.0, 5.0, 5.0]]));
    pass(ms(200)).await;
    assert_eq!(fake.connections(), 1, "one connection per running instance");
    let before = fake.color_frames().len();
    assert!(before >= 1);

    fake.set_accepting(false);
    fake.drop_connections();
    pass(Duration::from_secs(3)).await;
    assert!(
        fake.color_frames().len() <= before + 1,
        "nothing is sent while it can't connect"
    );

    fake.set_accepting(true);
    pass(Duration::from_secs(10)).await;
    assert!(fake.connections() > 1, "it came back");
    let after = fake.color_frames();
    assert!(after.len() > before + 1, "frames flow again");
    let last = after.last().unwrap();
    assert_eq!(last.levels(), [5, 5, 5]);
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_priority_is_cleared_when_a_target_is_no_longer_driven() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    sender.submit(frame("0", &[[9.0, 9.0, 9.0]; 2]));
    pass(ms(200)).await;
    assert_eq!(fake.priorities(0), vec![(100, "FOLO".to_owned())]);

    sender.submit(folo_provider::ElementFrame::new(0, Vec::new()));
    pass(ms(200)).await;
    let clears = fake.commands("clear");
    assert_eq!(clears.len(), 1);
    assert_eq!(clears[0].command["priority"], 100);
    assert!(
        fake.priorities(0).is_empty(),
        "Hyperion no longer shows our colours"
    );
    sender.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_priority_is_cleared_and_the_connection_closed_on_shutdown() {
    let fake = FakeHyperion::start().await;
    fake.add_priority(0, 50, "Someone else", "JSON API");
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0"]);
    sender.submit(frame("0", &[[9.0, 9.0, 9.0]; 2]));
    pass(ms(200)).await;
    assert_eq!(fake.priorities(0).len(), 2);

    sender.shutdown().await;
    pass(ms(100)).await;
    assert_eq!(fake.priorities(0), vec![(50, "Someone else".to_owned())]);
    let clears = fake.commands("clear");
    assert_eq!(clears.len(), 1, "only our own priority is cleared");
    assert_eq!(clears[0].command["priority"], 100);
}

#[tokio::test(start_paused = true)]
async fn a_target_hyperion_does_not_have_is_refused_and_the_others_carry_on() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    let sender = start_sender(&provider, &["0", "7"]);
    sender.submit(folo_provider::ElementFrame::new(
        0,
        vec![
            output("7", &[[1.0, 1.0, 1.0]]),
            output("0", &[[2.0, 2.0, 2.0]]),
        ],
    ));
    pass(ms(300)).await;
    let sent = fake.color_frames();
    assert!(!sent.is_empty());
    assert!(sent.iter().all(|f| f.levels() == [2, 2, 2]));
    assert_eq!(sender.status().borrow().state, ProviderState::Connected);
    sender.shutdown().await;
}

#[tokio::test]
async fn releasing_a_target_that_never_ran_is_harmless() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    let element = provider.element_output().unwrap();
    element.release("0").await.expect("harmless");
    element.release("not-a-number").await.expect("harmless");
    assert!(fake.color_frames().is_empty());
}

#[tokio::test]
async fn nothing_listening_is_unreachable() {
    let fake = FakeHyperion::start().await;
    let port = fake.port();
    let host = fake.host();
    drop(fake);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let provider = HyperionProvider::new(HyperionConfig {
        host,
        port,
        priority: 100,
        origin: "FOLO".to_owned(),
        token: None,
    });
    let error = provider.connect().await.unwrap_err();
    assert!(matches!(error, ProviderError::Unreachable(_)), "{error:?}");
}

/// Hyperion answers every command. A client that doesn't read the answers fills its socket
/// buffers, and stops. Here the replies add up to far more than the buffers hold.
#[tokio::test]
async fn the_replies_are_read_so_a_long_run_never_stalls() {
    let fake = FakeHyperion::start().await;
    let provider = provider(&fake);
    provider.connect().await.unwrap();
    provider.describe().await.unwrap();
    let element = provider.element_output().unwrap();
    element.start("0").await.unwrap();
    let frame = frame("0", &[[1.0, 2.0, 3.0]]);
    let run = async {
        for _ in 0..100_000 {
            element.send(&frame).await.unwrap();
        }
    };
    tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .expect("sending never blocks on unread replies");
    provider.disconnect().await;
}
