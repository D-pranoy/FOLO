//! Binding a scene's roles to a zone's surfaces (ADR-0038 D12).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// What binding needs to know of a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    pub id: String,
    pub canvas: bool,
    pub role: String,
    /// Cells across, which scaling takes (D14).
    pub width: u64,
    /// All the cells, which capability and the nearest size compare.
    pub cells: u64,
}

impl Surface {
    /// The surfaces of a zone, in `zones.json` order.
    #[must_use]
    pub fn of_zone(zone: &Value) -> Vec<Self> {
        let cell_count = |value: &Value| value.as_u64().unwrap_or(1);
        zone["surfaces"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|surface| {
                let width = cell_count(&surface["resolution"]["width"]);
                let height = cell_count(&surface["resolution"]["height"]);
                Self {
                    id: surface["id"].as_str().unwrap_or_default().to_owned(),
                    canvas: surface["kind"] == "canvas",
                    role: surface["role"].as_str().unwrap_or_default().to_owned(),
                    width,
                    cells: width * height,
                }
            })
            .collect()
    }
}

/// What a scene's roles ask of the zone.
pub struct Wants<'a> {
    /// Each role, with the most cells its layers' effects need (`min_elements`).
    pub roles: &'a BTreeMap<String, u64>,
    /// The item's `fallbacks`: role, another role to try.
    pub fallbacks: &'a BTreeMap<String, String>,
    /// The strip length the scene's cells were made for; 0 for none.
    pub led_count: u64,
    /// Role: surface id, from the request and what the show remembers.
    pub overrides: &'a BTreeMap<String, String>,
}

/// Where each role went.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Binding {
    /// Role: index of its surface in the zone's list.
    pub bound: BTreeMap<String, usize>,
    /// Roles no surface could take, in byte order.
    pub skipped: Vec<String>,
}

/// Binds the roles of `wants`, in byte order of their names: overrides for
/// every role first, then automatically for the rest.
#[must_use]
pub fn bind(wants: &Wants, surfaces: &[Surface]) -> Binding {
    let mut binding = Binding::default();
    // Roles of the surfaces already used: the engine finds layers by surface role
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    let mut overridden: BTreeSet<&str> = BTreeSet::new();
    for role in wants.roles.keys() {
        let Some(wanted) = wants.overrides.get(role) else {
            continue;
        };
        // An override naming no surface of the zone is ignored: the role is automatic
        let Some(index) = surfaces.iter().position(|s| s.id == *wanted) else {
            continue;
        };
        overridden.insert(role);
        if taken.insert(&surfaces[index].role) {
            binding.bound.insert(role.clone(), index);
        } else {
            binding.skipped.push(role.clone());
        }
    }
    for (role, need) in wants.roles {
        if overridden.contains(role.as_str()) {
            continue;
        }
        match automatic(wants, surfaces, &taken, role, *need) {
            Some(index) => {
                taken.insert(&surfaces[index].role);
                binding.bound.insert(role.clone(), index);
            }
            None => binding.skipped.push(role.clone()),
        }
    }
    binding.skipped.sort();
    binding
}

/// Steps 2.1 to 2.3: the same role, the same kind, then the fallbacks.
fn automatic(
    wants: &Wants,
    surfaces: &[Surface],
    taken: &BTreeSet<&str>,
    role: &str,
    need: u64,
) -> Option<usize> {
    let usable =
        |surface: &Surface| !taken.contains(surface.role.as_str()) && surface.cells >= need;
    let by_role = |name: &str| surfaces.iter().position(|s| usable(s) && s.role == name);
    let by_kind = |name: &str| same_kind(surfaces, &usable, name, wants.led_count);
    by_role(role)
        .or_else(|| by_kind(role))
        .or_else(|| {
            let other = wants.fallbacks.get(role)?;
            by_role(other).or_else(|| by_kind(other))
        })
        .or_else(|| {
            if !role.starts_with("strip.") {
                return None;
            }
            by_role("canvas.room").or_else(|| surfaces.iter().position(|s| usable(s) && s.canvas))
        })
}

/// Step 2.2: of the surfaces of the role's kind, the one nearest `led_count`
/// cells (when it isn't 0), else the one with the most; ties go to the first.
fn same_kind(
    surfaces: &[Surface],
    usable: &impl Fn(&Surface) -> bool,
    role: &str,
    led_count: u64,
) -> Option<usize> {
    let canvas = if role.starts_with("strip.") {
        false
    } else if role.starts_with("canvas.") {
        true
    } else {
        return None;
    };
    surfaces
        .iter()
        .enumerate()
        .filter(|(_, s)| s.canvas == canvas && usable(s))
        .min_by_key(|(_, s)| {
            if led_count == 0 {
                (0, Reverse(s.cells))
            } else {
                (s.cells.abs_diff(led_count), Reverse(0))
            }
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: &str, role: &str, canvas: bool, cells: u64) -> Surface {
        Surface {
            id: id.into(),
            canvas,
            role: role.into(),
            width: cells,
            cells,
        }
    }

    fn roles(names: &[&str]) -> BTreeMap<String, u64> {
        names.iter().map(|n| ((*n).to_owned(), 1)).collect()
    }

    fn run(
        names: &[&str],
        overrides: &[(&str, &str)],
        led_count: u64,
        surfaces: &[Surface],
    ) -> Binding {
        let overrides = overrides
            .iter()
            .map(|(r, s)| ((*r).to_owned(), (*s).to_owned()))
            .collect();
        bind(
            &Wants {
                roles: &roles(names),
                fallbacks: &BTreeMap::new(),
                led_count,
                overrides: &overrides,
            },
            surfaces,
        )
    }

    #[test]
    fn the_same_role_wins_over_the_same_kind() {
        let surfaces = [
            surface("a", "strip.other", false, 500),
            surface("b", "strip.main", false, 10),
        ];
        let binding = run(&["strip.main"], &[], 0, &surfaces);
        assert_eq!(binding.bound["strip.main"], 1);
    }

    #[test]
    fn the_same_kind_takes_the_nearest_size_or_the_most_cells() {
        let surfaces = [
            surface("a", "strip.a", false, 30),
            surface("b", "strip.b", false, 144),
            surface("c", "strip.c", false, 150),
        ];
        assert_eq!(
            run(&["strip.main"], &[], 140, &surfaces).bound["strip.main"],
            1
        );
        assert_eq!(
            run(&["strip.main"], &[], 0, &surfaces).bound["strip.main"],
            2
        );
        assert_eq!(
            run(&["strip.main"], &[], 31, &surfaces).bound["strip.main"],
            0
        );
    }

    #[test]
    fn ties_go_to_the_first_surface() {
        let surfaces = [
            surface("a", "strip.a", false, 60),
            surface("b", "strip.b", false, 60),
        ];
        assert_eq!(
            run(&["strip.main"], &[], 0, &surfaces).bound["strip.main"],
            0
        );
        assert_eq!(
            run(&["strip.main"], &[], 60, &surfaces).bound["strip.main"],
            0
        );
    }

    #[test]
    fn no_two_roles_share_a_surface_role() {
        let surfaces = [
            surface("a", "strip.main", false, 30),
            surface("b", "strip.main", false, 30),
        ];
        let binding = run(&["strip.left", "strip.right"], &[], 0, &surfaces);
        assert_eq!(binding.bound.len(), 1);
        assert_eq!(binding.skipped, ["strip.right"]);
    }

    #[test]
    fn an_override_wins_and_a_taken_one_skips_its_role() {
        let surfaces = [
            surface("a", "strip.a", false, 30),
            surface("b", "strip.b", false, 30),
        ];
        let binding = run(&["strip.main"], &[("strip.main", "b")], 0, &surfaces);
        assert_eq!(binding.bound["strip.main"], 1);
        let both = run(
            &["strip.l", "strip.r"],
            &[("strip.l", "a"), ("strip.r", "a")],
            0,
            &surfaces,
        );
        assert_eq!(both.bound["strip.l"], 0);
        assert_eq!(both.skipped, ["strip.r"]);
    }

    #[test]
    fn an_override_naming_no_surface_is_ignored() {
        let surfaces = [surface("a", "strip.a", false, 30)];
        let binding = run(&["strip.main"], &[("strip.main", "zzz")], 0, &surfaces);
        assert_eq!(binding.bound["strip.main"], 0);
    }

    #[test]
    fn overrides_are_taken_before_automatic_binding() {
        // `strip.a` would take surface `x` by role, but `strip.b` was told to
        let surfaces = [
            surface("x", "strip.a", false, 30),
            surface("y", "strip.z", false, 30),
        ];
        let binding = run(&["strip.a", "strip.b"], &[("strip.b", "x")], 0, &surfaces);
        assert_eq!(binding.bound["strip.b"], 0);
        assert_eq!(binding.bound["strip.a"], 1);
    }

    #[test]
    fn capability_is_checked_for_automatic_binding_only() {
        let surfaces = [surface("a", "strip.main", false, 3)];
        let needs = BTreeMap::from([("strip.main".to_owned(), 4)]);
        let wants = |overrides: &BTreeMap<String, String>| {
            bind(
                &Wants {
                    roles: &needs,
                    fallbacks: &BTreeMap::new(),
                    led_count: 0,
                    overrides,
                },
                &surfaces,
            )
        };
        assert_eq!(wants(&BTreeMap::new()).skipped, ["strip.main"]);
        let forced = BTreeMap::from([("strip.main".to_owned(), "a".to_owned())]);
        assert_eq!(wants(&forced).bound["strip.main"], 0);
    }

    #[test]
    fn fallbacks_then_the_room_canvas_for_strip_roles() {
        let surfaces = [
            surface("c", "canvas.wall", true, 100),
            surface("r", "canvas.room", true, 50),
        ];
        let fallbacks = BTreeMap::from([("strip.a".to_owned(), "canvas.wall".to_owned())]);
        let overrides = BTreeMap::new();
        let with = |roles: &BTreeMap<String, u64>, fallbacks: &BTreeMap<String, String>| {
            bind(
                &Wants {
                    roles,
                    fallbacks,
                    led_count: 0,
                    overrides: &overrides,
                },
                &surfaces,
            )
        };
        assert_eq!(with(&roles(&["strip.a"]), &fallbacks).bound["strip.a"], 0);
        assert_eq!(
            with(&roles(&["strip.a"]), &BTreeMap::new()).bound["strip.a"],
            1
        );
        assert!(
            with(&roles(&["points.a"]), &BTreeMap::new())
                .bound
                .is_empty()
        );
    }

    #[test]
    fn a_strip_role_falls_back_to_the_first_canvas_without_a_room_canvas() {
        let surfaces = [
            surface("c", "canvas.wall", true, 100),
            surface("d", "canvas.desk", true, 10),
        ];
        let binding = run(&["strip.main"], &[], 0, &surfaces);
        assert_eq!(binding.bound["strip.main"], 0);
    }
}
