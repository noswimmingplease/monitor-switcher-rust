param([string]$ToolchainBin, [switch]$TestOnly)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
if (-not $ToolchainBin) {
    $command = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $command) { throw 'Install Rust stable with the MSVC target, or provide -ToolchainBin.' }
    $ToolchainBin = Split-Path -Parent $command.Source
}
$previous = @{}
foreach ($name in @('PATH','RUSTC','CARGO_ENCODED_RUSTFLAGS')) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
Push-Location $projectRoot
try {
    $env:PATH = $ToolchainBin + ';' + $env:PATH
    $env:RUSTC = Join-Path $ToolchainBin 'rustc.exe'
    $cargoCache = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
    $env:CARGO_ENCODED_RUSTFLAGS = @('-D','warnings','-C','target-feature=+crt-static',('--remap-path-prefix=' + $projectRoot + '=monitor-switcher'),('--remap-path-prefix=' + $cargoCache + '=registry'),('--remap-path-prefix=' + [IO.Path]::GetFullPath((Join-Path $ToolchainBin '..')) + '=toolchain')) -join [char]31
    & (Join-Path $ToolchainBin 'cargo.exe') fmt --all -- --check
    if ($LASTEXITCODE -ne 0) { throw 'Formatting checks failed.' }
    & (Join-Path $ToolchainBin 'cargo.exe') test --locked
    if ($LASTEXITCODE -ne 0) { throw 'Regression tests failed.' }
    if (-not $TestOnly) {
        & (Join-Path $ToolchainBin 'cargo.exe') build --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
        $dist = Join-Path $projectRoot 'dist'
        New-Item -ItemType Directory -Path $dist -Force | Out-Null
        $exe = Join-Path $dist 'MonitorSwitcher.exe'
        Copy-Item -LiteralPath (Join-Path $projectRoot 'target\x86_64-pc-windows-msvc\release\MonitorSwitcher.exe') -Destination $exe
        Get-FileHash -LiteralPath $exe -Algorithm SHA256
    }
} finally {
    Pop-Location
    foreach ($name in $previous.Keys) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
}
