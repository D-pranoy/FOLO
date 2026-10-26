//! The sender: one task per provider instance (`docs/architecture/providers.md`, the sender).
//!
//! It keeps the instance connected (with backoff), describes it after each connection (and again
//! every `describe_every`, if set), starts a target when it is first driven, sends only the
//! frames that changed plus a keepalive (or, for state targets, paced updates from a token
//! bucket), and releases targets nothing drives any more. Which targets are driven is what the
//! newest submitted frame holds. Every call of the provider has a time limit.

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
    /// A target whose `start` the provider refused (or doesn't know) is tried again after this
    /// long; the other targets carry on meanwhile.
    pub start_retry: Duration,
    /// Describe the connected instance again this often, for devices whose layout can change
    /// (Hyperion's LED count). `None` describes once per connection.
    pub describe_every: Option<Duration>,
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
            start_retry: Duration::from_secs(5),
            describe_every: None,
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
    ///
    /// The sender repeats the newest frame as a keepalive for as long as it holds it, so a
    /// producer that has stopped (a crashed engine, a stale source) must say so with an empty
    /// frame: staleness is the caller's to detect.
    pub fn submit(&self, frame: ElementFrame) {
        self.frames.send_replace(Some(Arc::new(frame)));
    }

    /// The instance's state, which changes as the connection does.
    #[must_use]
    pub fn status(&self) -> watch::Receiver<ProviderStatus> {
        self.status.clone()
    }

    /// What `describe` found last (after the latest connection, or the latest `describe_every`
    /// round); `None` before the first.
    #[must_use]
    pub fn described(&self) -> watch::Receiver<Option<Vec<DescribedTarget>>> {
        self.described.clone()
    }

    /// Stops the sender: it releases the targets it started, closes the control connection and
    /// ends. A call in progress is cancelled where it awaits, except that a target whose `start`
    /// was cancelled still counts as started, so it is released. Each release and the close take
    /// at most `release_timeout`, and the whole stop three times that. Returns when it has.
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
        failed: HashMap::new(),
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
    /// Targets `start` has been called for on this connection, including one whose `start` was
    /// cancelled or failed: they may be running, so they are released. A target leaves the set
    /// only when its release has finished.
    started: HashSet<String>,
    /// Targets whose `start` was refused, and when to try them again.
    failed: HashMap<String, Instant>,
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

/// A provider call under its time limit: running over it is `Unreachable`.
async fn limited<T>(
    limit: Duration,
    what: &str,
    future: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    match tokio::time::timeout(limit, future).await {
        Ok(result) => result,
        Err(_) => Err(ProviderError::Unreachable(format!("{what} timed out"))),
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

    fn lost(&mut self) {
        // The connection is gone, and with it what ran on it: there is nothing to release
        self.started.clear();
        self.failed.clear();
        self.last.clear();
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
                    self.lost();
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
        let limit = self.config.policy.call_timeout;
        let connected = limited(limit, "connecting", provider.connect());
        if let Err(e) = until_shutdown(&mut self.shutdown, connected).await? {
            return Some(Err(e));
        }
        let described = limited(limit, "describing", provider.describe());
        match until_shutdown(&mut self.shutdown, described).await? {
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
        self.failed.clear();
        self.last.clear();
        let period = Duration::from_secs_f64(1.0 / self.config.rate_hz.clamp(0.1, 1000.0));
        let mut ticker = tokio::time::interval(period);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut bucket = Bucket::new(self.config.policy.bucket, Instant::now());
        let mut describe_at = self.config.describe_every.map(|d| Instant::now() + d);
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.changed() => return Ended::Shutdown,
                _ = ticker.tick() => {}
            }
            let mut shutdown = self.shutdown.clone();
            match until_shutdown(&mut shutdown, self.tick(&mut bucket, &mut describe_at)).await {
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

    /// Describes again when it is due.
    async fn redescribe(&mut self, describe_at: &mut Option<Instant>) -> Result<(), ProviderError> {
        let (Some(at), Some(every)) = (*describe_at, self.config.describe_every) else {
            return Ok(());
        };
        if Instant::now() < at {
            return Ok(());
        }
        *describe_at = Some(Instant::now() + every);
        let limit = self.config.policy.call_timeout;
        match limited(limit, "describing", self.provider.describe()).await {
            Ok(targets) => {
                self.described.send_replace(Some(targets));
                Ok(())
            }
            Err(e @ ProviderError::Unreachable(_)) => Err(e),
            Err(e) => {
                log::warn!("describing the provider again failed: {e}");
                Ok(())
            }
        }
    }

    /// One look at the newest frame: release what is no longer driven, start what is new, send
    /// what changed (and what is due a keepalive).
    ///
    /// A shutdown may cancel this at any await, so `started` is kept true to what may be
    /// running: a target goes in before its `start` and out after its release.
    async fn tick(
        &mut self,
        bucket: &mut Bucket,
        describe_at: &mut Option<Instant>,
    ) -> Result<(), ProviderError> {
        let limit = self.config.policy.call_timeout;
        self.redescribe(describe_at).await?;

        let frame = self.frames.borrow().clone();
        let driven: HashSet<&str> = frame
            .as_deref()
            .map(|f| f.targets.iter().map(|t| t.target.as_str()).collect())
            .unwrap_or_default();

        self.failed.retain(|id, _| driven.contains(id.as_str()));
        let undriven: Vec<String> = self
            .started
            .iter()
            .filter(|id| !driven.contains(id.as_str()))
            .cloned()
            .collect();
        for id in undriven {
            let released = limited(limit, "releasing", self.element().release(&id)).await;
            if let Err(e) = released {
                log::warn!("releasing target {id} failed: {e}");
            }
            self.started.remove(&id);
            self.last.remove(&id);
        }
        let Some(frame) = frame else { return Ok(()) };

        // Starts first, so the time below isn't stale after a slow one
        let mut running: HashSet<&str> = HashSet::new();
        for output in &frame.targets {
            let id = output.target.as_str();
            if self.started.contains(id) {
                running.insert(id);
                continue;
            }
            if self
                .failed
                .get(id)
                .is_some_and(|retry_at| Instant::now() < *retry_at)
            {
                continue;
            }
            self.started.insert(id.to_owned());
            match limited(limit, "starting", self.element().start(id)).await {
                Ok(()) => {
                    self.failed.remove(id);
                    running.insert(id);
                }
                // The connection is the trouble: all targets start again on the next one
                Err(e @ ProviderError::Unreachable(_)) => return Err(e),
                // This target is: carry on with the others, and try it again later
                Err(e) => {
                    log::warn!("starting target {id} failed: {e}");
                    self.started.remove(id);
                    self.failed
                        .insert(id.to_owned(), Instant::now() + self.config.start_retry);
                }
            }
        }

        let now = Instant::now();
        let mut due: Vec<(TargetOutput, Vec<u8>)> = Vec::new();
        let mut paced = false;
        for output in &frame.targets {
            let id = output.target.as_str();
            if !running.contains(id) {
                continue;
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
        limited(limit, "sending", self.element().send(&out)).await?;
        for (output, key) in out.targets.iter().zip(keys) {
            self.last.insert(output.target.clone(), (key, now));
        }
        Ok(())
    }

    /// The end of the sender: release what it started, close the connection. Each call takes at
    /// most `release_timeout`, and the whole stop three times that.
    async fn stop(&mut self) {
        let limit = self.config.policy.release_timeout;
        let deadline = Instant::now() + limit * 3;
        let mut ids: Vec<String> = self.started.drain().collect();
        ids.sort();
        for id in ids {
            let until = (Instant::now() + limit).min(deadline);
            match tokio::time::timeout_at(until, self.element().release(&id)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("releasing target {id} failed: {e}"),
                Err(_) => log::warn!("releasing target {id} timed out"),
            }
        }
        let until = (Instant::now() + limit).min(deadline);
        if tokio::time::timeout_at(until, self.provider.disconnect())
            .await
            .is_err()
        {
            log::warn!("closing the provider's connection timed out");
        }
        self.set_status(ProviderState::Stopped, None);
    }
}
