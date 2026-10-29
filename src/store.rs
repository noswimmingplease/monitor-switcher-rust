use crate::model::*;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    os::windows::ffi::OsStrExt,
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
    read_utf8(path).map_err(|e| e.to_string())
}

fn read_utf8(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path).map(|s| s.trim_start_matches('\u{feff}').to_owned())
}

fn check_primary_size(path: &Path) -> AppResult<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_FILE_BYTES => Err("The primary settings file exceeds 2 MiB; the settings and backup were preserved. Restore a known compatible settings file before saving.".into()),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn supported_version(contents: &str) -> AppResult<()> {
    // Check the version before deserialising the current schema: a future
    // version may have different required fields and is not a corrupt file.
    if let Ok(value) = serde_json::from_str::<Value>(contents)
        && value
            .get("version")
            .is_some_and(|version| version.as_u64() != Some(1))
    {
        return Err("Unsupported settings version; the settings and backup were preserved. Use a compatible application version.".into());
    }
    Ok(())
}

fn parse_settings(contents: &str) -> AppResult<Settings> {
    let settings: Settings = serde_json::from_str(contents).map_err(|e| e.to_string())?;
    settings.validate()?;
    Ok(settings)
}

pub fn load(dir: &Path) -> AppResult<(Settings, String)> {
    let path = dir.join("settings.json");
    check_primary_size(&path)?;
    if !path.exists() && !dir.join("settings.json.bak").exists() {
        let legacy = dir.parent().unwrap_or(dir).join("WorkMonitorSwitcher");
        let (s, w) = import_legacy(&legacy)?;
        save(dir, &s)?;
        return Ok((s, w));
    }
    let primary = read(&path);
    if let Ok(contents) = &primary {
        supported_version(contents)?;
    }
    match primary.and_then(|contents| parse_settings(&contents)) {
        Ok(s) => Ok((s, String::new())),
        Err(e) => match read(&dir.join("settings.json.bak"))
            .and_then(|contents| parse_settings(&contents))
        {
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
    check_primary_size(&path)?;
    let previous = if path.exists() {
        match read_utf8(&path) {
            Ok(contents) => {
                supported_version(&contents)?;
                Some(contents)
            }
            // Invalid UTF-8 is corruption, not a valid future JSON schema.
            // Keep the good backup while replacing the malformed primary.
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => None,
            Err(error) => return Err(error.to_string()),
        }
    } else {
        None
    };
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
    // Never replace the last good backup with corrupt settings.
    if previous.is_some_and(|contents| parse_settings(&contents).is_ok()) {
        fs::copy(&path, dir.join("settings.json.bak")).map_err(|e| e.to_string())?;
    }
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
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
    fn recovered_non_utf8_settings_can_be_saved_without_losing_the_good_backup() {
        let dir =
            std::env::temp_dir().join(format!("monitor-non-utf8-test-{}", std::process::id()));
        let settings = Settings::default()
            .with_profile(
                "Saved",
                BTreeSet::from([r"\\?\DISPLAY#MODEL#A#{GUID}".into()]),
            )
            .unwrap();
        save(&dir, &settings).unwrap();
        save(&dir, &settings).unwrap();
        let backup = fs::read(dir.join("settings.json.bak")).unwrap();
        fs::write(dir.join("settings.json"), [0xff, 0xfe, 0]).unwrap();
        let (recovered, warning) = load(&dir).unwrap();
        assert!(warning.contains("Recovered"));
        assert_eq!(recovered.selected, "Saved");
        save(&dir, &recovered).unwrap();
        assert_eq!(fs::read(dir.join("settings.json.bak")).unwrap(), backup);
        let (saved, warning) = load(&dir).unwrap();
        assert_eq!(saved.selected, "Saved");
        assert!(warning.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn oversized_primary_is_preserved_and_never_loaded_as_writable_backup_recovery() {
        let dir = std::env::temp_dir().join(format!(
            "monitor-oversized-primary-test-{}",
            std::process::id()
        ));
        let settings = Settings::default();
        save(&dir, &settings).unwrap();
        save(&dir, &settings).unwrap();
        let backup = fs::read(dir.join("settings.json.bak")).unwrap();
        let oversized = format!(
            r#"{{"version":2,"future":"{}"}}"#,
            "x".repeat(MAX_FILE_BYTES as usize)
        );
        fs::write(dir.join("settings.json"), &oversized).unwrap();
        assert!(load(&dir).unwrap_err().contains("exceeds 2 MiB"));
        assert!(save(&dir, &settings).unwrap_err().contains("exceeds 2 MiB"));
        assert_eq!(
            fs::read_to_string(dir.join("settings.json")).unwrap(),
            oversized
        );
        assert_eq!(fs::read(dir.join("settings.json.bak")).unwrap(), backup);
        assert!(!dir.join("settings.json.pending").exists());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unsupported_primary_never_recovers_or_overwrites_an_older_backup() {
        let dir = std::env::temp_dir().join(format!(
            "monitor-version-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = Settings::default();
        save(&dir, &old).unwrap();
        save(&dir, &old).unwrap();
        let backup = fs::read(dir.join("settings.json.bak")).unwrap();
        // Deliberately omit fields required by version 1, as a future schema may.
        for future in [
            r#"{"version":2,"future_preferences":{"enabled":true}}"#,
            r#"{"version":"next","future_preferences":{"enabled":true}}"#,
        ] {
            fs::write(dir.join("settings.json"), future).unwrap();
            assert!(
                load(&dir)
                    .unwrap_err()
                    .contains("Unsupported settings version")
            );
            assert!(
                save(&dir, &old)
                    .unwrap_err()
                    .contains("Unsupported settings version")
            );
            assert_eq!(
                fs::read_to_string(dir.join("settings.json")).unwrap(),
                future
            );
            assert_eq!(fs::read(dir.join("settings.json.bak")).unwrap(), backup);
            assert!(!dir.join("settings.json.pending").exists());
        }
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn saves_windows_paths_without_lossy_unicode_conversion() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let mut name: Vec<u16> = format!("monitor-path-test-{}-", std::process::id())
            .encode_utf16()
            .collect();
        name.push(0xd800);
        let dir = std::env::temp_dir().join(OsString::from_wide(&name));
        assert!(!dir.exists());
        save(&dir, &Settings::default()).unwrap();
        assert!(load(&dir).is_ok());
        fs::remove_dir_all(dir).unwrap();
    }
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
