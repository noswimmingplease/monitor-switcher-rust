param([string]$Executable)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $Executable) { $Executable = Join-Path $projectRoot 'dist\MonitorSwitcher.exe' }

# Read Git output as bytes, so staged binary files and unusual filenames are
# checked without text conversion, path quoting or shell interpolation.
function Read-GitBytes([string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new('git')
    $start.WorkingDirectory = $projectRoot
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $output = [IO.MemoryStream]::new()
    try {
        [void]$process.Start()
        $errorOutput = $process.StandardError.ReadToEndAsync()
        $process.StandardOutput.BaseStream.CopyTo($output)
        $process.WaitForExit()
        [void]$errorOutput.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw 'Unable to inspect publishable Git content.' }
        return ,$output.ToArray()
    } finally {
        $output.Dispose()
        $process.Dispose()
    }
}

function Add-PrivateNeedle([string]$Value) {
    if ([string]::IsNullOrEmpty($Value)) { return }
    [void]$needles.Add($Value)
    [void]$needles.Add($Value.Replace('\','/'))
    # Windows paths in JSON and ordinary Rust string literals escape backslashes.
    [void]$needles.Add($Value.Replace('\','\\'))
}

function Test-PrivateBytes([byte[]]$Bytes, [string]$RelativePath, [bool]$IsExecutable = $false) {
    $views = @([Text.Encoding]::UTF8.GetString($Bytes),[Text.Encoding]::Unicode.GetString($Bytes))
    if ($Bytes.Length -gt 1) { $views += [Text.Encoding]::Unicode.GetString($Bytes,1,$Bytes.Length-1) }
    foreach ($view in $views) {
        $found = $view -match $tokenPattern
        if (-not $found) {
            foreach ($needle in $needles) {
                if ($view.IndexOf($needle,[StringComparison]::OrdinalIgnoreCase) -ge 0) { $found = $true; break }
            }
        }
        if ($found) { $failures.Add($RelativePath); break }
    }
    $isLicenceBundle = $RelativePath.Replace('\','/') -eq 'assets/THIRD-PARTY-LICENCES.txt'
    if (-not $IsExecutable -and -not $isLicenceBundle -and [IO.Path]::GetExtension($RelativePath) -ne '.ico') {
        $text = [Text.Encoding]::UTF8.GetString($Bytes)
        # Required third-party copyright contacts stay in their licence bundle.
        if ($text -match '[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}') { $failures.Add($RelativePath) }
    }
}

# Check both the index (what a commit publishes) and local publishable files.
Push-Location $projectRoot
try {
    $cached = [Text.Encoding]::UTF8.GetString((Read-GitBytes @('ls-files','--cached','-z'))).Split([char]0,[StringSplitOptions]::RemoveEmptyEntries)
    $untracked = [Text.Encoding]::UTF8.GetString((Read-GitBytes @('ls-files','--others','--exclude-standard','-z'))).Split([char]0,[StringSplitOptions]::RemoveEmptyEntries)
    $relativeFiles = @(($cached + $untracked) | Sort-Object -Unique -CaseSensitive)
    $needles = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    Add-PrivateNeedle $env:USERPROFILE
    Add-PrivateNeedle $projectRoot
    # Local data supplies private identifiers to look for; values are never printed.
    foreach ($directoryName in @('WorkMonitorSwitcher','MonitorSwitcherRust')) {
        $dataPath = Join-Path $env:APPDATA $directoryName
        $settingsPath = Join-Path $dataPath 'settings.json'
        if (Test-Path -LiteralPath $settingsPath) {
            $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
            foreach ($profile in $settings.profiles) {
                foreach ($identity in $profile.targets) {
                    if ($identity.Length -ge 8) {
                        Add-PrivateNeedle $identity
                        $components = $identity.Split('#')
                        if ($components.Length -ge 4) { Add-PrivateNeedle $components[2] }
                    }
                }
            }
        }
        $aliasesPath = Join-Path $dataPath 'monitor-aliases.json'
        if (Test-Path -LiteralPath $aliasesPath) {
            $aliases = Get-Content -LiteralPath $aliasesPath -Raw | ConvertFrom-Json
            foreach ($entry in $aliases.PSObject.Properties.Value) {
                if ($entry.LastSerialNumber -and $entry.LastSerialNumber.Length -ge 8) { Add-PrivateNeedle $entry.LastSerialNumber }
            }
        }
    }
    $failures = [Collections.Generic.List[string]]::new()
    $tokenPattern = '(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,}|AKIA[0-9A-Z]{16}|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)'
    foreach ($relative in $cached) {
        Test-PrivateBytes (Read-GitBytes @('show','--no-textconv',":$relative")) $relative
    }
    foreach ($relative in $relativeFiles) {
        $path = Join-Path $projectRoot $relative
        # A staged file may have been removed locally; its index blob was checked.
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            Test-PrivateBytes ([IO.File]::ReadAllBytes($path)) $relative
        }
    }
    if (Test-Path -LiteralPath $Executable -PathType Leaf) {
        Test-PrivateBytes ([IO.File]::ReadAllBytes($Executable)) ([IO.Path]::GetRelativePath($projectRoot,$Executable)) $true
    }
    if ($failures.Count -gt 0) { throw ('Privacy audit failed for: ' + (($failures | Sort-Object -Unique) -join ', ') + '. Private values were not printed.') }
    foreach ($sample in @('settings.json','monitor-layout.cfg','monitor-inspection.json','.build/check.json','.env','backups/old.exe','dist/MonitorSwitcher.exe')) {
        git check-ignore -q -- $sample
        if ($LASTEXITCODE -ne 0) { throw "A private/output path is not ignored: $sample" }
    }
    [pscustomobject]@{ PublishableFiles=$relativeFiles.Count; ExecutableChecked=(Test-Path -LiteralPath $Executable); Result='PASS'; PersonalValuesPrinted=$false }
} finally { Pop-Location }
