//! The sender: one task per provider instance (`docs/architecture/providers.md`, the sender).
//!
//! It keeps the instance connected (with backoff), describes it after each connection, starts a
//! target when it is first driven, sends only the frames that changed plus a keepalive (or, for
//! state targets, paced updates from a token bucket), and releases targets nothing drives any
//! more. Which targets are driven is what the newest submitted frame holds.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use folo_protocol::generated::{ProviderInstance, Target};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::capabilities::effective_capabilities;
use crate::error::ProviderError;
use crate::traits::{ElementOutput, Provider};
use crate::types::{
    Delivery, DeliveryPolicy, DescribedTarget, ElementFrame, ProviderState, ProviderStatus,
    TargetOutput,
};

/// The delays between connection attempts: `initial`, doubling up to `max`, each cut by up to
/// 25% at random, as the node does for its engine. The count starts over once a connection has
/// lasted `reset_after`.
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
    pub reset_after: Duration,
    /// A random number in 0–1, one per delay; tests pass a constant.
    pub jitter: fn() -> f64,
}

fn random_unit() -> f64 {
    let mut bytes = [0u8; 4];
    if getrandom::fill(&mut bytes).is_err() {
        return 0.0;
    }
    f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX)
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            initial: Duration::from_secs(1),
            max: Duration::from_secs(60),
            reset_after: Duration::from_secs(60),
            jitter: random_unit,
        }
    }
}

impl Backoff {
    /// The delay before attempt number `attempt` (0 is the first retry).
    #[must_use]
    pub fn delay(&self, attempt: u32) -> Duration {
        let doubled = self
            .initial
            .saturating_mul(2u32.saturating_pow(attempt.min(30)));
        let base = doubled.min(self.max);
        base.mul_f64(1.0 - 0.25 * (self.jitter)().clamp(0.0, 1.0))
    }
}

/// A target the sender drives and how it is delivered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderTarget {
    pub id: String,
    pub delivery: Delivery,
}

/// What a sender needs.
#[derive(Clone, Debug)]
pub struct SenderConfig {
    /// Ticks per second: how often the sender looks for a changed frame.
    pub rate_hz: f64,
    /// The instance's targets, by the id frames name them with. A target in a frame that isn't
    /// listed is delivered as streaming.
    pub targets: Vec<SenderTarget>,
    pub policy: DeliveryPolicy,
    pub backoff: Backoff,
    /// How long a release, or closing the connection, may take when the sender stops.
    pub release_timeout: Duration,
}

impl SenderConfig {
    /// The configuration of an instance: its `rate_hz`, lowered to the slowest
    /// `limits.max_rate_hz` among its streaming targets, and the delivery of each of its targets
    /// from their [effective capabilities](effective_capabilities). `targets` are the zones'
    /// targets; those bound to another instance are left out.
    #[must_use]
    pub fn from_instance(
        instance: &ProviderInstance,
        policy: DeliveryPolicy,
        targets: &[Target],
    ) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let mut rate_hz = instance.rate_hz.get() as f64;
        let mut own = Vec::new();
        for target in targets
            .iter()
            .filter(|t| *t.provider.instance == *instance.id)
        {
            let caps = effective_capabilities(&target.capabilities);
            let delivery = Delivery::from(caps.delivery);
            if delivery == Delivery::Streaming && caps.limits.max_rate_hz > 0.0 {
                rate_hz = rate_hz.min(caps.limits.max_rate_hz);
            }
            own.push(SenderTarget {
                id: (*target.id).clone(),
                delivery,
            });
        }
        Self {
            rate_hz,
            targets: own,
            policy,
            backoff: Backoff::default(),
            release_timeout: Duration::from_secs(2),
        }
    }
}

/// A running sender. Dropping it stops the sender like [`shutdown`](Self::shutdown) does, but
/// without waiting.
pub struct SenderHandle {
    frames: watch::Sender<Option<Arc<ElementFrame>>>,
    shutdown: watch::Sender<bool>,
    status: watch::Receiver<ProviderStatus>,
    described: watch::Receiver<Option<Vec<DescribedTarget>>>,
    task: JoinHandle<()>,
}

impl SenderHandle {
    /// Hands the sender the newest frame, which replaces the one before it if that wasn't sent:
    /// nothing queues up. The targets in it are the ones driven; one the next frame leaves out
    /// is released. A frame with no targets releases all of them.
    pub fn submit(&self, frame: ElementFrame) {
        self.frames.send_replace(Some(Arc::new(frame)));
    }

    /// The instance's state, which changes as the connection does.
    #[must_use]
    pub fn status(&self) -> watch::Receiver<ProviderStatus> {
        self.status.clone()
    }

    /// What `describe` found after the latest connection; `None` before the first.
    #[must_use]
    pub fn described(&self) -> watch::Receiver<Option<Vec<DescribedTarget>>> {
        self.described.clone()
    }

    /// Stops the sender: it releases the targets it started (each within `release_timeout`),
    /// closes the control connection and ends. Returns when it has.
    pub async fn shutdown(self) {
        self.shutdown.send_replace(true);
        let _ = self.task.await;
    }
}

/// Starts the sender of an instance. It must run inside a Tokio runtime.
///
/// # Errors
/// When the provider has no [`ElementOutput`].
pub fn spawn_sender(
    provider: Arc<dyn Provider>,
    config: SenderConfig,
) -> Result<SenderHandle, ProviderError> {
    if provider.element_output().is_none() {
        return Err(ProviderError::Failed(
            "the provider has no element output".to_owned(),
        ));
    }
    let (frames, frame_rx) = watch::channel(None);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let (status_tx, status) = watch::channel(ProviderStatus::new(ProviderState::Connecting));
    let (described_tx, described) = watch::channel(None);
    let runner = Runner {
        provider,
        config,
        frames: frame_rx,
        shutdown: shutdown_rx,
        status: status_tx,
        described: described_tx,
        started: HashSet::new(),
        last: HashMap::new(),
    };
    let task = tokio::spawn(runner.run());
    Ok(SenderHandle {
        frames,
        shutdown,
        status,
        described,
        task,
    })
}

/// Tokens for state targets.
struct Bucket {
    capacity: f64,
    tokens: f64,
    per_second: f64,
    last: Instant,
}

impl Bucket {
    fn new(spec: crate::types::BucketSpec, now: Instant) -> Self {
        let capacity = f64::from(spec.capacity.max(1));
        Self {
            capacity,
            tokens: capacity,
            per_second: spec.per_second.max(0.001),
            last: now,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// What ended a connection.
enum Ended {
    Shutdown,
    Lost(ProviderError),
}

struct Runner {
    provider: Arc<dyn Provider>,
    config: SenderConfig,
    frames: watch::Receiver<Option<Arc<ElementFrame>>>,
    shutdown: watch::Receiver<bool>,
    status: watch::Sender<ProviderStatus>,
    described: watch::Sender<Option<Vec<DescribedTarget>>>,
    /// Targets `start` has run for on this connection.
    started: HashSet<String>,
    /// Per target: the wire key and the time of the last send.
    last: HashMap<String, (Vec<u8>, Instant)>,
}

/// Runs `future` unless the sender is told to stop first, or its handle is gone (which counts as
/// being told).
async fn until_shutdown<T>(
    shutdown: &mut watch::Receiver<bool>,
    future: impl Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        _ = shutdown.changed() => None,
        value = future => Some(value),
    }
}

impl Runner {
    fn element(&self) -> &dyn ElementOutput {
        // Checked by `spawn_sender`
        self.provider
            .element_output()
            .expect("a sender needs an element output")
    }

    fn set_status(&self, state: ProviderState, message: Option<String>) {
        self.status.send_replace(ProviderStatus { state, message });
    }

    async fn run(mut self) {
        let mut attempt = 0u32;
        loop {
            if self.status.borrow().state != ProviderState::Error {
                self.set_status(ProviderState::Connecting, None);
            }
            match self.connect().await {
                None => break,
                Some(Err(e)) => {
                    log::warn!("the provider can't connect: {e}");
                    self.set_status(ProviderState::Error, Some(e.message().to_owned()));
                    if self
                        .wait(self.config.backoff.delay(attempt))
                        .await
                        .is_none()
                    {
                        break;
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                Some(Ok(())) => {}
            }
            let connected = Instant::now();
            self.set_status(ProviderState::Connected, None);
            match self.serve().await {
                Ended::Shutdown => break,
                Ended::Lost(e) => {
                    log::warn!("the provider's connection ended: {e}");
                    self.set_status(ProviderState::Error, Some(e.message().to_owned()));
                    if connected.elapsed() >= self.config.backoff.reset_after {
                        attempt = 0;
                    }
                    if self
                        .wait(self.config.backoff.delay(attempt))
                        .await
                        .is_none()
                    {
                        break;
                    }
                    attempt = attempt.saturating_add(1);
                }
            }
        }
        self.stop().await;
    }

    /// Connects and describes. `None` when told to stop meanwhile.
    async fn connect(&mut self) -> Option<Result<(), ProviderError>> {
        let provider = Arc::clone(&self.provider);
        if let Err(e) = until_shutdown(&mut self.shutdown, provider.connect()).await? {
            return Some(Err(e));
        }
        match until_shutdown(&mut self.shutdown, provider.describe()).await? {
            Ok(targets) => {
                self.described.send_replace(Some(targets));
                Some(Ok(()))
            }
            Err(e) => Some(Err(e)),
        }
    }

    /// Sleeps; `None` when told to stop meanwhile.
    async fn wait(&mut self, delay: Duration) -> Option<()> {
        until_shutdown(&mut self.shutdown, tokio::time::sleep(delay)).await
    }

    /// Sends frames until the connection is lost or the sender is told to stop.
    async fn serve(&mut self) -> Ended {
        self.started.clear();
        self.last.clear();
        let period = Duration::from_secs_f64(1.0 / self.config.rate_hz.clamp(0.1, 1000.0));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut bucket = Bucket::new(self.config.policy.bucket, Instant::now());
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.changed() => return Ended::Shutdown,
                _ = ticker.tick() => {}
            }
            let mut shutdown = self.shutdown.clone();
            match until_shutdown(&mut shutdown, self.tick(&mut bucket)).await {
                None => return Ended::Shutdown,
                Some(Err(e)) => return Ended::Lost(e),
                Some(Ok(())) => {}
            }
        }
    }

    fn delivery(&self, target: &str) -> Delivery {
        self.config
            .targets
            .iter()
            .find(|t| t.id == target)
            .map_or(Delivery::Streaming, |t| t.delivery)
    }

    /// One look at the newest frame: release what is no longer driven, start what is new, send
    /// what changed (and what is due a keepalive).
    async fn tick(&mut self, bucket: &mut Bucket) -> Result<(), ProviderError> {
        let frame = self.frames.borrow().clone();
        let driven: HashSet<&str> = frame
            .as_deref()
            .map(|f| f.targets.iter().map(|t| t.target.as_str()).collect())
            .unwrap_or_default();

        let undriven: Vec<String> = self
            .started
            .iter()
            .filter(|id| !driven.contains(id.as_str()))
            .cloned()
            .collect();
        for id in undriven {
            self.started.remove(&id);
            self.last.remove(&id);
            if let Err(e) = self.element().release(&id).await {
                log::warn!("releasing target {id} failed: {e}");
            }
        }
        let Some(frame) = frame else { return Ok(()) };

        let now = Instant::now();
        let mut due: Vec<(TargetOutput, Vec<u8>)> = Vec::new();
        let mut paced = false;
        for output in &frame.targets {
            let id = output.target.as_str();
            if !self.started.contains(id) {
                self.element().start(id).await?;
                self.started.insert(id.to_owned());
            }
            let key = self.element().wire_key(output);
            let previous = self.last.get(id);
            let changed = previous.is_none_or(|(k, _)| *k != key);
            let delivery = self.delivery(id);
            let send = match delivery {
                Delivery::Streaming => {
                    changed
                        || previous.is_some_and(|(_, at)| {
                            now.saturating_duration_since(*at) >= self.config.policy.keepalive
                        })
                }
                Delivery::State => changed && bucket.take(now),
            };
            if send {
                paced |= delivery == Delivery::State;
                due.push((output.clone(), key));
            }
        }
        if due.is_empty() {
            return Ok(());
        }
        let transition = if paced {
            Duration::from_secs_f64(1.0 / self.config.policy.bucket.per_second.max(0.001))
        } else {
            Duration::ZERO
        };
        let (targets, keys): (Vec<_>, Vec<_>) = due.into_iter().unzip();
        let out = ElementFrame {
            timestamp_us: frame.timestamp_us,
            transition,
            targets,
        };
        self.element().send(&out).await?;
        for (output, key) in out.targets.iter().zip(keys) {
            self.last.insert(output.target.clone(), (key, now));
        }
        Ok(())
    }

    /// The end of the sender: release what it started, close the connection.
    async fn stop(&mut self) {
        let limit = self.config.release_timeout;
        let mut ids: Vec<String> = self.started.drain().collect();
        ids.sort();
        for id in ids {
            match tokio::time::timeout(limit, self.element().release(&id)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("releasing target {id} failed: {e}"),
                Err(_) => log::warn!("releasing target {id} timed out"),
            }
        }
        if tokio::time::timeout(limit, self.provider.disconnect())
            .await
            .is_err()
        {
            log::warn!("closing the provider's connection timed out");
        }
        self.set_status(ProviderState::Stopped, None);
    }
}
