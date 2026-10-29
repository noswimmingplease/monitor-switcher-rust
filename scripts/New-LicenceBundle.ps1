param([string]$Cargo = 'cargo')
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $projectRoot
try {
    $metadata = (& $Cargo metadata --locked --format-version 1 | ConvertFrom-Json)
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $text = [Text.StringBuilder]::new()
    [void]$text.AppendLine('Third-party licences. Language of this index: en-GB. Original licence texts follow unchanged.')
    foreach ($package in ($metadata.packages | Where-Object source | Sort-Object name,version)) {
        [void]$text.AppendLine("`n========== $($package.name) $($package.version) ($($package.license)) ==========")
        $dir = Split-Path -Parent $package.manifest_path
        $licences = @(Get-ChildItem -LiteralPath $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE)' })
        # The embedded fonts carry their own licences, not just the crate's code licence.
        if ($package.name -eq 'epaint_default_fonts') {
            $fontDirectory = Join-Path $dir 'fonts'
            $licences += @(Get-ChildItem -LiteralPath $fontDirectory -File -Filter '*.txt')
        }
        if ($package.license_file) { $licences += Get-Item -LiteralPath (Join-Path $dir $package.license_file) }
        if ($licences.Count -eq 0) {
            [void]$text.AppendLine("Declared licence: $($package.license). Source: https://crates.io/crates/$($package.name)/$($package.version)")
        }
        foreach ($file in ($licences | Sort-Object FullName -Unique)) {
            [void]$text.AppendLine("--- $($file.Name) ---")
            [void]$text.AppendLine([IO.File]::ReadAllText($file.FullName))
        }
    }
    [IO.File]::WriteAllText((Join-Path $projectRoot 'assets\THIRD-PARTY-LICENCES.txt'),$text.ToString(),[Text.UTF8Encoding]::new($false))
} finally { Pop-Location }
