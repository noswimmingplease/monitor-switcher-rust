//! Windows CCD backend. Every mutation owns an immutable pre-change snapshot.
use crate::model::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
    ptr, thread,
    time::Duration,
};
use windows_sys::Win32::{Devices::Display::*, Foundation::LUID};
const DISPLAYCONFIG_PATH_ACTIVE: u32 = 1;

#[derive(Clone)]
struct Entry {
    path: DISPLAYCONFIG_PATH_INFO,
    monitor: Monitor,
    source_index: Option<usize>,
}

#[derive(Clone)]
struct Snapshot {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    entries: Vec<Entry>,
}

fn luid(id: LUID) -> (i32, u32) {
    (id.HighPart, id.LowPart)
}
fn source(path: &DISPLAYCONFIG_PATH_INFO) -> ((i32, u32), u32) {
    (luid(path.sourceInfo.adapterId), path.sourceInfo.id)
}
fn utf16(text: &[u16]) -> String {
    String::from_utf16_lossy(&text[..text.iter().position(|c| *c == 0).unwrap_or(text.len())])
}

fn query(all: bool) -> AppResult<Snapshot> {
    let flags = if all {
        QDC_ALL_PATHS
    } else {
        QDC_ONLY_ACTIVE_PATHS
    };
    for _ in 0..6 {
        let (mut count, mut modes) = (0, 0);
        // SAFETY: Windows fills counts, then writes into arrays sized from those counts.
        let code = unsafe { GetDisplayConfigBufferSizes(flags, &mut count, &mut modes) };
        if code != 0 {
            return Err(format!("Windows display enumeration failed ({code})."));
        }
        if count == 0 || count > 16384 || modes > 65536 {
            return Err("Windows returned an empty or excessive display topology.".into());
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); count as usize];
        let mut mode_data = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        let code = unsafe {
            QueryDisplayConfig(
                flags,
                &mut count,
                paths.as_mut_ptr(),
                &mut modes,
                mode_data.as_mut_ptr(),
                ptr::null_mut(),
            )
        };
        if code == 122 {
            continue;
        }
        if code != 0 {
            return Err(format!("Windows display enumeration failed ({code})."));
        }
        paths.truncate(count as usize);
        mode_data.truncate(modes as usize);
        let mut entries = Vec::new();
        for path in &paths {
            let active = path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0;
            if active && path.targetInfo.targetAvailable == 0 {
                return Err("Windows reports an active monitor as unavailable. Wait for detection to settle, then Refresh.".into());
            }
            if !active && path.targetInfo.targetAvailable == 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            let code = unsafe { DisplayConfigGetDeviceInfo(&mut target.header) };
            let identity = key(&utf16(&target.monitorDevicePath));
            if code != 0 || !strong_path(&identity) {
                if active {
                    return Err("An active monitor has no reliable physical identity. Refresh after Windows finishes detecting displays.".into());
                }
                continue;
            }
            let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            name.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            let name_code = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
            let device = if active && name_code == 0 {
                utf16(&name.viewGdiDeviceName)
            } else {
                String::new()
            };
            // Virtual-mode-aware flags are deliberately absent: CCD returns ordinary indices.
            let source_index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let geometry = if active {
                let mode = mode_data
                    .get(source_index)
                    .ok_or("An active monitor has no source mode.")?;
                if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                    || luid(mode.adapterId) != luid(path.sourceInfo.adapterId)
                    || mode.id != path.sourceInfo.id
                {
                    return Err("Windows returned an inconsistent display source binding.".into());
                }
                let s = unsafe { mode.Anonymous.sourceMode };
                if s.width == 0 || s.height == 0 || !(1..=4).contains(&path.targetInfo.rotation) {
                    return Err("An active monitor has invalid size or orientation.".into());
                }
                Some(Geometry {
                    x: s.position.x,
                    y: s.position.y,
                    width: s.width,
                    height: s.height,
                    rotation: path.targetInfo.rotation as u32,
                })
            } else {
                None
            };
            entries.push(Entry {
                path: *path,
                monitor: Monitor {
                    path: identity,
                    name: utf16(&target.monitorFriendlyDeviceName),
                    device,
                    active,
                    geometry,
                },
                source_index: active.then_some(source_index),
            });
        }
        return Ok(Snapshot {
            paths,
            modes: mode_data,
            entries,
        });
    }
    Err("Displays kept changing while Windows was queried. Refresh and try again.".into())
}

fn geometry(snapshot: &Snapshot) -> AppResult<BTreeMap<String, Geometry>> {
    let mut result = BTreeMap::new();
    let mut sources = BTreeSet::new();
    for e in snapshot.entries.iter().filter(|e| e.monitor.active) {
        if result
            .insert(
                e.monitor.path.clone(),
                e.monitor
                    .geometry
                    .clone()
                    .ok_or("Missing active geometry.")?,
            )
            .is_some()
            || !sources.insert(source(&e.path))
        {
            return Err("Mirrored displays or duplicate physical targets are not supported. Use an extended desktop.".into());
        }
    }
    if result.is_empty() || result.values().filter(|g| g.x == 0 && g.y == 0).count() != 1 {
        return Err("An extended desktop with exactly one primary monitor is required.".into());
    }
    Ok(result)
}

pub fn detect() -> AppResult<Vec<Monitor>> {
    let snapshot = query(true)?;
    geometry(&snapshot)?;
    let mut result = BTreeMap::new();
    for e in snapshot.entries {
        let replace = result
            .get(&e.monitor.path)
            .is_none_or(|m: &Monitor| !m.active && e.monitor.active);
        if replace {
            result.insert(e.monitor.path.clone(), e.monitor);
        }
    }
    Ok(result.into_values().collect())
}

fn call(paths: &[DISPLAYCONFIG_PATH_INFO], modes: &[DISPLAYCONFIG_MODE_INFO], flags: u32) -> i32 {
    unsafe {
        SetDisplayConfig(
            paths.len() as u32,
            paths.as_ptr(),
            modes.len() as u32,
            if modes.is_empty() {
                ptr::null()
            } else {
                modes.as_ptr()
            },
            flags,
        )
    }
}

fn persist(snapshot: &Snapshot, allow_changes: bool) -> AppResult<()> {
    persist_with(snapshot, allow_changes, call)
}

fn persist_with(
    snapshot: &Snapshot,
    allow_changes: bool,
    mut submit: impl FnMut(&[DISPLAYCONFIG_PATH_INFO], &[DISPLAYCONFIG_MODE_INFO], u32) -> i32,
) -> AppResult<()> {
    let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | if allow_changes { SDC_ALLOW_CHANGES } else { 0 };
    let code = submit(&snapshot.paths, &snapshot.modes, flags | SDC_VALIDATE);
    if code != 0 {
        return Err(format!("Arrangement validation failed ({code})."));
    }
    let code = submit(
        &snapshot.paths,
        &snapshot.modes,
        flags | SDC_APPLY | SDC_SAVE_TO_DATABASE,
    );
    if code != 0 {
        return Err(format!("Arrangement application failed ({code})."));
    }
    Ok(())
}

fn same_rate(a: DISPLAYCONFIG_RATIONAL, b: DISPLAYCONFIG_RATIONAL) -> bool {
    if a.Denominator == 0 || b.Denominator == 0 {
        return a.Numerator == b.Numerator && a.Denominator == b.Denominator;
    }
    u64::from(a.Numerator) * u64::from(b.Denominator)
        == u64::from(b.Numerator) * u64::from(a.Denominator)
}

fn target_signal(snapshot: &Snapshot, entry: &Entry) -> AppResult<DISPLAYCONFIG_VIDEO_SIGNAL_INFO> {
    // QueryDisplayConfig is deliberately not virtual-mode-aware, so this is an ordinary index.
    let index = unsafe { entry.path.targetInfo.Anonymous.modeInfoIdx } as usize;
    let mode = snapshot
        .modes
        .get(index)
        .ok_or("Missing target mode during rollback verification.")?;
    if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_TARGET
        || luid(mode.adapterId) != luid(entry.path.targetInfo.adapterId)
        || mode.id != entry.path.targetInfo.id
    {
        return Err("Inconsistent target mode during rollback verification.".into());
    }
    Ok(unsafe { mode.Anonymous.targetMode.targetVideoSignalInfo })
}

fn same_signal(a: DISPLAYCONFIG_VIDEO_SIGNAL_INFO, b: DISPLAYCONFIG_VIDEO_SIGNAL_INFO) -> bool {
    a.pixelRate == b.pixelRate
        && same_rate(a.hSyncFreq, b.hSyncFreq)
        && same_rate(a.vSyncFreq, b.vSyncFreq)
        && a.activeSize.cx == b.activeSize.cx
        && a.activeSize.cy == b.activeSize.cy
        && a.totalSize.cx == b.totalSize.cx
        && a.totalSize.cy == b.totalSize.cy
        && a.scanLineOrdering == b.scanLineOrdering
        // Compare video standard and VSync divider, not the ten reserved bits or struct padding.
        && unsafe { a.Anonymous.videoStandard & 0x003f_ffff == b.Anonymous.videoStandard & 0x003f_ffff }
}

fn verify_snapshot(expected: &Snapshot, actual: &Snapshot) -> AppResult<()> {
    verify_geometry(&geometry(expected)?, &geometry(actual)?)?;
    for entry in expected.entries.iter().filter(|e| e.monitor.active) {
        let observed = actual
            .entries
            .iter()
            .find(|e| e.monitor.active && e.monitor.path == entry.monitor.path)
            .ok_or("Missing physical monitor during rollback verification.")?;
        let source_pixel_format = |snapshot: &Snapshot, e: &Entry| -> AppResult<i32> {
            let mode = e
                .source_index
                .and_then(|i| snapshot.modes.get(i))
                .ok_or("Missing source mode during rollback verification.")?;
            if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                || luid(mode.adapterId) != luid(e.path.sourceInfo.adapterId)
                || mode.id != e.path.sourceInfo.id
            {
                return Err("Inconsistent source mode during rollback verification.".into());
            }
            Ok(unsafe { mode.Anonymous.sourceMode.pixelFormat })
        };
        let target = entry.path.targetInfo;
        let observed_target = observed.path.targetInfo;
        if source_pixel_format(expected, entry)? != source_pixel_format(actual, observed)?
            || target.scaling != observed_target.scaling
            || !same_rate(target.refreshRate, observed_target.refreshRate)
            || target.scanLineOrdering != observed_target.scanLineOrdering
            || !same_signal(
                target_signal(expected, entry)?,
                target_signal(actual, observed)?,
            )
        {
            return Err(format!(
                "Rollback mode verification failed for {}: source format, scaling, refresh rate or target timing differs.",
                entry.monitor.path
            ));
        }
    }
    Ok(())
}

fn wait_for_snapshot(expected: &Snapshot) -> AppResult<()> {
    let mut error = String::new();
    for _ in 0..10 {
        match query(false).and_then(|actual| verify_snapshot(expected, &actual)) {
            Ok(()) => return Ok(()),
            Err(e) => error = e,
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(error)
}

fn restore_with(
    mut submit: impl FnMut(bool) -> AppResult<()>,
    verify: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    if let Err(strict) = submit(false) {
        // Keep best-effort recovery available, but never call it restored without full verification.
        submit(true).map_err(|recovery| {
            format!("Strict restoration failed: {strict} Recovery also failed: {recovery}")
        })?;
    }
    verify()
}

fn wait_for(expected: &BTreeMap<String, Geometry>) -> AppResult<()> {
    let mut error = String::new();
    for _ in 0..10 {
        match query(false)
            .and_then(|s| geometry(&s))
            .and_then(|g| verify_geometry(expected, &g))
        {
            Ok(()) => return Ok(()),
            Err(e) => error = e,
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err(error)
}

fn correction(snapshot: &Snapshot, desired: &BTreeMap<String, Geometry>) -> AppResult<Snapshot> {
    let mut adjusted = snapshot.clone();
    for e in &snapshot.entries {
        let g = desired
            .get(&e.monitor.path)
            .ok_or("Unexpected monitor during arrangement correction.")?;
        let index = e
            .source_index
            .ok_or("No source mode for arrangement correction.")?;
        let mut s = unsafe { adjusted.modes[index].Anonymous.sourceMode };
        s.position.x = g.x;
        s.position.y = g.y;
        s.width = g.width;
        s.height = g.height;
        adjusted.modes[index].Anonymous.sourceMode = s;
    }
    for p in &mut adjusted.paths {
        let entry = snapshot
            .entries
            .iter()
            .find(|e| source(&e.path) == source(p))
            .ok_or("Invalid path binding.")?;
        let g = &desired[&entry.monitor.path];
        p.targetInfo.rotation = g.rotation as i32;
        if entry.monitor.geometry.as_ref().is_some_and(|old| {
            old.width != g.width || old.height != g.height || old.rotation != g.rotation
        }) {
            // Request fresh target timing when source dimensions/orientation change.
            p.targetInfo.Anonymous.modeInfoIdx = u32::MAX;
        }
    }
    adjusted.paths.sort_by_key(|p| {
        let e = snapshot
            .entries
            .iter()
            .find(|e| source(&e.path) == source(p))
            .unwrap();
        let g = &desired[&e.monitor.path];
        !(g.x == 0 && g.y == 0)
    });
    Ok(adjusted)
}

fn routes(all: &Snapshot, wanted: &BTreeSet<String>) -> AppResult<Vec<DISPLAYCONFIG_PATH_INFO>> {
    let mut groups = Vec::new();
    for target in wanted {
        let mut entries: Vec<_> = all
            .entries
            .iter()
            .filter(|e| e.monitor.path == *target)
            .collect();
        if entries.is_empty() {
            return Err(format!(
                "A monitor saved in this profile is disconnected: {target}. No displays were changed."
            ));
        }
        entries.sort_by_key(|e| (!e.monitor.active, source(&e.path)));
        groups.push(entries);
    }
    groups.sort_by_key(|g| g.len());
    fn assign(
        groups: &[Vec<&Entry>],
        i: usize,
        used: &mut BTreeSet<((i32, u32), u32)>,
        out: &mut Vec<DISPLAYCONFIG_PATH_INFO>,
        budget: &mut usize,
    ) -> bool {
        if i == groups.len() {
            return true;
        }
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        for e in &groups[i] {
            let s = source(&e.path);
            if used.insert(s) {
                out.push(e.path);
                if assign(groups, i + 1, used, out, budget) {
                    return true;
                }
                out.pop();
                used.remove(&s);
            }
        }
        false
    }
    let mut result = Vec::new();
    if !assign(&groups, 0, &mut BTreeSet::new(), &mut result, &mut 100000) {
        return Err("Windows has no unique extended-desktop route for this monitor set.".into());
    }
    for p in &mut result {
        p.flags |= DISPLAYCONFIG_PATH_ACTIVE;
        p.sourceInfo.Anonymous.modeInfoIdx = u32::MAX;
        p.targetInfo.Anonymous.modeInfoIdx = u32::MAX;
    }
    Ok(result)
}

#[derive(Debug)]
pub struct ApplyOutcome {
    pub message: String,
    pub quarantined: bool,
    pub success: bool,
}

/// Worker-thread transaction. The caller must serialise all mutation requests.
pub fn apply(wanted: &BTreeSet<String>, fallback: Option<&str>) -> ApplyOutcome {
    let fail = |message: String| ApplyOutcome {
        message,
        quarantined: false,
        success: false,
    };
    if wanted.is_empty() || wanted.iter().any(|p| !strong_path(p)) {
        return fail("Invalid or empty profile. No monitors were changed.".into());
    }
    let original = match query(false) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let before = match geometry(&original) {
        Ok(g) => g,
        Err(e) => return fail(e),
    };
    if before.keys().cloned().collect::<BTreeSet<_>>() == *wanted {
        return ApplyOutcome {
            message: "Current profile".into(),
            quarantined: false,
            success: true,
        };
    }
    if let Err(e) = verify_snapshot(&original, &original) {
        return fail(format!(
            "A complete rollback snapshot could not be captured: {e} No monitors were changed."
        ));
    }
    if wanted.iter().all(|target| before.contains_key(target)) {
        let remaining = before
            .iter()
            .filter(|(target, _)| wanted.contains(*target))
            .map(|(target, g)| (target.clone(), g.clone()))
            .collect();
        if let Err(e) = validate_arrangement(&remaining) {
            return fail(format!("{e} No monitors were changed."));
        }
    }
    let all = match query(true) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    if verify_snapshot(&original, &all).is_err() {
        return fail("Monitors changed during preflight. Refresh and try again.".into());
    }
    let paths = match routes(&all, wanted) {
        Ok(p) => p,
        Err(e) => return fail(e),
    };
    let mut flags = SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES;
    if call(&paths, &[], flags | SDC_VALIDATE) != 0 {
        flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        let code = call(&paths, &[], flags | SDC_VALIDATE);
        if code != 0 {
            return fail(format!(
                "Windows rejected the monitor set ({code}). No monitors were changed."
            ));
        }
    }
    let transaction = (|| -> AppResult<()> {
        let code = call(
            &paths,
            &[],
            flags
                | SDC_APPLY
                | if flags & SDC_USE_SUPPLIED_DISPLAY_CONFIG != 0 {
                    SDC_SAVE_TO_DATABASE
                } else {
                    0
                },
        );
        if code != 0 {
            return Err(format!("Windows activation failed ({code})."));
        }
        let mut current = None;
        let mut stable = 0;
        for _ in 0..10 {
            if let Ok(s) = query(false) {
                if let Ok(g) = geometry(&s) {
                    if g.keys().cloned().collect::<BTreeSet<_>>() == *wanted {
                        let desired = corrected_geometry(&before, &g, fallback)?;
                        if desired == g {
                            stable += 1;
                            if stable >= 2 {
                                return Ok(());
                            }
                        } else {
                            stable = 0;
                        }
                        current = Some((s, desired));
                    } else {
                        current = None;
                        stable = 0;
                    }
                } else {
                    current = None;
                    stable = 0;
                }
            } else {
                current = None;
                stable = 0;
            }
            thread::sleep(Duration::from_millis(200));
        }
        let (s, desired) = current.ok_or(
            "Windows activated a different monitor set. The request could not be verified.",
        )?;
        persist(&correction(&s, &desired)?, true)?;
        wait_for(&desired)
    })();
    finish_transaction(transaction, || {
        restore_with(
            |allow_changes| persist(&original, allow_changes),
            || wait_for_snapshot(&original),
        )
    })
}

fn finish_transaction(
    transaction: AppResult<()>,
    rollback: impl FnOnce() -> AppResult<()>,
) -> ApplyOutcome {
    match transaction {
        Ok(()) => ApplyOutcome {
            message: "Profile applied.".into(),
            quarantined: false,
            success: true,
        },
        Err(e) => {
            let restored = rollback();
            match restored {
                Ok(()) => ApplyOutcome {
                    message: format!("{e} The previous display setup was restored and verified."),
                    quarantined: false,
                    success: false,
                },
                Err(rollback) => ApplyOutcome {
                    message: format!(
                        "{e} Rollback could not be verified: {rollback} Further changes are locked; check Windows Display Settings."
                    ),
                    quarantined: true,
                    success: false,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(target: usize, source_id: u32, active: bool) -> Entry {
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.sourceInfo.id = source_id;
        path.targetInfo.targetAvailable = 1;
        path.targetInfo.id = target as u32;
        if active {
            path.flags = DISPLAYCONFIG_PATH_ACTIVE;
        }
        Entry {
            path,
            source_index: None,
            monitor: Monitor {
                path: format!(r"\\?\DISPLAY#MODEL#{target}#{{GUID}}"),
                name: String::new(),
                device: String::new(),
                active,
                geometry: None,
            },
        }
    }
    #[test]
    fn source_assignment_backtracks_instead_of_cloning() {
        let entries = vec![entry(1, 0, true), entry(1, 1, false), entry(2, 0, false)];
        let wanted = entries.iter().map(|e| e.monitor.path.clone()).collect();
        let all = Snapshot {
            paths: vec![],
            modes: vec![],
            entries,
        };
        let selected = routes(&all, &wanted).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected.iter().map(source).collect::<BTreeSet<_>>().len(),
            2
        );
    }
    #[test]
    fn route_assignment_has_no_three_monitor_limit() {
        let entries: Vec<_> = (0..32).map(|i| entry(i, i as u32, false)).collect();
        let wanted = entries.iter().map(|e| e.monitor.path.clone()).collect();
        assert_eq!(
            routes(
                &Snapshot {
                    paths: vec![],
                    modes: vec![],
                    entries
                },
                &wanted
            )
            .unwrap()
            .len(),
            32
        );
    }
    #[test]
    fn missing_monitor_or_unassignable_routes_fail_closed() {
        let entries = vec![entry(1, 0, true), entry(2, 0, false)];
        let wanted = entries.iter().map(|e| e.monitor.path.clone()).collect();
        assert!(
            routes(
                &Snapshot {
                    paths: vec![],
                    modes: vec![],
                    entries
                },
                &wanted
            )
            .is_err()
        );
        assert!(
            routes(
                &Snapshot {
                    paths: vec![],
                    modes: vec![],
                    entries: vec![]
                },
                &wanted
            )
            .is_err()
        );
    }

    fn active_snapshot() -> Snapshot {
        let mut e = entry(1, 0, true);
        e.source_index = Some(0);
        e.path.sourceInfo.Anonymous.modeInfoIdx = 0;
        e.path.targetInfo.Anonymous.modeInfoIdx = 1;
        e.path.targetInfo.rotation = DISPLAYCONFIG_ROTATION_IDENTITY;
        e.path.targetInfo.scaling = DISPLAYCONFIG_SCALING_IDENTITY;
        e.path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
            Numerator: 60000,
            Denominator: 1000,
        };
        e.path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        e.monitor.geometry = Some(Geometry {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            rotation: 1,
        });
        let mut source = DISPLAYCONFIG_MODE_INFO::default();
        source.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        let mut source_mode = DISPLAYCONFIG_SOURCE_MODE::default();
        source_mode.width = 1920;
        source_mode.height = 1080;
        source_mode.pixelFormat = DISPLAYCONFIG_PIXELFORMAT_32BPP;
        source.Anonymous.sourceMode = source_mode;
        let mut target = DISPLAYCONFIG_MODE_INFO::default();
        target.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_TARGET;
        target.id = 1;
        let mut signal = DISPLAYCONFIG_VIDEO_SIGNAL_INFO::default();
        signal.pixelRate = 148_500_000;
        signal.hSyncFreq = DISPLAYCONFIG_RATIONAL {
            Numerator: 67500,
            Denominator: 1,
        };
        signal.vSyncFreq = e.path.targetInfo.refreshRate;
        signal.activeSize = DISPLAYCONFIG_2DREGION { cx: 1920, cy: 1080 };
        signal.totalSize = DISPLAYCONFIG_2DREGION { cx: 2200, cy: 1125 };
        signal.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE;
        target.Anonymous.targetMode = DISPLAYCONFIG_TARGET_MODE {
            targetVideoSignalInfo: signal,
        };
        Snapshot {
            paths: vec![e.path],
            entries: vec![e],
            modes: vec![source, target],
        }
    }

    fn alter_signal(
        snapshot: &mut Snapshot,
        alter: impl FnOnce(&mut DISPLAYCONFIG_VIDEO_SIGNAL_INFO),
    ) {
        let mut signal = unsafe { snapshot.modes[1].Anonymous.targetMode.targetVideoSignalInfo };
        alter(&mut signal);
        snapshot.modes[1].Anonymous.targetMode = DISPLAYCONFIG_TARGET_MODE {
            targetVideoSignalInfo: signal,
        };
    }

    #[test]
    fn rollback_verification_checks_format_scaling_refresh_and_complete_timing() {
        let original = active_snapshot();
        assert!(verify_snapshot(&original, &original).is_ok());
        let changes: [fn(&mut Snapshot); 6] = [
            |s| s.entries[0].path.targetInfo.scaling = DISPLAYCONFIG_SCALING_CENTERED,
            |s| s.entries[0].path.targetInfo.refreshRate.Numerator = 75000,
            |s| {
                let mut mode = unsafe { s.modes[0].Anonymous.sourceMode };
                mode.pixelFormat = DISPLAYCONFIG_PIXELFORMAT_24BPP;
                s.modes[0].Anonymous.sourceMode = mode;
            },
            |s| alter_signal(s, |signal| signal.pixelRate += 1),
            |s| alter_signal(s, |signal| signal.totalSize.cx += 1),
            |s| alter_signal(s, |signal| signal.vSyncFreq.Numerator = 75000),
        ];
        for change in changes {
            let mut actual = original.clone();
            change(&mut actual);
            assert!(verify_snapshot(&original, &actual).is_err());
        }
        let mut invalid_binding = original.clone();
        invalid_binding.modes[1].id += 1;
        assert!(verify_snapshot(&original, &invalid_binding).is_err());
    }

    #[test]
    fn rollback_verification_uses_semantics_not_indices_padding_or_fraction_encoding() {
        let original = active_snapshot();
        let mut actual = original.clone();
        actual.entries[0].path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL {
            Numerator: 60,
            Denominator: 1,
        };
        alter_signal(&mut actual, |signal| {
            signal.vSyncFreq = DISPLAYCONFIG_RATIONAL {
                Numerator: 60,
                Denominator: 1,
            };
            signal.Anonymous.videoStandard = 0xffc0_0000;
        });
        actual.modes.swap(0, 1);
        actual.entries[0].source_index = Some(1);
        actual.entries[0].path.sourceInfo.Anonymous.modeInfoIdx = 1;
        actual.entries[0].path.targetInfo.Anonymous.modeInfoIdx = 0;
        assert!(verify_snapshot(&original, &actual).is_ok());
        assert!(!same_rate(
            DISPLAYCONFIG_RATIONAL::default(),
            DISPLAYCONFIG_RATIONAL {
                Numerator: 60,
                Denominator: 1
            }
        ));
    }

    #[test]
    fn strict_snapshot_submission_never_allows_mode_changes_and_validates_first() {
        let original = active_snapshot();
        let mut flags = Vec::new();
        persist_with(&original, false, |_, _, flag| {
            flags.push(flag);
            0
        })
        .unwrap();
        assert_eq!(
            flags,
            vec![
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_VALIDATE,
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_SAVE_TO_DATABASE
            ]
        );
        assert!(flags.iter().all(|flag| flag & SDC_ALLOW_CHANGES == 0));
        flags.clear();
        assert!(
            persist_with(&original, false, |_, _, flag| {
                flags.push(flag);
                87
            })
            .is_err()
        );
        assert_eq!(flags.len(), 1);
    }

    #[test]
    fn rollback_tries_strict_before_recovery_and_never_skips_full_verification() {
        let mut attempts = Vec::new();
        let verified = std::cell::Cell::new(false);
        restore_with(
            |allow_changes| {
                attempts.push(allow_changes);
                Ok(())
            },
            || {
                verified.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(attempts, vec![false]);
        assert!(verified.get());
        attempts.clear();
        let result = restore_with(
            |allow_changes| {
                attempts.push(allow_changes);
                if allow_changes {
                    Ok(())
                } else {
                    Err("Strict request rejected".into())
                }
            },
            || Err("Wrong refresh rate".into()),
        );
        assert_eq!(attempts, vec![false, true]);
        assert!(result.unwrap_err().contains("Wrong refresh rate"));
        let outcome = finish_transaction(Err("Activation failed".into()), || {
            restore_with(
                |_| Err("Display unavailable".into()),
                || panic!("Failed restoration cannot be verified"),
            )
        });
        assert!(outcome.quarantined);
        assert!(!outcome.success);
    }

    #[test]
    fn failed_verification_rolls_back_and_unverified_rollback_quarantines() {
        let called = std::cell::Cell::new(false);
        let result = finish_transaction(Err("Wrong rotation".into()), || {
            called.set(true);
            Ok(())
        });
        assert!(called.get());
        assert!(!result.success);
        assert!(!result.quarantined);
        assert!(result.message.contains("restored and verified"));
        let result = finish_transaction(Err("Missing monitor".into()), || {
            Err("Device removed".into())
        });
        assert!(result.quarantined);
        assert!(!result.success);
        assert!(
            finish_transaction(Ok(()), || panic!(
                "Successful transactions must not roll back"
            ))
            .success
        );
    }
}
