//! The controller's output in the node (RB-08, ADR-0036): the output task that steps
//! `folo-controller`'s core when a frame arrives and at least every 1/60 s, one provider sender
//! per enabled instance of a registered kind, and what the live channel shows of it (the
//! `target:<id>` frames, the zones' reactive state and the providers' states).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use folo_controller::{Media, OutputCore, Pattern, PatternError, ZoneLive};
use folo_protocol::frames::{
    FrameError, Pixels, SURFACE_HEADER_LEN, SurfaceFrame, decode_surface, encode_surface,
};
use folo_protocol::generated::{
    ProviderInstance, ProviderInstanceOutput, ProviderSecrets, Target, Zone,
};
use folo_provider::{
    ElementFrame, Provider, ProviderState, ProviderStatus, Registry, SenderConfig, SenderHandle,
    SenderTarget, spawn_sender,
};
use folo_store::Doc;
use folo_supervisor::Stamped;
use folo_supervisor::config::LOCAL_SOURCE;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

use crate::live::ConfigSummary;
use crate::node::Node;
use crate::zones;

/// The output task steps at least this often (ADR-0036 D2).
const STEP_EVERY: Duration = Duration::from_micros(16_667);

/// One running instance: its provider, for administration, and its sender.
struct Running {
    /// What the sender was made from: an instance whose settings or targets change gets a new one.
    key: (Value, Vec<SenderTarget>),
    provider: Arc<dyn Provider>,
    sender: SenderHandle,
}

/// What a configured instance is doing.
enum InstanceState {
    /// Disabled, or not an element output: nothing runs.
    Off,
    /// Its kind has no implementation in this node.
    NotImplemented(String),
    /// Its kind refused to make it.
    Failed(String),
    Running(Box<Running>),
    /// Its sender starts once the ones it had before have stopped.
    Starting(Box<Starting>),
}

/// An instance whose sender starts once every sender it had before has stopped.
struct Starting {
    key: (Value, Vec<SenderTarget>),
    provider: Arc<dyn Provider>,
    config: SenderConfig,
}

/// What the engine's link sent that the output task has taken.
#[derive(Default)]
struct Intake {
    /// The newest update taken.
    update: u64,
    /// The links counter when last looked at.
    links: u64,
    /// An LZ4 frame was logged on this link.
    told_lz4: bool,
}

/// The documents the output was configured from, by identity: the store replaces a document on
/// every change, never mutates it.
#[derive(Default)]
struct Configured {
    zones: Option<Arc<Value>>,
    providers: Option<Arc<Value>>,
    count: u64,
}

/// What the live channel reads: the targets' frames as `FOLS` bytes (without the type byte),
/// each stamped with an update count, and the zones' reactive state.
#[derive(Default)]
pub struct Published {
    pub updates: u64,
    pub targets: HashMap<String, Stamped<Arc<Vec<u8>>>>,
    pub zones: BTreeMap<String, ZoneLive>,
    sequences: HashMap<String, u32>,
}

/// The node's output.
pub struct Output {
    registry: Registry,
    epoch: Instant,
    core: Mutex<OutputCore>,
    configured: Mutex<Configured>,
    instances: Mutex<BTreeMap<String, InstanceState>>,
    /// Senders being stopped, by instance: an instance that changed, went away or was disabled
    /// releases its targets, and a sender of the same instance starts only once they're done.
    retiring: Mutex<HashMap<String, Vec<JoinHandle<()>>>>,
    intake: Mutex<Intake>,
    published: Mutex<Published>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why a test pattern was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum TestRefused {
    UnknownZone,
    UnknownTarget(String),
    /// Another active zone drives these targets (`409 target_busy`).
    Busy(Vec<String>),
}

impl Output {
    /// The output with the provider kinds this node has.
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            epoch: Instant::now(),
            core: Mutex::new(OutputCore::new()),
            configured: Mutex::new(Configured::default()),
            instances: Mutex::new(BTreeMap::new()),
            retiring: Mutex::new(HashMap::new()),
            intake: Mutex::new(Intake::default()),
            published: Mutex::new(Published::default()),
        }
    }

    /// The registry of provider kinds.
    #[must_use]
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The controller's clock: µs since the output was made.
    fn now_us(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    /// Reads what the live channel shows, under the output's lock: keep `read` short.
    pub fn published<R>(&self, read: impl FnOnce(&Published) -> R) -> R {
        read(&locked(&self.published))
    }

    /// The task that steps the output until `stop` changes to true: when a frame arrives from
    /// the engine, and at least every 1/60 s.
    pub async fn run(node: Arc<Node>, mut stop: tokio::sync::watch::Receiver<bool>) {
        let mut arrivals = node.supervisor.arrivals();
        let mut tick = tokio::time::interval(STEP_EVERY);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                changed = arrivals.changed() => if changed.is_err() { break },
                _ = tick.tick() => {}
                _ = stop.wait_for(|stopping| *stopping) => break,
            }
            node.output.step(&node);
        }
    }

    /// One step: the configuration if it changed, what the engine sent, the core's step, the
    /// frames to the senders and the live data.
    pub fn step(&self, node: &Node) {
        let mut core = locked(&self.core);
        self.configure(node, &mut core);
        let now = self.now_us();
        self.take_engine_data(node, &mut core, now);
        let mut out = core.step(now);
        drop(core);
        self.start_ready();
        for (id, state) in locked(&self.instances).iter() {
            if let InstanceState::Running(running) = state {
                // An instance this step leaves out drives nothing: the empty frame releases it
                let frame = out
                    .frames
                    .remove(id)
                    .unwrap_or_else(|| ElementFrame::new(now, Vec::new()));
                running.sender.submit(frame);
            }
        }
        self.publish(now, out.targets, out.zones);
    }

    /// Applies the zones and the providers when either changed: the core's plans and chains,
    /// and the senders.
    fn configure(&self, node: &Node, core: &mut OutputCore) {
        // In the source role the controller's configuration is kept but dormant: nothing runs
        // (docs/architecture/control-plane.md)
        if node.role != "controller" {
            return;
        }
        let zones_doc = node.store.get(Doc::Zones);
        let providers_doc = node.store.get(Doc::Providers);
        let mut configured = locked(&self.configured);
        let same = |held: &Option<Arc<Value>>, now: &Arc<Value>| {
            held.as_ref().is_some_and(|held| Arc::ptr_eq(held, now))
        };
        if same(&configured.zones, &zones_doc) && same(&configured.providers, &providers_doc) {
            return;
        }
        let zones_changed = !same(&configured.zones, &zones_doc);
        configured.zones = Some(Arc::clone(&zones_doc));
        configured.providers = Some(Arc::clone(&providers_doc));
        configured.count += 1;
        let effective = zones::with_effective(&zones_doc);
        let zones: Vec<Zone> = effective["zones"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|zone| serde_json::from_value(zone.clone()).ok())
            .collect();
        if zones_changed {
            for (zone, issue) in core.configure(configured.count, &zones) {
                if issue.is_error() {
                    log::warn!("zone {zone}: {}: {} (skipped)", issue.path, issue.message);
                }
            }
            // A pattern ends when another active zone now drives its target, on overlapping
            // addresses too (the core ends those that share the id)
            for (zone, target) in core.patterns() {
                if zones::busy(&zones_doc, &zone, &target) {
                    core.stop_pattern(&target);
                }
            }
        }
        drop(configured);
        self.reconcile(node, &providers_doc, &zones);
    }

    /// Starts, keeps or stops each instance's sender to match the documents.
    fn reconcile(&self, node: &Node, providers: &Value, zones: &[Zone]) {
        let mut targets: Vec<Target> = Vec::new();
        for target in zones.iter().flat_map(|z| &z.targets) {
            if !targets.iter().any(|t| t.id == target.id) {
                targets.push(target.clone());
            }
        }
        let instances: Vec<(Value, Option<ProviderInstance>)> = providers["providers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| (p.clone(), folo_provider::instance_of(p).ok()))
            .collect();
        let mut secrets: Option<Value> = None;
        let mut held = locked(&self.instances);
        let mut before = std::mem::take(&mut *held);
        for (raw, instance) in instances {
            let Some(instance) = instance else { continue };
            let id = instance.id.as_str().to_owned();
            let state = if !instance.enabled || instance.output != ProviderInstanceOutput::Element {
                InstanceState::Off
            } else if !self.registry.is_implemented(instance.kind) {
                InstanceState::NotImplemented(folo_provider::not_implemented_message(instance.kind))
            } else {
                let previous = before.remove(&id);
                let secrets = secrets.get_or_insert_with(|| {
                    folo_store::read_secrets(node.store.dir()).unwrap_or_default()
                });
                self.instance_state(&instance, raw, &targets, secrets, previous)
            };
            held.insert(id, state);
        }
        // Instances that went away, or stopped running, release their targets
        for (id, state) in before {
            if let InstanceState::Running(running) = state {
                self.retire(&id, running.sender);
            }
        }
    }

    /// Stops a sender on a task of its own: it releases its targets and disconnects, and the
    /// instance's next sender waits for it.
    fn retire(&self, id: &str, sender: SenderHandle) {
        locked(&self.retiring)
            .entry(id.to_owned())
            .or_default()
            .push(tokio::spawn(sender.shutdown()));
    }

    /// An enabled instance of a registered kind: its running sender kept when nothing it was
    /// made from changed, or a new one to start. A sender that is replaced is stopped on a task
    /// of its own, and its successor starts only once it has, so its releases can't undo what
    /// the successor starts ([`start_ready`](Self::start_ready)). The provider is kept when only
    /// the targets changed.
    fn instance_state(
        &self,
        instance: &ProviderInstance,
        raw: Value,
        targets: &[Target],
        secrets: &Value,
        previous: Option<InstanceState>,
    ) -> InstanceState {
        let id = instance.id.as_str();
        let (old_key, old_provider, old_sender) = match previous {
            Some(InstanceState::Running(running)) => {
                let Running {
                    key,
                    provider,
                    sender,
                } = *running;
                (Some(key), Some(provider), Some(sender))
            }
            Some(InstanceState::Starting(starting)) => {
                (Some(starting.key), Some(starting.provider), None)
            }
            _ => (None, None, None),
        };
        let same_instance = old_key.as_ref().is_some_and(|(held, _)| *held == raw);
        let provider = if let Some(provider) = old_provider.filter(|_| same_instance) {
            provider
        } else {
            let own: Option<ProviderSecrets> =
                serde_json::from_value(secrets["providers"][id].clone()).ok();
            match self.registry.create(instance, own.as_ref()) {
                Ok(provider) => provider,
                Err(error) => {
                    log::warn!("provider {id}: {error}");
                    if let Some(sender) = old_sender {
                        self.retire(id, sender);
                    }
                    return InstanceState::Failed(error.to_string());
                }
            }
        };
        let config = SenderConfig::from_instance(instance, provider.delivery_policy(), targets);
        let key = (raw, config.targets.clone());
        // Only a running sender is kept, and only when nothing it was made from changed
        if let Some(sender) = old_sender {
            if old_key.as_ref() == Some(&key) {
                return InstanceState::Running(Box::new(Running {
                    key,
                    provider,
                    sender,
                }));
            }
            self.retire(id, sender);
        }
        InstanceState::Starting(Box::new(Starting {
            key,
            provider,
            config,
        }))
    }

    /// Starts the senders of the instances whose earlier senders have all stopped, and forgets
    /// the stops that are done. It never waits: a slow device delays only its own instance.
    fn start_ready(&self) {
        // instances before retiring, as `reconcile` takes them
        let mut instances = locked(&self.instances);
        let mut retiring = locked(&self.retiring);
        retiring.retain(|_, stops| {
            stops.retain(|stop| !stop.is_finished());
            !stops.is_empty()
        });
        for (id, state) in instances.iter_mut() {
            let InstanceState::Starting(starting) = state else {
                continue;
            };
            if retiring.contains_key(id) {
                continue;
            }
            *state = match spawn_sender(Arc::clone(&starting.provider), starting.config.clone()) {
                Ok(sender) => InstanceState::Running(Box::new(Running {
                    key: starting.key.clone(),
                    provider: Arc::clone(&starting.provider),
                    sender,
                })),
                Err(error) => {
                    log::warn!("provider {id}: {error}");
                    InstanceState::Failed(error.to_string())
                }
            };
        }
    }

    /// Takes the frames, the activity and the now playing the engine sent since the last step.
    fn take_engine_data(&self, node: &Node, core: &mut OutputCore, now: u64) {
        let mut intake = locked(&self.intake);
        let links = *node.supervisor.links().borrow();
        if links != intake.links {
            // A link made or ended: what the engine sent before is gone
            intake.links = links;
            intake.told_lz4 = false;
            core.forget_source(LOCAL_SOURCE);
        }
        if !node.supervisor.linked() {
            return;
        }
        let summary = node
            .supervisor
            .with_config(|config| config.map(ConfigSummary::of).unwrap_or_default());
        let since = intake.update;
        let (frames, activity, media, newest) = node.supervisor.received(|received| {
            let frames: Vec<(u16, Arc<Vec<u8>>)> = received
                .frames
                .iter()
                .filter(|((kind, _), f)| *kind == SURFACE_FRAME && f.update > since)
                .map(|((_, index), f)| (*index, Arc::clone(&f.value)))
                .collect();
            let newer = |method: &str| {
                received
                    .notifications
                    .get(method)
                    .filter(|n| n.update > since)
                    .map(|n| n.value.clone())
            };
            (frames, newer("activity"), newer("media"), received.updates)
        });
        // A configuration published while the frames were read may have renumbered them:
        // nothing is taken, and the next step reads them again under the new numbering
        let moved = node
            .supervisor
            .with_config(|config| config.map(ConfigSummary::of).unwrap_or_default())
            != summary;
        if moved {
            return;
        }
        intake.update = newest;
        for (index, bytes) in frames {
            let Some(surface) = summary.surfaces.get(usize::from(index)) else {
                continue;
            };
            match decode_surface(&bytes) {
                Ok(frame) => {
                    if let Err(refused) = core.frame(surface, LOCAL_SOURCE, &frame, now) {
                        log::debug!("dropped a frame of surface {surface}: {refused:?}");
                    }
                }
                Err(FrameError::Lz4Unsupported) if !intake.told_lz4 => {
                    intake.told_lz4 = true;
                    log::warn!("dropped LZ4 frames from the effect engine: LZ4 arrives with RB-16");
                }
                Err(error) => log::debug!("dropped a frame of surface {surface}: {error:?}"),
            }
        }
        if let Some(activity) = activity {
            let surface = activity["surface"]
                .as_u64()
                .and_then(|i| summary.surfaces.get(usize::try_from(i).ok()?));
            let active = activity["active"].as_bool().unwrap_or(false);
            core.activity(LOCAL_SOURCE, surface.map(String::as_str), active, now);
        }
        if let Some(media) = media {
            let state = media["state"].as_str().unwrap_or_default();
            core.media(LOCAL_SOURCE, Media::from_state(state));
        }
    }

    /// Keeps the step's target frames, as `FOLS` bytes, and the zones' state for the live channel.
    fn publish(
        &self,
        now: u64,
        targets: BTreeMap<String, Vec<u8>>,
        zones: BTreeMap<String, ZoneLive>,
    ) {
        let mut published = locked(&self.published);
        published.zones = zones;
        published.targets.retain(|id, _| targets.contains_key(id));
        for (id, bytes) in targets {
            let unchanged = published
                .targets
                .get(&id)
                .is_some_and(|held| held_pixels(&held.value) == bytes.as_slice());
            if unchanged {
                continue;
            }
            let sequence = published.sequences.entry(id.clone()).or_insert(0);
            *sequence = sequence.wrapping_add(1);
            let width = u16::try_from(bytes.len() / 3).unwrap_or(u16::MAX);
            let frame = SurfaceFrame::new(0, *sequence, now, width, 1, Pixels::Rgb8(bytes));
            let Ok(encoded) = encode_surface(&frame) else {
                continue;
            };
            published.updates += 1;
            let update = published.updates;
            published.targets.insert(
                id,
                Stamped {
                    update,
                    value: Arc::new(encoded),
                },
            );
        }
    }

    /// Runs a test pattern on `target` of `zone` (all of its targets when empty) for `seconds`.
    ///
    /// # Errors
    /// [`TestRefused`]: an unknown zone or target, or targets another active zone drives.
    pub fn test(
        &self,
        node: &Node,
        zone: &str,
        target: &str,
        pattern: Pattern,
        seconds: f64,
    ) -> Result<(), TestRefused> {
        let mut core = locked(&self.core);
        // The core knows the zones as they are now, a zone written a moment ago too
        self.configure(node, &mut core);
        let wanted: Vec<String> = if target.is_empty() {
            Vec::new()
        } else {
            vec![target.to_owned()]
        };
        let document = node.store.get(Doc::Zones);
        let now = self.now_us();
        let chosen = core
            .pattern_targets(zone, &wanted)
            .map_err(|error| match error {
                PatternError::UnknownZone => TestRefused::UnknownZone,
                PatternError::UnknownTarget(t) => TestRefused::UnknownTarget(t),
            })?;
        let busy: Vec<String> = chosen
            .iter()
            .filter(|t| zones::busy(&document, zone, t))
            .cloned()
            .collect();
        if !busy.is_empty() {
            return Err(TestRefused::Busy(busy));
        }
        core.start_pattern(zone, &chosen, pattern, seconds, now)
            .map(|_| ())
            .map_err(|_| TestRefused::UnknownZone)
    }

    /// The provider of a running instance, for administration; or why there is none. The
    /// output applies the documents first, so an instance written a moment ago has its provider.
    pub(crate) fn provider(
        &self,
        node: &Node,
        id: &str,
    ) -> Result<Arc<dyn Provider>, ProviderAbsent> {
        self.configure(node, &mut locked(&self.core));
        match locked(&self.instances).get(id) {
            Some(InstanceState::Running(running)) => Ok(Arc::clone(&running.provider)),
            Some(InstanceState::Starting(starting)) => Ok(Arc::clone(&starting.provider)),
            Some(InstanceState::Failed(message)) => Err(ProviderAbsent::Failed(message.clone())),
            Some(InstanceState::NotImplemented(_)) => Err(ProviderAbsent::NotImplemented),
            Some(InstanceState::Off) => Err(ProviderAbsent::Off),
            None => Err(ProviderAbsent::Unknown),
        }
    }

    /// An instance's state and message, as the senders keep them.
    #[must_use]
    pub fn provider_status(&self, id: &str) -> ProviderStatus {
        match locked(&self.instances).get(id) {
            Some(InstanceState::Running(running)) => running.sender.status().borrow().clone(),
            Some(InstanceState::Starting(_)) => ProviderStatus::new(ProviderState::Connecting),
            Some(InstanceState::NotImplemented(message)) => ProviderStatus {
                state: ProviderState::Stopped,
                message: Some(message.clone()),
            },
            Some(InstanceState::Failed(message)) => ProviderStatus {
                state: ProviderState::Error,
                message: Some(message.clone()),
            },
            Some(InstanceState::Off) | None => ProviderStatus::new(ProviderState::Stopped),
        }
    }

    /// Stops every sender: each releases the targets it started and closes its connection,
    /// all at once, so the whole takes at most what the slowest instance's stop takes (three
    /// times its `release_timeout`).
    pub async fn shutdown(&self) {
        let instances = std::mem::take(&mut *locked(&self.instances));
        for (id, state) in instances {
            if let InstanceState::Running(running) = state {
                self.retire(&id, running.sender);
            }
        }
        let stopping: Vec<JoinHandle<()>> = std::mem::take(&mut *locked(&self.retiring))
            .into_values()
            .flatten()
            .collect();
        for stop in stopping {
            let _ = stop.await;
        }
    }
}

/// Why an instance has no provider to administer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProviderAbsent {
    /// Not in the providers document.
    Unknown,
    /// Disabled.
    Off,
    NotImplemented,
    Failed(String),
}

/// The type byte of surface frames on the link.
const SURFACE_FRAME: u8 = 0x02;
/// A published target frame's pixels.
fn held_pixels(frame: &[u8]) -> &[u8] {
    frame.get(SURFACE_HEADER_LEN..).unwrap_or_default()
}
