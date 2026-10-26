//! The output core: per zone, the compositor and the ambient light, projection, the adapter
//! chains and the test patterns, stepped on the controller's clock.

use std::collections::{BTreeMap, HashMap};

use folo_adapters::{AdapterChain, delay_ms};
use folo_compositor::{
    AmbientSettings, Compositor, FrameOutcome, FrameRefused, SurfaceSettings, ZoneAmbient,
};
use folo_geometry::{
    CpuProjector, Issue, Layout, ProjectError, Projector, Shape, SurfaceView, element_layout,
    published_size,
};
use folo_protocol::frames::SurfaceFrame;
use folo_protocol::generated::{Bounds, Zone};
use folo_provider::{ElementFrame, TargetOutput, effective_capabilities};
use serde_json::{Value, json};

use crate::pattern::Pattern;

/// What a source's now playing says, for the zone's pause (ADR-0036 D8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    /// No media session: `off`, `unavailable`, `error` or `none`.
    NoSession,
    Playing,
    Paused,
    Stopped,
}

impl Media {
    /// From a `media` notification's `state`.
    #[must_use]
    pub fn from_state(state: &str) -> Self {
        match state {
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            "stopped" => Self::Stopped,
            _ => Self::NoSession,
        }
    }
}

/// A zone's reactive state, as the live channel reports it (`ZoneLive`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneLive {
    /// Effectively active.
    pub active: bool,
    /// The ambient fade: 0 reactive … 1 ambient light.
    pub ambient: f64,
    /// Seconds of silence, on the controller's clock.
    pub quiet_s: f64,
}

impl ZoneLive {
    const INACTIVE: Self = Self {
        active: false,
        ambient: 0.0,
        quiet_s: 0.0,
    };
}

/// What one step produced.
#[derive(Clone, Debug, Default)]
pub struct StepOutput {
    /// Per provider instance, the colours of the targets it drives now. An instance that isn't
    /// here drives nothing, and its sender gets an empty frame, which releases its targets.
    pub frames: BTreeMap<String, ElementFrame>,
    /// Per driven target, its projected colours after the ambient light and before the adapters,
    /// or its test pattern, as `rgb8` (the `target:<id>` topic, ADR-0036 D19).
    pub targets: BTreeMap<String, Vec<u8>>,
    /// Every zone's reactive state.
    pub zones: BTreeMap<String, ZoneLive>,
}

/// Why a test pattern can't start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternError {
    UnknownZone,
    /// The zone has no such target.
    UnknownTarget(String),
}

/// A pattern running on a target.
#[derive(Clone, Debug)]
struct Running {
    zone: String,
    pattern: Pattern,
    started_us: u64,
    until_us: u64,
}

/// A surface of a zone as the compositor composes it.
struct SurfaceSlot {
    id: String,
    master: f64,
    sources: Vec<String>,
}

/// One target of a zone: its chain and what it showed last.
struct TargetSlot {
    id: String,
    /// The provider instance it is bound to; empty when none.
    instance: String,
    layout: Layout,
    /// What the chain was made from: a chain whose settings stay keeps its delay line and its
    /// flash limiter's history across configurations, so an edit elsewhere can't reset the limiter.
    settings: Value,
    chain: AdapterChain,
    /// The projected colours of the last step that projected it. A frame of another size than
    /// the plans were made for leaves them as they were (`docs/architecture/spatial-model.md`).
    projected: Vec<[f32; 3]>,
}

struct ZoneOutput {
    id: String,
    active: bool,
    bounds: Bounds,
    compositor: Compositor,
    ambient: ZoneAmbient,
    surfaces: Vec<SurfaceSlot>,
    targets: Vec<TargetSlot>,
}

impl ZoneOutput {
    /// The zone's output for this configuration, keeping what `before` had that still applies:
    /// the compositor's frames and fades, the ambient light's state, and the chains of targets
    /// whose settings didn't change.
    fn new(zone: &Zone, before: Option<Self>) -> Self {
        let settings: Vec<SurfaceSettings> = zone
            .surfaces
            .iter()
            .map(|surface| {
                let (width, height) = published_size(surface);
                let cells = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
                SurfaceSettings::from_surface(surface, cells(width), cells(height))
            })
            .collect();
        let surfaces = settings
            .iter()
            .map(|s| SurfaceSlot {
                id: s.id.clone(),
                master: s.master,
                sources: s.sources.clone(),
            })
            .collect();
        let ambient_settings = AmbientSettings::from_zone(zone);
        let comes_on = zone.active && before.as_ref().is_some_and(|b| !b.active);
        let (mut compositor, ambient, mut kept) = match before {
            Some(before) => {
                let mut ambient = before.ambient;
                // a zone that comes on starts reactive, with its silence counted from now
                if comes_on {
                    ambient = ZoneAmbient::new(ambient_settings);
                } else {
                    ambient.configure(ambient_settings);
                }
                (before.compositor, ambient, before.targets)
            }
            None => (
                Compositor::new(),
                ZoneAmbient::new(ambient_settings),
                Vec::new(),
            ),
        };
        compositor.configure(settings);
        let slowest = zone
            .targets
            .iter()
            .map(|t| effective_capabilities(&t.capabilities).limits.latency_ms)
            .fold(0.0, f64::max);
        let limiter = zone.flash_limiter.enabled;
        let targets = zone
            .targets
            .iter()
            .map(|target| {
                let delay = delay_ms(target, slowest);
                let settings = json!([
                    target.calibration,
                    target.capabilities,
                    target.provider.address.count,
                    delay,
                    limiter,
                ]);
                let layout = element_layout(target);
                let before = kept
                    .iter()
                    .position(|t| t.id == *target.id)
                    .map(|at| kept.swap_remove(at));
                let (mut chain, projected) = match before {
                    Some(t) if t.settings == settings => (t.chain, t.projected),
                    // new settings: a new chain, which carries on with the old one's flashes
                    Some(mut t) => {
                        let mut chain = AdapterChain::new(target, limiter, delay);
                        chain.inherit_limiter(&mut t.chain);
                        (chain, t.projected)
                    }
                    None => (AdapterChain::new(target, limiter, delay), Vec::new()),
                };
                let projected = if projected.len() == layout.len() {
                    projected
                } else {
                    vec![[0.0; 3]; layout.len()]
                };
                // frames from before the zone went off mustn't come out of the delay line
                if comes_on {
                    chain.clear_delay();
                }
                TargetSlot {
                    id: target.id.as_str().to_owned(),
                    instance: target.provider.instance.as_str().to_owned(),
                    layout,
                    settings,
                    chain,
                    projected,
                }
            })
            .collect();
        Self {
            id: zone.id.as_str().to_owned(),
            active: zone.active,
            bounds: zone.bounds.clone(),
            compositor,
            ambient,
            surfaces,
            targets,
        }
    }

    /// Whether the zone's players are paused: at least one linked source has a media session
    /// and all of those say `paused` (ADR-0036 D8).
    fn paused(&self, media: &HashMap<String, Media>) -> bool {
        let mut sessions = self
            .surfaces
            .iter()
            .flat_map(|s| &s.sources)
            .filter_map(|source| media.get(source))
            .filter(|m| **m != Media::NoSession)
            .peekable();
        sessions.peek().is_some() && sessions.all(|m| *m == Media::Paused)
    }
}

/// The controller's output: what every zone's targets show at each step.
///
/// The node calls [`step`](Self::step) when a frame arrives and at least every 1/60 s
/// (ADR-0036 D2); nothing here reads a clock, so tests step it with their own.
#[derive(Default)]
pub struct OutputCore {
    revision: Option<u64>,
    projector: CpuProjector,
    zones: Vec<ZoneOutput>,
    patterns: HashMap<String, Running>,
    media: HashMap<String, Media>,
    /// The zone that last drove each target, so a zone taking a target over takes its flash
    /// limiter's history too.
    drivers: HashMap<String, String>,
}

impl OutputCore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the zones of configuration `revision`, whose `active` is the **effective** state.
    /// The same revision is the same configuration and changes nothing; another rebuilds the
    /// gather plans and keeps what still applies. Returns what validation found in the zones it
    /// planned; mappings with errors are skipped.
    pub fn configure(&mut self, revision: u64, zones: &[Zone]) -> Vec<(String, Issue)> {
        if self.revision == Some(revision) {
            return Vec::new();
        }
        self.revision = Some(revision);
        let mut before: HashMap<String, ZoneOutput> =
            self.zones.drain(..).map(|z| (z.id.clone(), z)).collect();
        let mut issues = Vec::new();
        for zone in zones {
            let prepared = self.projector.prepare(revision, zone);
            issues.extend(
                prepared
                    .issues
                    .into_iter()
                    .map(|issue| (zone.id.as_str().to_owned(), issue)),
            );
            let id = zone.id.as_str();
            self.zones.push(ZoneOutput::new(zone, before.remove(id)));
        }
        for gone in before.keys() {
            self.projector.forget(gone);
        }
        // a pattern ends with its zone or target, and when another active zone now drives its
        // target (overlapping addresses under another id are the caller's to end: `stop_pattern`)
        let zones = &self.zones;
        self.patterns.retain(|target, running| {
            let has = |z: &ZoneOutput| z.targets.iter().any(|t| t.id == *target);
            zones.iter().any(|z| z.id == running.zone && has(z))
                && !zones
                    .iter()
                    .any(|z| z.id != running.zone && z.active && has(z))
        });
        issues
    }

    /// The running patterns: their zone and target.
    #[must_use]
    pub fn patterns(&self) -> Vec<(String, String)> {
        self.patterns
            .iter()
            .map(|(target, running)| (running.zone.clone(), target.clone()))
            .collect()
    }

    /// Ends the pattern on `target`, if one runs.
    pub fn stop_pattern(&mut self, target: &str) {
        self.patterns.remove(target);
    }

    /// The first effectively active zone with the surface `surface`, which is the one the
    /// engine renders it for (`folo-supervisor`'s `local_config`).
    #[must_use]
    pub fn zone_of_surface(&self, surface: &str) -> Option<&str> {
        self.zones
            .iter()
            .find(|z| z.active && z.surfaces.iter().any(|s| s.id == surface))
            .map(|z| z.id.as_str())
    }

    /// Takes a source's frame for a surface of the active zone that has it. `now_us` is when it
    /// arrived.
    ///
    /// # Errors
    /// [`FrameRefused`] when no active zone has the surface, the surface doesn't list the
    /// source, or the frame isn't the surface's published size.
    pub fn frame(
        &mut self,
        surface: &str,
        source: &str,
        frame: &SurfaceFrame,
        now_us: u64,
    ) -> Result<FrameOutcome, FrameRefused> {
        let zone = self
            .zones
            .iter_mut()
            .find(|z| z.active && z.surfaces.iter().any(|s| s.id == surface))
            .ok_or(FrameRefused::UnknownSurface)?;
        zone.compositor.frame(surface, source, frame, now_us)
    }

    /// Takes a source's `activity` report: `surface` is the notification's, `None` for the
    /// whole source.
    pub fn activity(&mut self, source: &str, surface: Option<&str>, active: bool, now_us: u64) {
        for zone in &mut self.zones {
            zone.compositor.activity(source, surface, active, now_us);
        }
    }

    /// Takes a source's now playing.
    pub fn media(&mut self, source: &str, media: Media) {
        self.media.insert(source.to_owned(), media);
    }

    /// Forgets what a source sent: its link dropped or its engine restarted, so its last frames
    /// mustn't stay on the lights while they go stale.
    pub fn forget_source(&mut self, source: &str) {
        for zone in &mut self.zones {
            zone.compositor.forget_source(source);
        }
        self.media.remove(source);
    }

    /// The targets a pattern on `targets` of `zone` would run on: those, or all of the zone's
    /// when `targets` is empty.
    ///
    /// # Errors
    /// [`PatternError`] for a zone or target that isn't configured.
    pub fn pattern_targets(
        &self,
        zone: &str,
        targets: &[String],
    ) -> Result<Vec<String>, PatternError> {
        let found = self
            .zones
            .iter()
            .find(|z| z.id == zone)
            .ok_or(PatternError::UnknownZone)?;
        // display targets are validated, never executed (ADR-0036 D12)
        let runs = |t: &&TargetSlot| t.layout.shape() != Shape::Display;
        if targets.is_empty() {
            return Ok(found
                .targets
                .iter()
                .filter(runs)
                .map(|t| t.id.clone())
                .collect());
        }
        for id in targets {
            if !found.targets.iter().filter(runs).any(|t| t.id == *id) {
                return Err(PatternError::UnknownTarget(id.clone()));
            }
        }
        Ok(targets.to_vec())
    }

    /// Runs `pattern` on `targets` of `zone` (all of its targets when empty) for `seconds`,
    /// replacing a pattern they run, and returns the targets. Whether another active zone drives
    /// a target is the caller's check (`409 target_busy`).
    ///
    /// # Errors
    /// [`PatternError`] for a zone or target that isn't configured; nothing starts then.
    pub fn start_pattern(
        &mut self,
        zone: &str,
        targets: &[String],
        pattern: Pattern,
        seconds: f64,
        now_us: u64,
    ) -> Result<Vec<String>, PatternError> {
        let chosen = self.pattern_targets(zone, targets)?;
        // 1–60 s by the contract: far below 2^64 µs
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let until_us = now_us.saturating_add((seconds.clamp(0.0, 60.0) * 1e6) as u64);
        for id in &chosen {
            self.patterns.insert(
                id.clone(),
                Running {
                    zone: zone.to_owned(),
                    pattern,
                    started_us: now_us,
                    until_us,
                },
            );
        }
        Ok(chosen)
    }

    /// The step at `now_us`: every active zone composed, the ambient light blended, projected
    /// and adapted; the running patterns in place of their targets' colours.
    pub fn step(&mut self, now_us: u64) -> StepOutput {
        self.patterns.retain(|_, running| now_us < running.until_us);
        self.hand_over();
        let mut out = StepOutput::default();
        let mut outputs: BTreeMap<String, Vec<TargetOutput>> = BTreeMap::new();
        let mut buffer = Vec::new();
        for zone in &mut self.zones {
            let paused = zone.paused(&self.media);
            if zone.active {
                let composed = {
                    let live = zone
                        .surfaces
                        .iter()
                        .any(|s| zone.compositor.is_live(&s.id, now_us));
                    zone.ambient.step_to(now_us, live, paused);
                    compose_surfaces(zone, now_us)
                };
                let views: Vec<(&str, SurfaceView<'_>)> = composed
                    .iter()
                    .filter_map(|(id, width, height, pixels)| {
                        let cells = pixels.as_chunks::<3>().0;
                        SurfaceView::new(usize::from(*width), usize::from(*height), cells)
                            .ok()
                            .map(|view| (id.as_str(), view))
                    })
                    .collect();
                for target in &mut zone.targets {
                    if self.patterns.contains_key(&target.id)
                        || target.layout.shape() == Shape::Display
                    {
                        continue;
                    }
                    buffer.clear();
                    buffer.resize(target.projected.len(), [0.0; 3]);
                    match self
                        .projector
                        .project(&zone.id, &target.id, &views, &mut buffer)
                    {
                        Ok(()) => target.projected.copy_from_slice(&buffer),
                        // the frame isn't the size its plans were made for: keep what it had
                        Err(ProjectError::SurfaceSize { .. }) => {}
                        Err(_) => continue,
                    }
                    emit(target, None, now_us, &mut out, &mut outputs);
                }
                out.zones.insert(
                    zone.id.clone(),
                    ZoneLive {
                        active: true,
                        ambient: zone.ambient.amount(),
                        quiet_s: zone.ambient.quiet_s(),
                    },
                );
            } else {
                out.zones.insert(zone.id.clone(), ZoneLive::INACTIVE);
            }
            for target in &mut zone.targets {
                let Some(running) = self.patterns.get(&target.id).filter(|r| r.zone == zone.id)
                else {
                    continue;
                };
                if target.layout.shape() == Shape::Display {
                    continue;
                }
                let mut colours = vec![[0.0; 3]; target.layout.len()];
                running.pattern.render(
                    now_us.saturating_sub(running.started_us),
                    &target.layout,
                    &zone.bounds,
                    &mut colours,
                );
                emit(target, Some(&colours), now_us, &mut out, &mut outputs);
            }
        }
        out.frames = outputs
            .into_iter()
            .map(|(instance, targets)| (instance, ElementFrame::new(now_us, targets)))
            .collect();
        out
    }
}

impl OutputCore {
    /// Which zone drives each target this step: the zone of its pattern, else the active zone
    /// that has it. A target another zone drove before passes its limiter's history on, so a
    /// take-over can't open a fresh window on the same lights.
    fn hand_over(&mut self) {
        let mut now: HashMap<String, usize> = HashMap::new();
        for (index, zone) in self.zones.iter().enumerate() {
            for target in &zone.targets {
                let pattern = self.patterns.get(&target.id).map(|r| r.zone.as_str());
                let drives = match pattern {
                    Some(owner) => owner == zone.id,
                    None => zone.active,
                };
                if drives {
                    now.entry(target.id.clone()).or_insert(index);
                }
            }
        }
        for (target, &to) in &now {
            let from = self
                .drivers
                .get(target)
                .and_then(|id| self.zones.iter().position(|z| z.id == *id));
            if let Some(from) = from.filter(|from| *from != to) {
                let (a, b) = if from < to {
                    let (left, right) = self.zones.split_at_mut(to);
                    (&mut left[from], &mut right[0])
                } else {
                    let (left, right) = self.zones.split_at_mut(from);
                    (&mut right[0], &mut left[to])
                };
                // `a` drove it, `b` drives it now
                let old = a.targets.iter_mut().find(|t| t.id == *target);
                let new = b.targets.iter_mut().find(|t| t.id == *target);
                if let (Some(old), Some(new)) = (old, new) {
                    new.chain.inherit_limiter(&mut old.chain);
                }
            }
        }
        for (target, to) in now {
            self.drivers.insert(target, self.zones[to].id.clone());
        }
    }
}

/// The zone's surfaces composed at `now_us`, with its ambient light (already stepped) blended in.
fn compose_surfaces(zone: &mut ZoneOutput, now_us: u64) -> Vec<(String, u16, u16, Vec<f32>)> {
    let mut composed = Vec::with_capacity(zone.surfaces.len());
    for surface in &zone.surfaces {
        if let Some(mut frame) = zone.compositor.compose(&surface.id, now_us) {
            zone.ambient.apply(&mut frame.pixels, surface.master);
            composed.push((surface.id.clone(), frame.width, frame.height, frame.pixels));
        }
    }
    composed
}

/// Hands a target's colours (its projected ones, or `pattern`'s) to the live topic and through
/// its chain to its instance's frame.
fn emit(
    target: &mut TargetSlot,
    pattern: Option<&[[f32; 3]]>,
    now_us: u64,
    out: &mut StepOutput,
    outputs: &mut BTreeMap<String, Vec<TargetOutput>>,
) {
    let shown = pattern.unwrap_or(&target.projected);
    out.targets.insert(target.id.clone(), rgb8(shown));
    let colors = target.chain.run(now_us, shown);
    if !target.instance.is_empty() {
        outputs
            .entry(target.instance.clone())
            .or_default()
            .push(TargetOutput {
                target: target.id.clone(),
                colors,
            });
    }
}

/// Colours on the 0–255 scale as bytes, truncated as providers do.
fn rgb8(colours: &[[f32; 3]]) -> Vec<u8> {
    colours
        .iter()
        .flatten()
        // clamped first, so the cast only drops the fraction
        .map(|&v| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let byte = if v.is_nan() {
                0
            } else {
                v.clamp(0.0, 255.0) as u8
            };
            byte
        })
        .collect()
}
