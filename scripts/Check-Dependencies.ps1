# Language: en-GB
param([string]$Lockfile, [switch]$SelfTest)
$ErrorActionPreference = 'Stop'

# These exact package versions have already been reviewed. This is not a blanket
# exemption for new advisories, security findings or future dependency versions.
$acceptedMaintenance = @{
    'RUSTSEC-2026-0192' = @('ttf-parser', '0.25.1')
    'RUSTSEC-2026-0370' = @('zvariant', '5.15.1')
    'RUSTSEC-2026-0371' = @('zbus_names', '4.3.5')
    'RUSTSEC-2026-0372' = @('zvariant_derive', '5.15.1')
}

function Get-LockedDependencies([string]$Contents) {
    $sections = $Contents -split '(?m)^[ \t]*\[\[package\]\][ \t]*(?:#.*)?\r?$'
    for ($i = 1; $i -lt $sections.Count; $i++) {
        $section = $sections[$i]
        $name = [regex]::Match($section, '(?m)^[ \t]*name[ \t]*=[ \t]*"([A-Za-z0-9_-]+)"[ \t]*(?:#.*)?\r?$')
        $version = [regex]::Match($section, '(?m)^[ \t]*version[ \t]*=[ \t]*"([0-9A-Za-z.+-]+)"[ \t]*(?:#.*)?\r?$')
        $source = [regex]::Match($section, '(?m)^[ \t]*source[ \t]*=[ \t]*"([^"]+)"[ \t]*(?:#.*)?\r?$')
        if (-not $name.Success -or -not $version.Success) {
            throw 'Unable to parse a locked dependency; no advisory check was skipped.'
        }
        if (-not $source.Success) {
            $hasSource = [regex]::IsMatch($section, '(?m)^[ \t]*source[ \t]*=')
            if ($hasSource -or $name.Groups[1].Value -ne 'monitor-switcher-rust') {
                throw 'A locked package has an unrecognised or missing source; review it explicitly.'
            }
            continue # Only this application's own package may omit a registry source.
        }
        if ($source.Groups[1].Value -ne 'registry+https://github.com/rust-lang/crates.io-index') {
            throw 'A dependency uses an unrecognised source; review its advisories separately.'
        }
        @{ package = @{ name = $name.Groups[1].Value; ecosystem = 'crates.io' }; version = $version.Groups[1].Value }
    }
}

function Get-AdvisoryDisposition($Advisory, [string]$Package, [string]$Version) {
    if ($Advisory.withdrawn) { return 'withdrawn' }
    $accepted = $acceptedMaintenance[$Advisory.id]
    if (-not $accepted -or $accepted[0] -ne $Package -or $accepted[1] -ne $Version) {
        return 'blocked'
    }
    $affected = @($Advisory.affected | Where-Object {
        $_.package.ecosystem -eq 'crates.io' -and $_.package.name -eq $Package
    })
    if ($affected.Count -eq 0 -or $Advisory.severity) { return 'blocked' }
    foreach ($entry in $affected) {
        if ($entry.database_specific.informational -ne 'unmaintained' -or
            $entry.database_specific.cvss -or $entry.database_specific.categories -or $entry.severity) {
            return 'blocked'
        }
    }
    'accepted maintenance notice'
}

if ($SelfTest) {
    $fixture = @'
version = 4
[[package]]
name = "monitor-switcher-rust"
version = "0.1.0"
[[package]]
name = "ttf-parser"
version = "0.25.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
'@
    $dependencies = @(Get-LockedDependencies $fixture)
    if ($dependencies.Count -ne 1 -or $dependencies[0].package.name -ne 'ttf-parser') { throw 'Lockfile parser regression.' }
    $notice = '{"id":"RUSTSEC-2026-0192","affected":[{"package":{"ecosystem":"crates.io","name":"ttf-parser"},"database_specific":{"informational":"unmaintained","categories":[],"cvss":null}}]}' | ConvertFrom-Json
    if ((Get-AdvisoryDisposition $notice 'ttf-parser' '0.25.1') -ne 'accepted maintenance notice') { throw 'Reviewed maintenance notice was not recognised.' }
    if ((Get-AdvisoryDisposition $notice 'ttf-parser' '0.25.2') -ne 'blocked') { throw 'Unreviewed dependency version was accepted.' }
    $notice.id = 'RUSTSEC-2099-0001'
    if ((Get-AdvisoryDisposition $notice 'ttf-parser' '0.25.1') -ne 'blocked') { throw 'A new advisory was accepted.' }
    $notice.id = 'RUSTSEC-2026-0192'
    $notice.affected[0].database_specific.informational = $null
    if ((Get-AdvisoryDisposition $notice 'ttf-parser' '0.25.1') -ne 'blocked') { throw 'A security finding was accepted.' }
    $rejected = $false
    try { Get-LockedDependencies ($fixture.Replace('registry+https://github.com/rust-lang/crates.io-index', 'git+https://example.invalid/source')) | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'An unaudited dependency source was accepted.' }
    $compact = @(Get-LockedDependencies ($fixture.Replace('source = ', 'source=')))
    if ($compact.Count -ne 1 -or $compact[0].package.name -ne 'ttf-parser') { throw 'Compact TOML silently omitted a dependency.' }
    $notice.affected[0].database_specific.informational = 'unmaintained'
    $notice.affected[0] | Add-Member -NotePropertyName severity -NotePropertyValue @(@{type='CVSS_V3';score='CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H'})
    if ((Get-AdvisoryDisposition $notice 'ttf-parser' '0.25.1') -ne 'blocked') { throw 'A package-level security severity was accepted.' }
    $rejected = $false
    try { Get-LockedDependencies ($fixture.Replace('monitor-switcher-rust', 'unknown-local-package')) | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw 'An unknown source-less package was skipped.' }
    [pscustomobject]@{ DependencyRegressionChecks = 9; Result = 'PASS' }
    return
}

if (-not $Lockfile) { $Lockfile = Join-Path $PSScriptRoot '..\Cargo.lock' }
$queries = @(Get-LockedDependencies ([IO.File]::ReadAllText($Lockfile)))
if ($queries.Count -eq 0) { throw 'No locked dependencies were found; advisory checking did not run.' }
$payload = @{ queries = $queries } | ConvertTo-Json -Depth 6 -Compress
try {
    # Only public package names and versions are sent. No application/user data.
    $batch = Invoke-RestMethod -Method Post -Uri 'https://api.osv.dev/v1/querybatch' -ContentType 'application/json' -Body $payload -TimeoutSec 30
} catch { throw 'Unable to query OSV; dependency advisory verification is incomplete.' }
if (@($batch.results).Count -ne $queries.Count) { throw 'The advisory response was incomplete.' }
$blocked = [Collections.Generic.List[string]]::new()
$notices = [Collections.Generic.HashSet[string]]::new()
$details = @{}
for ($i = 0; $i -lt $queries.Count; $i++) {
    $result = $batch.results[$i]
    if ($null -eq $result -or $result -isnot [pscustomobject] -or $result.next_page_token) {
        throw 'The advisory response was incomplete or paginated; dependency verification is incomplete.'
    }
    foreach ($finding in $result.vulns) {
        if (-not $finding.id) { throw 'An advisory had no identifier; verification is incomplete.' }
        if (-not $details.ContainsKey($finding.id)) {
            try {
                $details[$finding.id] = Invoke-RestMethod -Uri ('https://api.osv.dev/v1/vulns/' + [Uri]::EscapeDataString($finding.id)) -TimeoutSec 30
            } catch { throw 'Unable to retrieve an advisory; dependency verification is incomplete.' }
        }
        $advisory = $details[$finding.id]
        if ($advisory.id -ne $finding.id) { throw 'Advisory identifiers did not match.' }
        $package = $queries[$i].package.name
        $version = $queries[$i].version
        $disposition = Get-AdvisoryDisposition $advisory $package $version
        $label = "$($finding.id): $package $version"
        if ($disposition -eq 'blocked') { $blocked.Add($label) }
        elseif ($disposition -eq 'accepted maintenance notice') { [void]$notices.Add($label) }
    }
}
foreach ($notice in ($notices | Sort-Object)) { Write-Warning "Reviewed maintenance notice: $notice" }
if ($blocked.Count -gt 0) { throw ('Unreviewed dependency advisories: ' + (($blocked | Sort-Object -Unique) -join '; ')) }
[pscustomobject]@{ LockedDependencies = $queries.Count; AcceptedMaintenanceNotices = $notices.Count; UnreviewedAdvisories = 0; Result = 'PASS' }
