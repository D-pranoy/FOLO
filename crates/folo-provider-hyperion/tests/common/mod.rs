//! What the tests of this crate share: a provider on a fake Hyperion, and frames.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use folo_provider::{
    Backoff, Delivery, ElementColors, ElementFrame, Provider, SenderConfig, SenderHandle,
    SenderTarget, TargetOutput, spawn_sender,
};
use folo_provider_hyperion::testing::FakeHyperion;
use folo_provider_hyperion::{HyperionConfig, HyperionProvider};

pub const ORIGIN: &str = "FOLO";
pub const PRIORITY: u16 = 100;

pub fn config(fake: &FakeHyperion) -> HyperionConfig {
    HyperionConfig {
        host: fake.host(),
        port: fake.port(),
        priority: PRIORITY,
        origin: ORIGIN.to_owned(),
        token: None,
    }
}

pub fn provider(fake: &FakeHyperion) -> Arc<HyperionProvider> {
    Arc::new(HyperionProvider::new(config(fake)))
}

pub fn provider_with_token(fake: &FakeHyperion, token: &str) -> Arc<HyperionProvider> {
    Arc::new(HyperionProvider::new(HyperionConfig {
        token: Some(token.to_owned()),
        ..config(fake)
    }))
}

pub fn output(target: &str, levels: &[[f32; 3]]) -> TargetOutput {
    TargetOutput {
        target: target.to_owned(),
        colors: ElementColors::Rgb(levels.to_vec()),
    }
}

pub fn frame(target: &str, levels: &[[f32; 3]]) -> ElementFrame {
    ElementFrame::new(0, vec![output(target, levels)])
}

fn no_jitter() -> f64 {
    0.0
}

pub fn sender_config(provider: &HyperionProvider, targets: &[&str]) -> SenderConfig {
    SenderConfig {
        rate_hz: 40.0,
        targets: targets
            .iter()
            .map(|id| SenderTarget {
                id: (*id).to_owned(),
                delivery: Delivery::Streaming,
            })
            .collect(),
        policy: provider.delivery_policy(),
        backoff: Backoff {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(8),
            reset_after: Duration::from_secs(60),
            jitter: no_jitter,
        },
        start_retry: Duration::from_secs(5),
        describe_every: None,
    }
}

pub fn start_sender(provider: &Arc<HyperionProvider>, targets: &[&str]) -> SenderHandle {
    spawn_sender(provider.clone(), sender_config(provider, targets)).expect("a sender")
}

pub async fn pass(duration: Duration) {
    tokio::time::sleep(duration).await;
}
