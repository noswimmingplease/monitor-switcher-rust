param([string]$Executable)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $Executable) { $Executable = Join-Path $projectRoot 'dist\MonitorSwitcher.exe' }

# Inspect exactly the files Git would publish, including currently untracked files.
Push-Location $projectRoot
try {
    $relativeFiles = @(git ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Unable to enumerate publishable files.' }
    $needles = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    if ($env:USERPROFILE) {
        [void]$needles.Add($env:USERPROFILE)
        [void]$needles.Add($env:USERPROFILE.Replace('\','/'))
    }
    [void]$needles.Add($projectRoot)
    [void]$needles.Add($projectRoot.Replace('\','/'))
    # Local data supplies private identifiers to look for; values are never printed.
    foreach ($directoryName in @('WorkMonitorSwitcher','MonitorSwitcherRust')) {
        $dataPath = Join-Path $env:APPDATA $directoryName
        $settingsPath = Join-Path $dataPath 'settings.json'
        if (Test-Path -LiteralPath $settingsPath) {
            $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
            foreach ($profile in $settings.profiles) {
                foreach ($identity in $profile.targets) {
                    if ($identity.Length -ge 8) {
                        [void]$needles.Add($identity)
                        [void]$needles.Add($identity.Replace('\','\\'))
                        $components = $identity.Split('#')
                        if ($components.Length -ge 4) { [void]$needles.Add($components[2]) }
                    }
                }
            }
        }
        $aliasesPath = Join-Path $dataPath 'monitor-aliases.json'
        if (Test-Path -LiteralPath $aliasesPath) {
            $aliases = Get-Content -LiteralPath $aliasesPath -Raw | ConvertFrom-Json
            foreach ($entry in $aliases.PSObject.Properties.Value) {
                if ($entry.LastSerialNumber -and $entry.LastSerialNumber.Length -ge 8) { [void]$needles.Add($entry.LastSerialNumber) }
            }
        }
    }
    $failures = [Collections.Generic.List[string]]::new()
    $tokenPattern = '(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|AKIA[0-9A-Z]{16}|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)'
    $files = @($relativeFiles | ForEach-Object { Join-Path $projectRoot $_ })
    if (Test-Path -LiteralPath $Executable) { $files += $Executable }
    foreach ($path in $files) {
        $bytes = [IO.File]::ReadAllBytes($path)
        $views = @([Text.Encoding]::UTF8.GetString($bytes),[Text.Encoding]::Unicode.GetString($bytes))
        if ($bytes.Length -gt 1) { $views += [Text.Encoding]::Unicode.GetString($bytes,1,$bytes.Length-1) }
        foreach ($view in $views) {
            $found = $view -match $tokenPattern
            if (-not $found) {
                foreach ($needle in $needles) {
                    if ($view.IndexOf($needle,[StringComparison]::OrdinalIgnoreCase) -ge 0) { $found = $true; break }
                }
            }
            if ($found) { $failures.Add([IO.Path]::GetRelativePath($projectRoot,$path)); break }
        }
        $isLicenceBundle = [IO.Path]::GetFullPath($path) -eq (Join-Path $projectRoot 'assets\THIRD-PARTY-LICENCES.txt')
        if ($path -ne $Executable -and -not $isLicenceBundle -and [IO.Path]::GetExtension($path) -ne '.ico') {
            $text = [Text.Encoding]::UTF8.GetString($bytes)
            # Required third-party copyright contacts stay in their licence bundle.
            if ($text -match '[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}') { $failures.Add([IO.Path]::GetRelativePath($projectRoot,$path)) }
        }
    }
    if ($failures.Count -gt 0) { throw ('Privacy audit failed for: ' + (($failures | Sort-Object -Unique) -join ', ') + '. Private values were not printed.') }
    foreach ($sample in @('settings.json','monitor-layout.cfg','monitor-inspection.json','.build/check.json','.env','backups/old.exe','dist/MonitorSwitcher.exe')) {
        git check-ignore -q -- $sample
        if ($LASTEXITCODE -ne 0) { throw "A private/output path is not ignored: $sample" }
    }
    [pscustomobject]@{ PublishableFiles=$relativeFiles.Count; ExecutableChecked=(Test-Path -LiteralPath $Executable); Result='PASS'; PersonalValuesPrinted=$false }
} finally { Pop-Location }
