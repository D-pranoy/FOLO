//! Hyperion's LED rectangles as element positions, and the proportional scaling that follows a
//! change of the LED count (`docs/integrations/hyperion.md`, FOLO provider design).

use folo_protocol::generated::{Address, ManualRange, Plane, Span, Vec3};
use folo_provider::DescribedTarget;

use crate::serverinfo::LedRect;

/// A TV's default size in metres (`docs/specs/config-model.md`, zone reference).
const DEFAULT_TV_WIDTH: f64 = 1.2;
const DEFAULT_TV_HEIGHT: f64 = 0.68;

/// The plane of a TV of the default size at the default reference position, facing the room, as
/// the spatial model gives it: origin at the top left as a viewer sees it, `u` along the width
/// and `v` down the height. `describe` offers layouts on this plane, because a provider doesn't
/// know the zone; the zone editor places them on the zone's own TV with [`place_on_plane`].
#[must_use]
pub fn default_tv_plane() -> Plane {
    Plane {
        origin: Vec3 {
            x: -DEFAULT_TV_WIDTH / 2.0,
            y: 0.0,
            z: DEFAULT_TV_HEIGHT / 2.0,
        },
        u: Vec3 {
            x: DEFAULT_TV_WIDTH,
            y: 0.0,
            z: 0.0,
        },
        v: Vec3 {
            x: 0.0,
            y: 0.0,
            z: -DEFAULT_TV_HEIGHT,
        },
    }
}

/// The position of each LED on a TV's plane: its rectangle's horizontal centre along `u` and its
/// vertical centre along `v`, from the plane's origin corner. On an unturned TV that is x from
/// the horizontal centre and z from the vertical one, and a turned TV turns them with it.
#[must_use]
pub fn place_on_plane(leds: &[LedRect], plane: &Plane) -> Vec<Vec3> {
    leds.iter()
        .map(|led| {
            let along = f64::midpoint(led.hmin, led.hmax);
            let down = f64::midpoint(led.vmin, led.vmax);
            Vec3 {
                x: plane.origin.x + plane.u.x * along + plane.v.x * down,
                y: plane.origin.y + plane.u.y * along + plane.v.y * down,
                z: plane.origin.z + plane.u.z * along + plane.v.z * down,
            }
        })
        .collect()
}

/// A span stretched to a new LED count so it keeps its share of the strip, with the current
/// studio's arithmetic (`scale_segments`): start and end are scaled and rounded half to even, and
/// the end is never before the start. Returns `(start, count)`.
#[must_use]
pub fn scale_span(start: u64, count: u64, old_count: u64, new_count: u64) -> (u64, u64) {
    if old_count == new_count || old_count == 0 || new_count == 0 {
        return (start, count);
    }
    #[allow(clippy::cast_precision_loss)]
    let ratio = new_count as f64 / old_count as f64;
    let scaled = |value: u64| {
        #[allow(clippy::cast_precision_loss)]
        let value = value as f64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rounded = (value * ratio).round_ties_even() as u64;
        rounded
    };
    let new_start = scaled(start).min(new_count - 1);
    let end = start + count.max(1) - 1;
    let new_end = scaled(end + 1)
        .saturating_sub(1)
        .max(new_start)
        .min(new_count - 1);
    (new_start, new_end - new_start + 1)
}

fn scale_in_place(span: &mut Span, old_count: u64, new_count: u64) {
    let (start, count) = scale_span(
        u64::from(span.start),
        span.count.get(),
        old_count,
        new_count,
    );
    span.start = u16::try_from(start).unwrap_or(u16::MAX);
    span.count = std::num::NonZeroU64::new(count).unwrap_or(std::num::NonZeroU64::MIN);
}

/// A target's address stretched to a new LED count.
pub fn scale_address(address: &mut Address, old_count: u64, new_count: u64) {
    let (start, count) = scale_span(
        u64::from(address.start),
        address.count.get(),
        old_count,
        new_count,
    );
    address.start = u16::try_from(start).unwrap_or(u16::MAX);
    address.count = std::num::NonZeroU64::new(count).unwrap_or(std::num::NonZeroU64::MIN);
}

/// The manual ranges of a mapping onto a target, stretched to the target's new LED count: the
/// `elements` side always, and the `cells` side too when the surface is the target's own strip,
/// whose resolution follows the element count.
pub fn scale_ranges(ranges: &mut [ManualRange], old_count: u64, new_count: u64, strip_cells: bool) {
    for range in ranges {
        scale_in_place(&mut range.elements, old_count, new_count);
        if strip_cells {
            scale_in_place(&mut range.cells, old_count, new_count);
        }
    }
}

/// A target whose LED count changed between two describes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutChange {
    pub target: String,
    pub old_count: u64,
    pub new_count: u64,
}

/// The targets present in both describes whose element count differs. The controller runs this
/// on each round the sender's `describe_every` brings, and rescales what it stores with
/// [`scale_address`] and [`scale_ranges`].
#[must_use]
pub fn layout_changes(before: &[DescribedTarget], after: &[DescribedTarget]) -> Vec<LayoutChange> {
    after
        .iter()
        .filter_map(|now| {
            let was = before.iter().find(|t| t.id == now.id)?;
            (was.elements != now.elements).then(|| LayoutChange {
                target: now.id.clone(),
                old_count: u64::from(was.elements),
                new_count: u64::from(now.elements),
            })
        })
        .collect()
}
