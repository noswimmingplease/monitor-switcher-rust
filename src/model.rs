use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type AppResult<T> = Result<T, String>;

pub fn key(path: &str) -> String {
    path.trim().trim_matches('"').to_ascii_uppercase()
}

pub fn strong_path(path: &str) -> bool {
    let p = key(path);
    p.starts_with(r"\\?\DISPLAY#") && p.split('#').count() >= 4
}

pub fn profile_name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 100 && !name.chars().any(char::is_control)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
}

impl Geometry {
    pub fn effective_size(&self) -> (u32, u32) {
        if matches!(self.rotation, 2 | 4) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Monitor {
    pub path: String,
    pub name: String,
    pub device: String,
    pub active: bool,
    pub geometry: Option<Geometry>,
}

impl Monitor {
    pub fn number(&self) -> &str {
        self.device.strip_prefix(r"\\.\DISPLAY").unwrap_or("—")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub targets: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub dark: bool,
    pub always_on_top: bool,
    pub minimise_to_tray: bool,
    pub start_with_windows: bool,
    pub confirm_disabling: bool,
    pub apply_on_start: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            dark: true,
            always_on_top: false,
            minimise_to_tray: true,
            start_with_windows: false,
            confirm_disabling: true,
            apply_on_start: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    pub selected: String,
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub fallback: Option<String>,
    #[serde(default)]
    pub preferences: Preferences,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            selected: "Default".into(),
            profiles: vec![],
            aliases: BTreeMap::new(),
            order: vec![],
            fallback: None,
            preferences: Preferences::default(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> AppResult<()> {
        if self.version != 1 {
            return Err("Unsupported settings version; no settings were overwritten.".into());
        }
        let mut names = BTreeSet::new();
        for p in &self.profiles {
            if !profile_name_valid(p.name.trim()) || !names.insert(p.name.to_lowercase()) {
                return Err("Profile names must be non-empty and unique.".into());
            }
            if p.targets.is_empty() || p.targets.iter().any(|p| !strong_path(p)) {
                return Err(format!(
                    "Profile '{}' has an empty or invalid monitor set.",
                    p.name
                ));
            }
        }
        Ok(())
    }

    pub fn title(&self, monitor: &Monitor) -> String {
        self.aliases
            .get(&key(&monitor.path))
            .filter(|s| !s.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| {
                if monitor.device.is_empty() {
                    monitor.name.clone()
                } else {
                    format!("Display {}", monitor.number())
                }
            })
    }

    pub fn profile(&self) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == self.selected)
    }

    pub fn with_profile(&self, name: &str, targets: BTreeSet<String>) -> AppResult<Self> {
        let name = name.trim().to_owned();
        let mut next = self.clone();
        let profile = Profile {
            name: name.clone(),
            targets,
        };
        if let Some(old) = next
            .profiles
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(&name))
        {
            *old = profile;
        } else {
            next.profiles.push(profile);
        }
        next.selected = name;
        next.validate()?;
        Ok(next)
    }
}

pub fn active_set(monitors: &[Monitor]) -> BTreeSet<String> {
    monitors
        .iter()
        .filter(|m| m.active)
        .map(|m| key(&m.path))
        .collect()
}

pub fn desired_for_toggle(monitors: &[Monitor], target: &str) -> AppResult<BTreeSet<String>> {
    if !monitors.iter().any(|m| key(&m.path) == key(target)) {
        return Err("The monitor is no longer connected. Refresh and try again.".into());
    }
    let mut set = active_set(monitors);
    if !set.remove(&key(target)) {
        set.insert(key(target));
    }
    if set.is_empty() {
        return Err("At least one monitor must remain enabled.".into());
    }
    Ok(set)
}

/// Reject layouts that would make Windows reposition displays implicitly.
/// Coordinates and dimensions are promoted before computing rectangle edges.
pub fn validate_arrangement(monitors: &BTreeMap<String, Geometry>) -> AppResult<()> {
    if monitors.is_empty() {
        return Err("At least one monitor must remain enabled.".into());
    }
    let mut rectangles = Vec::with_capacity(monitors.len());
    for g in monitors.values() {
        if g.width == 0 || g.height == 0 || !(1..=4).contains(&g.rotation) {
            return Err(
                "The requested arrangement has invalid display dimensions or rotation.".into(),
            );
        }
        let (width, height) = g.effective_size();
        rectangles.push((
            i64::from(g.x),
            i64::from(g.y),
            i64::from(g.x) + i64::from(width),
            i64::from(g.y) + i64::from(height),
        ));
    }
    for (i, a) in rectangles.iter().enumerate() {
        for b in &rectangles[i + 1..] {
            if a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3 {
                return Err("Preserving the monitor arrangement would overlap displays. Arrange them in Windows Display Settings, then try again.".into());
            }
        }
    }
    let mut connected = vec![false; rectangles.len()];
    connected[0] = true;
    let mut pending = vec![0];
    while let Some(i) = pending.pop() {
        let a = rectangles[i];
        for (j, b) in rectangles.iter().enumerate() {
            // Accept edge or corner contact; native validation remains authoritative.
            if !connected[j] && a.0 <= b.2 && b.0 <= a.2 && a.1 <= b.3 && b.1 <= a.3 {
                connected[j] = true;
                pending.push(j);
            }
        }
    }
    if connected.contains(&false) {
        return Err("Preserving the monitor arrangement would leave disconnected displays. Windows requires a connected desktop; arrange these displays in Windows Display Settings, then try again.".into());
    }
    Ok(())
}

/// Preserve relative positions and rotation while rebasing the primary to (0, 0).
/// New targets retain the geometry Windows selected for them.
pub fn corrected_geometry(
    before: &BTreeMap<String, Geometry>,
    after: &BTreeMap<String, Geometry>,
    preferred_primary: Option<&str>,
) -> AppResult<BTreeMap<String, Geometry>> {
    let retained: Vec<_> = before.keys().filter(|p| after.contains_key(*p)).collect();
    let old_primary = before
        .iter()
        .find(|(p, g)| g.x == 0 && g.y == 0 && after.contains_key(*p))
        .map(|(p, _)| p);
    let primary = old_primary
        .or_else(|| preferred_primary.and_then(|p| after.get_key_value(p).map(|(k, _)| k)))
        .or_else(|| {
            after
                .iter()
                .find(|(_, g)| g.x == 0 && g.y == 0)
                .map(|(p, _)| p)
        })
        .ok_or("Windows did not return a unique primary monitor.")?;
    let mut result = after.clone();
    let anchor = before
        .contains_key(primary)
        .then_some(primary)
        .or_else(|| retained.first().copied());
    if let Some(anchor) = anchor {
        let dx = i64::from(after[anchor].x) - i64::from(before[anchor].x);
        let dy = i64::from(after[anchor].y) - i64::from(before[anchor].y);
        for p in retained {
            let mut g = before[p].clone();
            g.x = i32::try_from(i64::from(g.x) + dx).map_err(|_| "Display position overflow.")?;
            g.y = i32::try_from(i64::from(g.y) + dy).map_err(|_| "Display position overflow.")?;
            result.insert(p.clone(), g);
        }
    }
    let origin = result[primary].clone();
    for g in result.values_mut() {
        g.x =
            g.x.checked_sub(origin.x)
                .ok_or("Display position overflow.")?;
        g.y =
            g.y.checked_sub(origin.y)
                .ok_or("Display position overflow.")?;
    }
    if result.values().filter(|g| g.x == 0 && g.y == 0).count() != 1 {
        return Err("The requested arrangement has ambiguous primary origins.".into());
    }
    validate_arrangement(&result)?;
    Ok(result)
}

pub fn verify_geometry(
    expected: &BTreeMap<String, Geometry>,
    actual: &BTreeMap<String, Geometry>,
) -> AppResult<()> {
    if expected.keys().ne(actual.keys()) {
        return Err("The active physical monitor set does not match the request.".into());
    }
    for (p, g) in expected {
        if actual[p] != *g {
            return Err(format!(
                "Arrangement verification failed for {p}: observed {:?}, expected {g:?}.",
                actual[p]
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_profile_names_and_empty_sets_cannot_poison_existing_settings() {
        let original = Settings::default();
        let targets = BTreeSet::from([r"\\?\DISPLAY#MODEL#A#{GUID}".into()]);
        for name in [String::new(), "x".repeat(101), "Bad\tname".into()] {
            assert!(original.with_profile(&name, targets.clone()).is_err());
            assert!(original.profiles.is_empty());
        }
        assert!(original.with_profile("Valid", BTreeSet::new()).is_err());
        let first = original.with_profile(" Work ", targets.clone()).unwrap();
        let replaced = first.with_profile("work", targets).unwrap();
        assert_eq!(replaced.profiles.len(), 1);
        assert_eq!(replaced.selected, "work");
        assert_eq!(first.selected, "Work");
    }
    fn g(x: i32, y: i32) -> Geometry {
        Geometry {
            x,
            y,
            width: 2560,
            height: 1440,
            rotation: 4,
        }
    }
    #[test]
    fn corrects_small_and_large_drift_without_a_tolerance_limit() {
        let before = BTreeMap::from([("A".into(), g(0, 0)), ("B".into(), g(1440, -1133))]);
        for offset in [-1130, 22000] {
            let actual = BTreeMap::from([("A".into(), g(0, 0)), ("B".into(), g(1440, offset))]);
            assert_eq!(corrected_geometry(&before, &actual, None).unwrap(), before);
        }
    }
    #[test]
    fn primary_removal_preserves_portrait_flipped_and_relative_geometry() {
        let before = BTreeMap::from([
            ("A".into(), g(0, 0)),
            ("B".into(), g(1440, -1133)),
            ("C".into(), g(2880, -1133)),
        ]);
        let after = BTreeMap::from([("B".into(), g(0, 0)), ("C".into(), g(1440, 3))]);
        let result = corrected_geometry(&before, &after, None).unwrap();
        assert_eq!(result["B"], g(0, 0));
        assert_eq!(result["C"], g(1440, 0));
        assert_eq!(corrected_geometry(&result, &result, None).unwrap(), result);
    }
    #[test]
    fn new_targets_keep_windows_geometry() {
        let before = BTreeMap::from([("A".into(), g(0, 0))]);
        let after = BTreeMap::from([("A".into(), g(0, 0)), ("B".into(), g(-1440, 0))]);
        assert_eq!(corrected_geometry(&before, &after, None).unwrap(), after);
    }
    #[test]
    fn strict_verification_checks_rotation_and_targets() {
        let expected = BTreeMap::from([("A".into(), g(0, 0))]);
        let mut actual = expected.clone();
        actual.get_mut("A").unwrap().rotation = 1;
        assert!(verify_geometry(&expected, &actual).is_err());
        assert!(verify_geometry(&expected, &BTreeMap::new()).is_err());
    }

    fn landscape(x: i32) -> Geometry {
        Geometry {
            x,
            y: 0,
            width: 1920,
            height: 1080,
            rotation: 1,
        }
    }

    #[test]
    fn disjoint_profiles_honour_the_available_fallback_primary() {
        let before = BTreeMap::from([("A".into(), landscape(0))]);
        let after = BTreeMap::from([("B".into(), landscape(0)), ("C".into(), landscape(1920))]);
        let corrected = corrected_geometry(&before, &after, Some("C")).unwrap();
        assert_eq!(corrected["C"], landscape(0));
        assert_eq!(corrected["B"], landscape(-1920));
        assert_eq!(
            corrected_geometry(&before, &after, Some("Absent")).unwrap(),
            after
        );
        assert_eq!(corrected_geometry(&before, &after, None).unwrap(), after);
    }

    #[test]
    fn correction_rejects_new_targets_overlapping_retained_positions() {
        let before = BTreeMap::from([("A".into(), landscape(0)), ("B".into(), landscape(1920))]);
        let after = BTreeMap::from([
            ("A".into(), landscape(0)),
            ("B".into(), landscape(3840)),
            ("C".into(), landscape(1920)),
        ]);
        assert!(validate_arrangement(&before).is_ok());
        assert!(validate_arrangement(&after).is_ok());
        assert!(
            corrected_geometry(&before, &after, None)
                .unwrap_err()
                .contains("overlap")
        );
    }

    #[test]
    fn removing_a_bridge_display_cannot_silently_rearrange_survivors() {
        let before = BTreeMap::from([
            ("A".into(), landscape(0)),
            ("B".into(), landscape(1920)),
            ("C".into(), landscape(3840)),
        ]);
        let after = BTreeMap::from([("A".into(), landscape(0)), ("C".into(), landscape(1920))]);
        assert!(
            corrected_geometry(&before, &after, None)
                .unwrap_err()
                .contains("disconnected")
        );
    }

    #[test]
    fn arrangement_validation_handles_rotation_corner_contact_and_large_edges() {
        let mut monitors = BTreeMap::from([("A".into(), g(0, 0)), ("B".into(), g(1440, 2560))]);
        assert!(validate_arrangement(&monitors).is_ok());
        monitors.get_mut("B").unwrap().x += 1;
        assert!(validate_arrangement(&monitors).is_err());
        let far = Geometry {
            x: i32::MAX,
            width: u32::MAX,
            ..landscape(0)
        };
        assert!(validate_arrangement(&BTreeMap::from([("A".into(), far)])).is_ok());
    }
}
