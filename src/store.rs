use crate::model::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub fn data_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("WorkMonitorSwitcher")
}
fn read(path: &Path) -> AppResult<String> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_FILE_BYTES {
        return Err("Settings/profile file exceeds 2 MiB.".into());
    }
    fs::read_to_string(path)
        .map(|s| s.trim_start_matches('\u{feff}').to_owned())
        .map_err(|e| e.to_string())
}

pub fn load(dir: &Path) -> AppResult<(Settings, String)> {
    let path = dir.join("settings.json");
    if !path.exists() && !dir.join("settings.json.bak").exists() {
        let legacy = dir.parent().unwrap_or(dir).join("WorkMonitorSwitcher");
        let (s, w) = import_legacy(&legacy)?;
        save(dir, &s)?;
        return Ok((s, w));
    }
    let parse = |path: &Path| -> AppResult<Settings> {
        let s: Settings = serde_json::from_str(&read(path)?).map_err(|e| e.to_string())?;
        s.validate()?;
        Ok(s)
    };
    match parse(&path) {
        Ok(s) => Ok((s, String::new())),
        Err(e) => match parse(&dir.join("settings.json.bak")) {
            Ok(s) => Ok((s, format!("Recovered settings from backup: {e}"))),
            Err(_) => Err(format!(
                "Unable to read settings: {e}. Original files were preserved."
            )),
        },
    }
}

pub fn save(dir: &Path, settings: &Settings) -> AppResult<()> {
    settings.validate()?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("settings.json");
    let stage = dir.join("settings.json.pending");
    let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Settings exceed 2 MiB; the previous settings were preserved.".into());
    }
    use std::io::Write;
    let mut file = fs::File::create(&stage).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    if path.exists() {
        // Never replace the last good backup with corrupt settings.
        if read(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .is_some_and(|s| s.validate().is_ok())
        {
            fs::copy(&path, dir.join("settings.json.bak")).map_err(|e| e.to_string())?;
        }
    }
    let wide = |p: &Path| {
        p.as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    if unsafe {
        MoveFileExW(
            wide(&stage).as_ptr(),
            wide(&path).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(format!(
            "Unable to atomically save settings: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn parse_profile(contents: &str) -> AppResult<BTreeSet<String>> {
    let mut targets = BTreeSet::new();
    let mut version = false;
    for line in contents.lines().map(str::trim) {
        if line == "NativeProfileVersion=1" {
            version = true;
        }
        if let Some(p) = line.strip_prefix("MonitorDevicePath=") {
            if !strong_path(p) || !targets.insert(key(p)) {
                return Err("Profile has a missing or duplicate native target identity.".into());
            }
        }
    }
    if !version || targets.is_empty() {
        return Err(
            "Only native version-1 profiles with physical monitor identities can be imported."
                .into(),
        );
    }
    Ok(targets)
}

pub fn import_legacy(legacy: &Path) -> AppResult<(Settings, String)> {
    let mut settings = Settings::default();
    if !legacy.exists() {
        return Ok((
            settings,
            "Create a profile by saving your current enabled monitors.".into(),
        ));
    }
    let mut warnings = Vec::new();
    let profile_names: Vec<String> = read(&legacy.join("layouts/profiles.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| vec!["Default".into()]);
    for name in profile_names {
        if name.is_empty() || name.contains(['/', '\\', ':']) || name == "." || name == ".." {
            warnings.push("Skipped an invalid legacy profile name.".into());
            continue;
        }
        let path = if name.eq_ignore_ascii_case("Default") {
            legacy.join("monitor-layout.cfg")
        } else {
            legacy.join("layouts").join(format!("{name}.cfg"))
        };
        let import = (|| -> AppResult<Profile> {
            let targets = parse_profile(&read(&path)?)?;
            let sidecar: Value = serde_json::from_str(&read(&PathBuf::from(format!(
                "{}.identity.json",
                path.display()
            )))?)
            .map_err(|e| e.to_string())?;
            let identities = sidecar["Monitors"]
                .as_array()
                .ok_or("Missing identity sidecar monitors.")?;
            let side_targets: BTreeSet<_> = identities
                .iter()
                .filter_map(|i| i["NativeTargetPath"].as_str())
                .map(key)
                .collect();
            if side_targets != targets || identities.len() != targets.len() {
                return Err("The profile and its identity sidecar disagree.".into());
            }
            Ok(Profile {
                name: name.clone(),
                targets,
            })
        })();
        match import {
            Ok(p) => settings.profiles.push(p),
            Err(e) => warnings.push(format!("Could not import {name}: {e}")),
        }
    }
    if let Ok(text) = read(&legacy.join("monitor-aliases.json")) {
        if let Ok(aliases) = serde_json::from_str::<BTreeMap<String, Value>>(&text) {
            let mut order = Vec::new();
            for alias in aliases.values() {
                let Some(path) = alias["LastNativeTargetPath"]
                    .as_str()
                    .filter(|p| strong_path(p))
                else {
                    continue;
                };
                let path = key(path);
                if let Some(name) = alias["Name"].as_str() {
                    settings.aliases.insert(path.clone(), name.to_owned());
                }
                order.push((
                    alias["PreferredOrder"].as_i64().unwrap_or(i64::MAX),
                    path.clone(),
                ));
                if alias["IsFallbackPrimary"].as_bool() == Some(true) {
                    settings.fallback = Some(path);
                }
            }
            order.sort();
            settings.order = order.into_iter().map(|(_, p)| p).collect();
        }
    }
    if let Ok(text) = read(&legacy.join("ui-settings.json")) {
        if let Ok(ui) = serde_json::from_str::<Value>(&text) {
            settings.selected = ui["SelectedLayoutProfile"]
                .as_str()
                .unwrap_or("Default")
                .to_owned();
            settings.preferences.dark = ui["DarkMode"].as_bool().unwrap_or(true);
            settings.preferences.always_on_top = ui["AlwaysOnTop"].as_bool().unwrap_or(false);
            settings.preferences.minimise_to_tray = ui["MinimizeToTray"].as_bool().unwrap_or(true);
            settings.preferences.confirm_disabling =
                ui["ConfirmBeforeDisable"].as_bool().unwrap_or(true);
        }
    }
    if !settings
        .profiles
        .iter()
        .any(|p| p.name == settings.selected)
    {
        settings.selected = settings
            .profiles
            .first()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Default".into());
    }
    settings.validate()?;
    let imported = format!(
        "Imported {} profiles and {} aliases. Automatic profile application is disabled.",
        settings.profiles.len(),
        settings.aliases.len()
    );
    Ok((
        settings,
        if warnings.is_empty() {
            imported
        } else {
            format!("{imported} {}", warnings.join(" "))
        },
    ))
}

pub fn log(dir: &Path, message: &str) {
    let _ = fs::create_dir_all(dir);
    let path = dir.join("diagnostics.log");
    let previous = fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<_> = previous.lines().rev().take(199).collect();
    let mut text = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
    text.push_str(&format!(
        "\n[{}] {message}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    ));
    let _ = fs::write(path, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_settings_recovers_the_existing_backup_instead_of_reimporting() {
        let dir = std::env::temp_dir().join(format!("monitor-backup-test-{}", std::process::id()));
        let settings = Settings::default()
            .with_profile(
                "Saved",
                BTreeSet::from([r"\\?\DISPLAY#MODEL#A#{GUID}".into()]),
            )
            .unwrap();
        save(&dir, &settings).unwrap();
        fs::rename(dir.join("settings.json"), dir.join("settings.json.bak")).unwrap();
        let (recovered, warning) = load(&dir).unwrap();
        assert_eq!(recovered.selected, "Saved");
        assert_eq!(recovered.profiles.len(), 1);
        assert!(warning.contains("Recovered"));
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn oversized_settings_do_not_overwrite_the_readable_copy() {
        let dir = std::env::temp_dir().join(format!("monitor-size-test-{}", std::process::id()));
        let settings = Settings::default();
        save(&dir, &settings).unwrap();
        let mut oversized = settings.clone();
        oversized
            .aliases
            .insert("test".into(), "x".repeat(MAX_FILE_BYTES as usize));
        assert!(save(&dir, &oversized).is_err());
        assert!(load(&dir).unwrap().0.aliases.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn import_rejects_weak_or_duplicate_paths() {
        assert!(parse_profile("NativeProfileVersion=1\nMonitorDevicePath=DISPLAY1").is_err());
        let p = r"\\?\DISPLAY#A#B#{GUID}";
        assert!(
            parse_profile(&format!(
                "NativeProfileVersion=1\nMonitorDevicePath={p}\nMonitorDevicePath={p}"
            ))
            .is_err()
        );
    }
    #[test]
    fn corruption_does_not_destroy_last_good_backup() {
        let dir = std::env::temp_dir().join(format!(
            "monitor-rust-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = Settings::default();
        save(&dir, &a).unwrap();
        save(&dir, &a).unwrap();
        fs::write(dir.join("settings.json"), "{broken").unwrap();
        let (recovered, warning) = load(&dir).unwrap();
        assert!(!warning.is_empty());
        save(&dir, &recovered).unwrap();
        assert!(load(&dir).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }
}
