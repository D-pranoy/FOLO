//! The projector: a zone's mappings as gather plans, cached per configuration revision, and the
//! per-frame gather and blend.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use folo_protocol::generated::{MappingBlend, Zone};

use crate::build;
use crate::layout::{Shape, element_layout};
use crate::plan::GatherPlan;
use crate::validate::{Issue, mapping_issues, validate};

/// A surface's pixels for one frame: `width × height` RGB triples, row-major, f32 on the 0–255 scale
/// the pipeline uses (so an rgb8 frame is its bytes as floats).
#[derive(Clone, Copy, Debug)]
pub struct SurfaceView<'a> {
    width: usize,
    height: usize,
    pixels: &'a [[f32; 3]],
}

impl<'a> SurfaceView<'a> {
    /// # Errors
    /// [`ProjectError::PixelCount`] when `pixels` isn't `width × height` long.
    pub fn new(width: usize, height: usize, pixels: &'a [[f32; 3]]) -> Result<Self, ProjectError> {
        if pixels.len() == width * height {
            Ok(Self {
                width,
                height,
                pixels,
            })
        } else {
            Err(ProjectError::PixelCount {
                width,
                height,
                got: pixels.len(),
            })
        }
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &'a [[f32; 3]] {
        self.pixels
    }
}

/// Why a frame couldn't be projected. The output is untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    UnknownZone(String),
    UnknownTarget(String),
    /// The output slice isn't one colour per element.
    OutputLength {
        expected: usize,
        got: usize,
    },
    /// A surface's frame isn't the size its plans were made for (the configuration moved on).
    SurfaceSize {
        surface: String,
        expected: (usize, usize),
        got: (usize, usize),
    },
    /// A [`SurfaceView`] whose pixels aren't `width × height`.
    PixelCount {
        width: usize,
        height: usize,
        got: usize,
    },
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownZone(z) => write!(f, "zone `{z}` isn't prepared"),
            Self::UnknownTarget(t) => write!(f, "no target `{t}` in the zone"),
            Self::OutputLength { expected, got } => {
                write!(f, "the output has {got} colours for {expected} elements")
            }
            Self::SurfaceSize {
                surface,
                expected,
                got,
            } => write!(
                f,
                "surface `{surface}` is {}×{}, not {}×{}",
                got.0, got.1, expected.0, expected.1
            ),
            Self::PixelCount { width, height, got } => {
                write!(
                    f,
                    "a {width}×{height} surface has {} pixels, not {got}",
                    width * height
                )
            }
        }
    }
}

impl Error for ProjectError {}

/// What [`Projector::prepare`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// Whether plans were built, or the cached ones for this revision were kept.
    pub rebuilt: bool,
    /// What [`validate`] found when the plans were built. Mappings with errors are skipped.
    pub issues: Vec<Issue>,
}

/// Samples surfaces into the colours of a target's elements. An implementation may run on the CPU or
/// the GPU; callers don't change.
pub trait Projector {
    /// Makes the plans of `zone` for this configuration `revision`, unless it has them already.
    ///
    /// The revision is the contract: the same zone id with the same revision is the same
    /// configuration, and its plans are kept.
    fn prepare(&mut self, revision: u64, zone: &Zone) -> Prepared;

    /// Drops what [`prepare`](Self::prepare) kept for a zone that is gone.
    fn forget(&mut self, zone: &str);

    /// Projects `surfaces` (by id) onto one target's elements: `out[i]` becomes element `i`'s colour.
    ///
    /// Mappings apply in ascending priority, ties in document order, blending with their `blend`
    /// into what the lower ones left. Elements no mapping covers are black. A mapping whose surface
    /// isn't in `surfaces` is left out. Colours are on the 0–255 scale.
    ///
    /// # Errors
    /// [`ProjectError`]; `out` is then untouched.
    fn project(
        &self,
        zone: &str,
        target: &str,
        surfaces: &[(&str, SurfaceView<'_>)],
        out: &mut [[f32; 3]],
    ) -> Result<(), ProjectError>;
}

/// One mapping's plan, in a target's blend order.
#[derive(Clone, Debug)]
pub struct MappingPlan {
    pub mapping: String,
    pub surface: String,
    pub blend: MappingBlend,
    pub plan: GatherPlan,
}

#[derive(Clone, Debug)]
struct TargetPlan {
    id: String,
    elements: usize,
    /// Ascending priority, ties in document order.
    mappings: Vec<MappingPlan>,
}

#[derive(Clone, Debug)]
struct ZonePlan {
    revision: u64,
    issues: Vec<Issue>,
    targets: Vec<TargetPlan>,
}

/// The projector that gathers on the CPU.
#[derive(Debug, Default)]
pub struct CpuProjector {
    zones: HashMap<String, ZonePlan>,
}

impl CpuProjector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The revision a zone's plans were built for.
    #[must_use]
    pub fn revision(&self, zone: &str) -> Option<u64> {
        self.zones.get(zone).map(|z| z.revision)
    }

    /// A target's mapping plans in blend order, for inspection.
    #[must_use]
    pub fn plans(&self, zone: &str, target: &str) -> Option<&[MappingPlan]> {
        let t = self
            .zones
            .get(zone)?
            .targets
            .iter()
            .find(|t| t.id == target)?;
        Some(&t.mappings)
    }
}

fn build_zone(revision: u64, zone: &Zone) -> ZonePlan {
    let issues = validate(zone);
    let mut targets = Vec::with_capacity(zone.targets.len());
    for target in &zone.targets {
        let layout = element_layout(target);
        // Display targets are validated only.
        let executable = layout.shape() != Shape::Display;
        let mut mappings: Vec<(i64, MappingPlan)> = Vec::new();
        for mapping in zone
            .mappings
            .iter()
            .filter(|m| executable && *m.target == *target.id)
        {
            let Some(surface) = zone.surfaces.iter().find(|s| *s.id == *mapping.surface) else {
                continue;
            };
            if mapping_issues(mapping, surface, &layout)
                .iter()
                .any(Issue::is_error)
            {
                continue;
            }
            if let Some(plan) = build::build(mapping, surface, &layout) {
                mappings.push((
                    mapping.priority,
                    MappingPlan {
                        mapping: mapping.id.as_str().to_owned(),
                        surface: surface.id.as_str().to_owned(),
                        blend: mapping.blend,
                        plan,
                    },
                ));
            }
        }
        // A stable sort keeps document order for equal priorities.
        mappings.sort_by_key(|(priority, _)| *priority);
        targets.push(TargetPlan {
            id: target.id.as_str().to_owned(),
            elements: layout.len(),
            mappings: mappings.into_iter().map(|(_, m)| m).collect(),
        });
    }
    ZonePlan {
        revision,
        issues,
        targets,
    }
}

impl Projector for CpuProjector {
    fn prepare(&mut self, revision: u64, zone: &Zone) -> Prepared {
        let id = zone.id.as_str();
        let rebuilt = self.zones.get(id).is_none_or(|z| z.revision != revision);
        if rebuilt {
            self.zones.insert(id.to_owned(), build_zone(revision, zone));
        }
        Prepared {
            rebuilt,
            issues: self.zones[id].issues.clone(),
        }
    }

    fn forget(&mut self, zone: &str) {
        self.zones.remove(zone);
    }

    fn project(
        &self,
        zone: &str,
        target: &str,
        surfaces: &[(&str, SurfaceView<'_>)],
        out: &mut [[f32; 3]],
    ) -> Result<(), ProjectError> {
        let plan = self
            .zones
            .get(zone)
            .ok_or_else(|| ProjectError::UnknownZone(zone.to_owned()))?;
        let plan = plan
            .targets
            .iter()
            .find(|t| t.id == target)
            .ok_or_else(|| ProjectError::UnknownTarget(target.to_owned()))?;
        if out.len() != plan.elements {
            return Err(ProjectError::OutputLength {
                expected: plan.elements,
                got: out.len(),
            });
        }
        // Check every frame before writing anything, so an error leaves `out` as it was.
        let view_of = |m: &MappingPlan| {
            surfaces
                .iter()
                .find(|(id, _)| *id == m.surface)
                .map(|s| s.1)
        };
        for m in &plan.mappings {
            let Some(view) = view_of(m) else { continue };
            let expected = m.plan.surface_size();
            if (view.width, view.height) != expected {
                return Err(ProjectError::SurfaceSize {
                    surface: m.surface.clone(),
                    expected,
                    got: (view.width, view.height),
                });
            }
        }
        out.fill([0.0; 3]);
        for m in &plan.mappings {
            if let Some(view) = view_of(m) {
                blend(&m.plan, m.blend, view.pixels, out);
            }
        }
        Ok(())
    }
}

/// Gathers a mapping's colours and blends them into `out`.
///
/// - **max:** each channel is the larger of what is there and the gathered colour.
/// - **alpha:** the gathered colour goes over what is there, by the share of its kernel that was on
///   the surface: 1 (a plain replacement) except at an edge with the `none` policy.
///
/// Uncovered elements are left as they are.
fn blend(plan: &GatherPlan, mode: MappingBlend, pixels: &[[f32; 3]], out: &mut [[f32; 3]]) {
    for (e, slot) in out.iter_mut().enumerate() {
        let Some(c) = plan.gather(e, pixels) else {
            continue;
        };
        match mode {
            MappingBlend::Max => {
                for ch in 0..3 {
                    slot[ch] = slot[ch].max(c[ch]);
                }
            }
            MappingBlend::Alpha => {
                let a = plan.alpha(e);
                if a >= 1.0 {
                    *slot = c;
                } else {
                    for ch in 0..3 {
                        slot[ch] = slot[ch] * (1.0 - a) + c[ch] * a;
                    }
                }
            }
        }
    }
}
