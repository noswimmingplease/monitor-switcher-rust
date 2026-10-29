<div lang="en-GB">

# Monitor Switcher 1.0

Monitor profiles and display controls for Windows 10 and Windows 11 (x64).

## Download and run

Download **MonitorSwitcher.exe**, save it in a permanent folder and open it.
No installer, .NET runtime, WebView2 or NirSoft monitor tool is required.
Application assets, fonts and licence texts are embedded. Windows supplies the
system libraries and graphics driver.

If an older version is running, choose **Exit** from its notification-area menu
before replacing it. Compatible profiles and preferences are retained locally
in `%APPDATA%\WorkMonitorSwitcher`.

## Included

- Turn off displays after a two-second countdown. Move the mouse or press a key
  to wake them. This does not disable monitors or change the saved profile.
- Status messages clear automatically; safety-lock warnings remain visible.

- High CPU use while hidden in the notification area. The GUI framework now
  sleeps between background deadlines instead of spinning on hidden-window
  repaints; the hidden app no longer builds or paints its interface.
- Safer profile activation and fallback-primary selection. Retained monitor
  positions and orientation are checked, including portrait displays.
- Stronger rollback verification, including display modes, refresh rate and
  scaling. If rollback cannot be verified, further display changes are blocked
  until the app is restarted after checking Windows Display Settings.
- Safer profile deletion, monitor-name editing and card reordering, including
  protection against tray actions conflicting with open confirmation dialogues.
- Settings recovery that preserves unsupported newer formats and good backups
  rather than overwriting them with incompatible or damaged data.
- Tray-icon recovery with bounded retries. The window remains accessible if its
  notification-area icon cannot be registered, and Exit waits for display work.
- Release checks covering staged files, local paths, hardware identifiers and
  dependency advisories.

Profiles still control which monitors are enabled. Arrange and rotate displays
in Windows Display Settings. Layouts that cannot safely be retained are rejected
rather than silently rearranging monitors.

## Important

This executable is **not Authenticode-signed**. Windows SmartScreen may show an
unknown publisher or unrecognised app warning. Use the official release download
and compare its SHA-256 with **SHA256SUMS.txt**. No security settings are changed
by the app or need to be disabled.

Extended desktops are supported; mirrored/duplicated displays are not. A profile
containing a physically disconnected monitor fails safely rather than disabling
the remaining displays. Switching and physical reconnection remain dependent on
Windows and the graphics driver. No automatic update downloader is included.

The dependency check reports three reviewed maintenance notices for non-Windows
dependencies and no unreviewed advisories at release time. These notices do not
affect the Windows dependency graph.

Automated tests do not change displays; physical switching, reconnection, display
wake and graphics-driver behaviour require hardware validation.

</div>
