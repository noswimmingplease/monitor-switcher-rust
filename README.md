---
lang: en-GB
---

# Monitor Switcher

Switch named monitor profiles on 64-bit Windows 10 and Windows 11. Fonts, icon,
licences and the application runtime are bundled in **MonitorSwitcher.exe**.
No installer, .NET, WebView2 or external monitor utility is required. Windows
supplies its standard libraries and your graphics driver.

## Download

[**Download MonitorSwitcher.exe — Windows x64**](https://github.com/noswimmingplease/monitor-switcher-rust/releases/latest/download/MonitorSwitcher.exe)

Save the executable in a permanent folder and open it. There is nothing to
install. To update, choose **Exit** from the old app's tray menu before replacing
the executable. Closing its window alone normally leaves it running in the tray.
Keep `%APPDATA%\WorkMonitorSwitcher` to retain your profiles and preferences.

[Release notes and SHA-256 checksums](https://github.com/noswimmingplease/monitor-switcher-rust/releases/latest)
are available alongside each download. Builds are unsigned, so Windows may show
a SmartScreen unknown publisher warning. Download only from this repository's
release page; no Windows security settings need to be disabled.

## Using the app

Choose a profile and click **Apply profile**. Apply is disabled when the enabled
physical monitor set already matches. Profile changes control membership;
Windows Display Settings controls the arrangement. If activation moves retained
displays, their live pre-change geometry is corrected and verified. Failures
trigger a verified rollback. An unverified rollback locks further changes until
the app is restarted after checking Windows Display Settings.
The app rejects overlapping or disconnected arrangements rather than moving
retained monitors silently. If preserving a layout is impossible, arrange the
displays in Windows Settings before trying again. Rollback checks the original
arrangement and display modes, including refresh rate and scaling.

Save the current enabled set as a named profile, rename monitors in Settings,
and drag only the three-line handle to reorder cards. The miniature arrangement
is read-only. Expanding monitor information shows the native physical identity.
Closing the window normally hides it in the notification area; its menu can
reopen the app or exit. Exit waits for an active display transaction to finish.
Use **Turn off displays** to put the displays to sleep after a two-second
countdown; mouse or keyboard input wakes them without changing the monitor set.
If the notification-area icon is unavailable, the window stays accessible while
registration is retried; it will not be hidden without a usable tray icon.
While hidden, only background logic runs: the app does not build or paint its
interface. Monitor detection and tray recovery use their next maintenance
deadline, and completed operations wake the app directly.
Opening the executable again also reopens the running app without applying a profile.
The tray menu follows the saved appearance and provides profile selection, Apply,
Refresh, Settings, Diagnostics and saved preference toggles, including Windows startup.
Selecting a profile does not apply it until you choose **Apply selected profile**.

An extended desktop is required; mirrored/duplicated displays are not supported.
If a profile contains a physically disconnected monitor, applying it fails safely
without disabling the remaining displays. Reconnect the monitor or save a new set.

Profiles, preferences and diagnostics are stored locally in
`%APPDATA%\WorkMonitorSwitcher`. Compatible profiles from earlier versions can
be imported on first launch. **Start with Windows** in Settings registers this
executable for your Windows account when you save; unticking it removes that
registration. There is currently no automatic update downloader.

## Build

Install Rust 1.95 or later (MSVC), Visual Studio C++ Build Tools and a Windows SDK:

```powershell
.\scripts\Build.ps1
# Or supply an existing MSVC Rust toolchain:
.\scripts\Build.ps1 -ToolchainBin 'D:\Tools\Rust\bin'
```

Dependencies are locked in `Cargo.lock`. The build checks formatting, runs tests
and builds a static-CRT executable under `dist`. To refresh embedded dependency
licences after changing the dependency lockfile, run
`scripts/New-LicenceBundle.ps1` with `cargo` available.
The Windows build workflow repeats the build and privacy checks on GitHub.
It also runs synthetic regression tests for the release checks and queries OSV
for advisories affecting the locked dependencies. Unreviewed advisories or an
incomplete advisory query fail the check. Previously reviewed maintenance
notices are reported explicitly; they are not blanket exemptions for security
findings or future dependency versions. To run these checks locally:

```powershell
.\scripts\Test-Privacy.ps1
.\scripts\Check-Dependencies.ps1 -SelfTest
.\scripts\Check-Dependencies.ps1
.\scripts\Check-Privacy.ps1
```

The dependency check sends only public package names and versions to OSV. The
application itself does not perform this query.

Read-only hardware inspection, without importing settings or changing displays:

```powershell
.\dist\MonitorSwitcher.exe --inspect --output .\.build\monitor-inspection.json
```

No display changes are executed by the automated tests. Actual switching,
physical disconnection, portrait orientation and graphics-driver behaviour
require hardware validation.

## Privacy

Profiles contain local monitor identities, aliases and preferences. Diagnostics
can contain hardware details. Keep these in the application data directory or
the ignored `.build` directory. They are not part of the application source or
the distributed executable. Local build and user directories are remapped out
of the executable by the build script. No telemetry is sent by the app.
The privacy check scans both staged Git content and working/untracked files,
including escaped local paths. Settings from an unsupported newer format are
preserved: an older app will not replace them with its older backup.

## Licence and unsigned builds

GPL-3.0-only. The original application icon and original application are
Copyright © 2026 Ci303 / noswimmingplease. Third-party licences and the GPL are
embedded and available in Settings. The native backend uses Windows display
APIs directly and can import compatible native profiles from earlier versions.

Builds are not Authenticode-signed. Windows may show a SmartScreen unknown
publisher warning. No security settings are changed by the application.
