//! Zone activation (RB-08 step 5, ADR-0036 D3 to D6, docs/architecture/
//! spatial-model.md#zone-activation-rules): which zones are effectively
//! active, the conflicts of an activation and taking targets over. The
//! functions are pure: they read a zones document and return what to write.

use serde_json::Value;

/// What the check found in a zones document.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// The effectively active zones, in document order.
    pub active: Vec<String>,
    /// Zones whose stored flag is true that lost a target: the zone and the
    /// zone holding it.
    pub suppressed: Vec<(String, String)>,
}

impl Check {
    /// Whether the zone is effectively active.
    #[must_use]
    pub fn is_active(&self, id: &str) -> bool {
        self.active.iter().any(|active| active == id)
    }
}

fn zone_list(zones: &Value) -> &[Value] {
    zones["zones"].as_array().map_or(&[], Vec::as_slice)
}

fn id_of(zone: &Value) -> &str {
    zone["id"].as_str().unwrap_or_default()
}

/// The zone's stored flag; a zone without one is active (the default).
fn stored_active(zone: &Value) -> bool {
    zone["active"] != false
}

/// Runs the check: of the zones that are active in the file and share a
/// target, the first keeps it. A zone that lost is no holder for the zones
/// after it, so a chain of overlaps doesn't silence more zones than it must.
#[must_use]
pub fn check(zones: &Value) -> Check {
    let mut found = Check::default();
    let mut kept: Vec<&Value> = Vec::new();
    for zone in zone_list(zones).iter().filter(|z| stored_active(z)) {
        if let Some(holder) = kept.iter().find(|holder| shares(holder, zone)) {
            let lost = (id_of(zone).to_owned(), id_of(holder).to_owned());
            found.suppressed.push(lost);
        } else {
            found.active.push(id_of(zone).to_owned());
            kept.push(zone);
        }
    }
    found
}

/// The active zones other than `id` that share a target with it: what an
/// activation would take over. Empty when the zone itself is effectively
/// active, or when there is no such zone.
#[must_use]
pub fn conflicts(zones: &Value, id: &str) -> Vec<String> {
    let found = check(zones);
    let Some(zone) = zone_list(zones).iter().find(|z| id_of(z) == id) else {
        return Vec::new();
    };
    if found.is_active(id) {
        return Vec::new();
    }
    zone_list(zones)
        .iter()
        .filter(|other| id_of(other) != id && found.is_active(id_of(other)))
        .filter(|other| shares(other, zone))
        .map(|other| id_of(other).to_owned())
        .collect()
}

/// The document with the zone's stored flag set.
#[must_use]
pub fn set_active(zones: &Value, id: &str, active: bool) -> Value {
    let mut out = zones.clone();
    if let Some(list) = out["zones"].as_array_mut() {
        for zone in list.iter_mut().filter(|z| id_of(z) == id) {
            zone["active"] = Value::Bool(active);
        }
    }
    out
}

/// The document with the zone activated and every other zone that is
/// active in the file and shares a target with it deactivated entirely. That
/// includes zones the check had suppressed: they would win the target from
/// the zone if they come before it in the file.
#[must_use]
pub fn take_over(zones: &Value, id: &str) -> Value {
    let Some(zone) = zone_list(zones).iter().find(|z| id_of(z) == id) else {
        return zones.clone();
    };
    let losers: Vec<String> = zone_list(zones)
        .iter()
        .filter(|other| id_of(other) != id && stored_active(other) && shares(other, zone))
        .map(|other| id_of(other).to_owned())
        .collect();
    let mut out = set_active(zones, id, true);
    for loser in losers {
        out = set_active(&out, &loser, false);
    }
    out
}

/// The document with each zone's `active` set to its effective state, for
/// what reads the documents to act on them (the engine's `config.apply`).
#[must_use]
pub fn with_effective(zones: &Value) -> Value {
    let found = check(zones);
    let mut out = zones.clone();
    if let Some(list) = out["zones"].as_array_mut() {
        for zone in list {
            zone["active"] = Value::Bool(found.is_active(id_of(zone)));
        }
    }
    out
}

/// Whether two zones share a target ([ADR-0036](../../../docs/decisions/ADR-0036-controller-output.md) D4).
fn shares(a: &Value, b: &Value) -> bool {
    let targets = |zone: &'_ Value| zone["targets"].as_array().map_or(Vec::new(), Clone::clone);
    let (a, b) = (targets(a), targets(b));
    a.iter().any(|x| b.iter().any(|y| same_target(x, y)))
}

/// The same target id, or overlapping addresses on one provider instance.
fn same_target(a: &Value, b: &Value) -> bool {
    if !a["id"].as_str().unwrap_or_default().is_empty() && a["id"] == b["id"] {
        return true;
    }
    let instance = a["provider"]["instance"].as_str().unwrap_or_default();
    // A target without an instance isn't bound to anything yet
    if instance.is_empty() || a["provider"]["instance"] != b["provider"]["instance"] {
        return false;
    }
    overlap(&a["provider"]["address"], &b["provider"]["address"])
}

/// Hue channels in common, or overlapping LED ranges when neither lists channels.
fn overlap(a: &Value, b: &Value) -> bool {
    let channels = |address: &'_ Value| -> Vec<u64> {
        address["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64)
            .collect()
    };
    let (ca, cb) = (channels(a), channels(b));
    if !ca.is_empty() || !cb.is_empty() {
        return ca.iter().any(|c| cb.contains(c));
    }
    let range = |address: &'_ Value| {
        let start = address["start"].as_u64().unwrap_or(0);
        (start, start + address["count"].as_u64().unwrap_or(1))
    };
    let ((sa, ea), (sb, eb)) = (range(a), range(b));
    sa < eb && sb < ea
}

#[cfg(test)]
#[allow(clippy::needless_pass_by_value)] // the helpers take the lists the tests write inline
mod tests {
    use super::*;
    use serde_json::json;

    fn leds(id: &str, instance: &str, start: u64, count: u64) -> Value {
        json!({"id": id, "provider": {"instance": instance, "address": {"start": start, "count": count}}})
    }

    fn zone(id: &str, active: bool, targets: Vec<Value>) -> Value {
        json!({"id": id, "active": active, "targets": targets})
    }

    fn doc(zones: Vec<Value>) -> Value {
        json!({"zones": zones})
    }

    fn stored(zones: &Value, id: &str) -> bool {
        stored_active(zone_list(zones).iter().find(|z| id_of(z) == id).unwrap())
    }

    #[test]
    fn zones_without_shared_targets_all_run() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 10, 10)]),
            zone("c", true, vec![leds("t3", "q", 0, 10)]),
        ]);
        let found = check(&zones);
        assert_eq!(found.active, ["a", "b", "c"]);
        assert_eq!(found.suppressed.len(), 0);
    }

    #[test]
    fn the_first_in_the_file_keeps_a_shared_target() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 9, 10)]),
        ]);
        let found = check(&zones);
        assert_eq!(found.active, ["a"]);
        assert_eq!(found.suppressed, [("b".to_owned(), "a".to_owned())]);
        // The check never rewrites the stored flag
        assert!(stored(&zones, "b"));
    }

    #[test]
    fn the_same_id_is_shared_whatever_the_address() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t", "p", 0, 10)]),
            zone("b", true, vec![leds("t", "q", 50, 10)]),
        ]);
        assert_eq!(check(&zones).active, ["a"]);
    }

    #[test]
    fn touching_ranges_and_other_instances_are_not_shared() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 10, 10)]),
            zone("c", true, vec![leds("t3", "q", 0, 10)]),
            zone("d", true, vec![leds("t4", "", 0, 10)]),
            zone("e", true, vec![leds("t5", "", 0, 10)]),
        ]);
        assert_eq!(check(&zones).active, ["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn hue_channels_in_common_are_shared() {
        let hue = |id: &str, channels: Vec<u64>| json!({"id": id, "provider": {"instance": "h", "address": {"channels": channels}}});
        let zones = doc(vec![
            zone("a", true, vec![hue("t1", vec![0, 1])]),
            zone("b", true, vec![hue("t2", vec![2, 3])]),
            zone("c", true, vec![hue("t3", vec![1, 4])]),
        ]);
        assert_eq!(check(&zones).active, ["a", "b"]);
    }

    #[test]
    fn channels_and_led_ranges_on_one_instance_never_share() {
        let hue = json!({"id": "h", "provider": {"instance": "p", "address": {"channels": [0]}}});
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![hue]),
        ]);
        assert_eq!(check(&zones).active, ["a", "b"]);
    }

    #[test]
    fn a_zone_that_lost_does_not_hold_targets_for_later_zones() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 5, 10)]),
            zone("c", true, vec![leds("t3", "p", 12, 10)]),
        ]);
        assert_eq!(check(&zones).active, ["a", "c"]);
    }

    #[test]
    fn stored_inactive_zones_never_hold_anything() {
        let zones = doc(vec![
            zone("a", false, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 0, 10)]),
        ]);
        assert_eq!(check(&zones).active, ["b"]);
        assert_eq!(check(&zones).suppressed.len(), 0);
    }

    #[test]
    fn a_new_zone_goes_last_and_comes_up_inactive() {
        let mut zones = doc(vec![zone("a", true, vec![leds("t1", "p", 0, 10)])]);
        zones["zones"].as_array_mut().unwrap().push(zone(
            "new",
            true,
            vec![leds("t9", "p", 0, 10)],
        ));
        assert!(!check(&zones).is_active("new"));
        assert!(stored(&zones, "new"));
    }

    #[test]
    fn a_suppressed_zone_comes_up_when_its_holder_is_deactivated() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 0, 10)]),
        ]);
        let off = set_active(&zones, "a", false);
        assert_eq!(check(&off).active, ["b"]);
    }

    #[test]
    fn conflicts_are_the_active_zones_sharing_a_target() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 20, 10)]),
            zone("c", false, vec![leds("t3", "p", 5, 30)]),
        ]);
        assert_eq!(conflicts(&zones, "c"), ["a", "b"]);
        assert_eq!(conflicts(&zones, "a").len(), 0);
        assert_eq!(conflicts(&zones, "missing").len(), 0);
    }

    #[test]
    fn a_suppressed_zone_has_its_holder_as_conflict() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 0, 10)]),
        ]);
        assert_eq!(conflicts(&zones, "b"), ["a"]);
    }

    #[test]
    fn take_over_deactivates_the_conflicting_zones_entirely() {
        let zones = doc(vec![
            zone(
                "a",
                true,
                vec![leds("t1", "p", 0, 10), leds("t1b", "p", 100, 10)],
            ),
            zone("b", true, vec![leds("t2", "q", 0, 10)]),
            zone("c", false, vec![leds("t3", "p", 5, 10)]),
        ]);
        let taken = take_over(&zones, "c");
        assert!(stored(&taken, "c"));
        assert!(!stored(&taken, "a"));
        assert!(stored(&taken, "b"));
        assert_eq!(check(&taken).active, ["b", "c"]);
    }

    #[test]
    fn take_over_wins_against_a_suppressed_zone_earlier_in_the_file() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 0, 10)]),
            zone("c", false, vec![leds("t3", "p", 0, 10)]),
        ]);
        let taken = take_over(&zones, "c");
        assert_eq!(check(&taken).active, ["c"]);
    }

    #[test]
    fn effective_flags_replace_the_stored_ones_in_a_copy() {
        let zones = doc(vec![
            zone("a", true, vec![leds("t1", "p", 0, 10)]),
            zone("b", true, vec![leds("t2", "p", 0, 10)]),
        ]);
        let effective = with_effective(&zones);
        assert_eq!(effective["zones"][1]["active"], false);
        assert!(stored(&zones, "b"));
    }
}
