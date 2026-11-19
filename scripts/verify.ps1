$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Invoke-Check([string]$Program, [string[]]$Arguments) {
    Write-Output "Running: $Program $($Arguments -join ' ')..."
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Verification failed: $Program $Arguments" }
}

Write-Output "==> [1/5] Verifying React + Vite SPA Compilation..."
if (-not (Test-Path "web/node_modules")) {
    Invoke-Check npm @('--prefix', 'web', 'install')
}
Invoke-Check npm @('--prefix', 'web', 'run', 'build')

Write-Output "==> [2/5] Checking Rust Code Formatting..."
Invoke-Check cargo @('fmt', '--check')

Write-Output "==> [3/5] Running Clippy Linting..."
Invoke-Check cargo @('clippy', '--all-targets', '--', '-D', 'warnings')

Write-Output "==> [4/5] Running Rust Test Suite..."
Invoke-Check cargo @('test')

Write-Output "==> [5/5] Building Release Binaries (airobot & airobot-cli)..."
Invoke-Check cargo @('build', '--release', '--bin', 'airobot', '--bin', 'airobot-cli')

Write-Output '==> All Airobot verification checks passed successfully!'
