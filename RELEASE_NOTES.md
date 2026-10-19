<div lang="en-GB">

# Monitor Switcher 0.1.0

The first Rust release of Monitor Switcher for Windows 10 and Windows 11 (x64).

## Download and run

Download **MonitorSwitcher.exe**, save it in a permanent folder and open it.
No installer, .NET runtime, WebView2 or NirSoft monitor tool is required.
Application assets, fonts and licence texts are embedded. Windows supplies the
system libraries and graphics driver.

If an older version is running, choose **Exit** from its notification-area menu
before replacing it. Compatible profiles and preferences are retained locally
in `%APPDATA%\WorkMonitorSwitcher`.

## Included

- Native Windows monitor enable/disable and named monitor profiles.
- Membership-only profiles: Windows Display Settings remains the source of
  monitor arrangement and orientation, with verification and rollback safeguards.
- Reorderable monitor cards, custom names and expandable monitor information.
- Consistent light/dark appearance, readable selection colours and status bars.
- A themed tray menu with profile selection, Apply, Settings and Diagnostics.
- Start with Windows, always-on-top and confirmation preferences, with immediate
  tray-setting updates.
- A single-instance app: opening the executable again restores the running window.
- GPL-3.0-only source, embedded third-party notices and no application telemetry.

## Important

This executable is **not Authenticode-signed**. Windows SmartScreen may show an
unknown publisher or unrecognised app warning. Use the official release download
and compare its SHA-256 with **SHA256SUMS.txt**. No security settings are changed
by the app or need to be disabled.

Extended desktops are supported; mirrored/duplicated displays are not. A profile
containing a physically disconnected monitor fails safely rather than disabling
the remaining displays. Switching and physical reconnection remain dependent on
Windows and the graphics driver. No automatic update downloader is included.

The font-rendering dependency currently carries an informational
[ttf-parser maintenance advisory](https://rustsec.org/advisories/RUSTSEC-2026-0192.html).
No reported security vulnerability was returned by the dependency scan for this
release. The app uses its embedded fonts; it does not import user-supplied fonts.

Validation: 28 automated tests passed, release and privacy checks passed, and
appearance switching, monitor information and immediate tray-setting updates
were checked on the test installation. Automated tests do not change displays.

</div>
