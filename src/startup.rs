//! Per-user Windows logon registration. Only our own named value is modified.
use crate::model::AppResult;
use std::{path::Path, ptr};
use windows_sys::{Win32::System::Registry::*, core::w};

const RUN: windows_sys::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: windows_sys::core::PCWSTR = w!("MonitorSwitcher");
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

pub fn command(executable: &Path) -> AppResult<String> {
    let text = executable
        .to_str()
        .ok_or("The application path is not valid Unicode.")?;
    if !executable.is_absolute()
        || text.contains(['"', '\0'])
        || !executable
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err("An absolute executable path is required for Windows startup.".into());
    }
    let result = format!("\"{text}\"");
    if result.encode_utf16().count() > 260 {
        return Err("The application path is too long for Windows startup registration.".into());
    }
    Ok(result)
}

fn read() -> AppResult<Option<String>> {
    let mut handle = ptr::null_mut();
    let code = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, 0, KEY_QUERY_VALUE, &mut handle) };
    if code == 2 {
        return Ok(None);
    }
    if code != 0 {
        return Err(format!(
            "Unable to read Start with Windows (Windows error {code})."
        ));
    }
    let handle = Key(handle);
    for _ in 0..3 {
        let (mut kind, mut size) = (0, 0);
        let code = unsafe {
            RegQueryValueExW(
                handle.0,
                VALUE,
                ptr::null(),
                &mut kind,
                ptr::null_mut(),
                &mut size,
            )
        };
        if code == 2 {
            return Ok(None);
        }
        if code != 0 {
            return Err(format!("Unable to read the startup entry ({code})."));
        }
        if kind != REG_SZ || size > 8192 || size % 2 != 0 {
            return Err(
                "The existing startup entry has an unsupported format; it was preserved.".into(),
            );
        }
        let mut data = vec![0_u16; size as usize / 2];
        let code = unsafe {
            RegQueryValueExW(
                handle.0,
                VALUE,
                ptr::null(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if code == 234 {
            continue;
        }
        if code != 0 || kind != REG_SZ {
            return Err(format!("Unable to read the startup entry ({code})."));
        }
        data.truncate(size as usize / 2);
        while data.last() == Some(&0) {
            data.pop();
        }
        return String::from_utf16(&data)
            .map(Some)
            .map_err(|_| "The startup entry is invalid Unicode; it was preserved.".into());
    }
    Err("The Windows startup entry kept changing. Try saving again.".into())
}

fn write(value: Option<&str>) -> AppResult<()> {
    let mut handle = ptr::null_mut();
    let code = if value.is_some() {
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                RUN,
                0,
                ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                ptr::null(),
                &mut handle,
                ptr::null_mut(),
            )
        }
    } else {
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, 0, KEY_SET_VALUE, &mut handle) }
    };
    if code == 2 && value.is_none() {
        return Ok(());
    }
    if code != 0 {
        return Err(format!(
            "Unable to change Start with Windows (Windows error {code})."
        ));
    }
    let handle = Key(handle);
    let code = if let Some(value) = value {
        let data: Vec<_> = value.encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetValueExW(
                handle.0,
                VALUE,
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(handle.0, VALUE) }
    };
    if code == 0 || (code == 2 && value.is_none()) {
        Ok(())
    } else {
        Err(format!(
            "Unable to change Start with Windows (Windows error {code})."
        ))
    }
}

fn plan(existing: Option<&str>, expected: &str, enabled: bool) -> AppResult<Option<String>> {
    if existing.is_some_and(|value| {
        !value.trim().eq_ignore_ascii_case(expected)
            && !value
                .trim()
                .eq_ignore_ascii_case(expected.trim_matches('"'))
    }) {
        return Err("Windows startup points to another Monitor Switcher copy. Its entry was preserved; update that registration before saving this setting.".into());
    }
    Ok(enabled.then(|| expected.to_owned()))
}

pub fn registered() -> AppResult<bool> {
    let expected = command(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    let existing = read()?;
    plan(existing.as_deref(), &expected, true)?;
    Ok(existing.is_some())
}

/// Restore registration if saving the preferences file fails.
pub fn save(enabled: bool, persist: impl FnOnce() -> AppResult<()>) -> AppResult<()> {
    let expected = command(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    let previous = read()?;
    let desired = plan(previous.as_deref(), &expected, enabled)?;
    let changed = previous != desired;
    if changed {
        write(desired.as_deref())?;
    }
    if let Err(error) = persist() {
        if changed {
            if let Err(rollback) = write(previous.as_deref()) {
                return Err(format!(
                    "{error} Startup registration could not be restored: {rollback}"
                ));
            }
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quotes_paths_with_spaces_and_rejects_invalid_commands() {
        assert_eq!(
            command(Path::new(
                r"C:\Program Files\Monitor Switcher\MonitorSwitcher.exe"
            ))
            .unwrap(),
            r#""C:\Program Files\Monitor Switcher\MonitorSwitcher.exe""#
        );
        assert!(command(Path::new("relative.exe")).is_err());
        assert!(command(Path::new(r"C:\App\startup.cmd")).is_err());
        assert!(command(Path::new(&format!(r"C:\{}\App.exe", "a".repeat(260)))).is_err());
    }
    #[test]
    fn registration_targets_only_the_current_app() {
        let expected = r#""C:\Apps\MonitorSwitcher.exe""#;
        assert_eq!(plan(None, expected, true).unwrap(), Some(expected.into()));
        assert_eq!(plan(Some(expected), expected, false).unwrap(), None);
        assert_eq!(plan(None, expected, false).unwrap(), None);
        assert!(plan(Some(r#""C:\Other\MonitorSwitcher.exe""#), expected, true).is_err());
        assert!(plan(Some(r#""C:\Other\MonitorSwitcher.exe""#), expected, false).is_err());
    }
}
