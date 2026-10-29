# Language: en-GB
# All fixtures use synthetic values in an isolated, ignored Git repository.
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$fixtureRoot = Join-Path $projectRoot ('.build\privacy-tests\' + [guid]::NewGuid().ToString('N'))
$fixtureScripts = Join-Path $fixtureRoot 'scripts'
New-Item -ItemType Directory -Path $fixtureScripts -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'Check-Privacy.ps1') -Destination $fixtureScripts
$audit = Join-Path $fixtureScripts 'Check-Privacy.ps1'
$encoding = [Text.UTF8Encoding]::new($false)
$previousProfile = $env:USERPROFILE
$previousAppData = $env:APPDATA
$checks = 0

function Write-Fixture([string]$RelativePath, [string]$Contents) {
    $path = Join-Path $fixtureRoot $RelativePath
    New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
    [IO.File]::WriteAllText($path,$Contents,$encoding)
}
function Invoke-FixtureGit([string[]]$Arguments) {
    & git -C $fixtureRoot @Arguments
    if ($LASTEXITCODE -ne 0) { throw 'Isolated privacy fixture Git operation failed.' }
}
function Assert-Audit([bool]$ShouldPass, [string]$Name) {
    $failed = $false
    try { & $audit | Out-Null }
    catch {
        if ($_.Exception.Message -notlike 'Privacy audit failed for:*') { throw }
        $failed = $true
    }
    if ($failed -eq $ShouldPass) { throw "Privacy regression failed: $Name" }
    $script:checks++
}

try {
    # Do not inspect or mutate the user's actual application data or identity.
    $env:USERPROFILE = 'C:\Users\PrivacyFixture'
    $env:APPDATA = Join-Path $fixtureRoot 'synthetic-appdata'
    New-Item -ItemType Directory -Path $env:APPDATA -Force | Out-Null
    Write-Fixture '.gitignore' "settings.json`n*.cfg`nmonitor-inspection.json`n.build/`n.env`nbackups/`ndist/`nsynthetic-appdata/`n"
    Invoke-FixtureGit @('init','--quiet')
    Write-Fixture 'notes.txt' 'Safe fixture.'
    Write-Fixture 'Résumé notes.txt' 'Safe non-ASCII filename.'
    Assert-Audit $true 'clean and non-ASCII files pass'

    # Construct a fake credential at runtime so this test source is publishable.
    $syntheticCredential = 'gh' + 'p_' + ('A' * 30)
    Write-Fixture 'notes.txt' $syntheticCredential
    Assert-Audit $false 'untracked credential is rejected'
    Invoke-FixtureGit @('add','--','notes.txt')
    Write-Fixture 'notes.txt' 'Sanitised working tree, still unsafe in the index.'
    Assert-Audit $false 'staged credential with sanitised working tree is rejected'
    Remove-Item -LiteralPath (Join-Path $fixtureRoot 'notes.txt')
    Assert-Audit $false 'staged credential without a working-tree file is rejected'
    Invoke-FixtureGit @('rm','--cached','--quiet','--','notes.txt')

    Write-Fixture 'notes.txt' $env:USERPROFILE
    Assert-Audit $false 'literal private path is rejected'
    Write-Fixture 'notes.txt' (@{path=($env:USERPROFILE + '\Documents\fixture.txt')} | ConvertTo-Json -Compress)
    Assert-Audit $false 'JSON-escaped private path is rejected'
    Write-Fixture 'notes.txt' ('let path = "' + $env:USERPROFILE.Replace('\','\\') + '\\Documents\\fixture.txt";')
    Assert-Audit $false 'Rust-escaped private path is rejected'

    Write-Fixture 'notes.txt' 'Safe fixture.'
    Write-Fixture '.build/scratch.txt' $syntheticCredential
    Write-Fixture '.env' $syntheticCredential
    Assert-Audit $true 'ignored scratch outputs remain ignored'

    $syntheticContact = 'holder' + [char]64 + 'example.invalid'
    Write-Fixture 'assets/THIRD-PARTY-LICENCES.txt' ("Required third-party copyright contact: $syntheticContact")
    Assert-Audit $true 'third-party licence contacts remain allowed'
    Invoke-FixtureGit @('add','--','assets/THIRD-PARTY-LICENCES.txt','Résumé notes.txt')
    Assert-Audit $true 'staged licence contacts and non-ASCII filenames pass'

    [pscustomobject]@{ RegressionChecks=$checks; Result='PASS'; UsesSyntheticFixtures=$true }
} finally {
    $env:USERPROFILE = $previousProfile
    $env:APPDATA = $previousAppData
}
