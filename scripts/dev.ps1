# Local development helper for Airobot.
#
#   ./scripts/dev.ps1 start              # postgres + frontend build + the web app (SMTP off)
#   ./scripts/dev.ps1 login [email]      # opens the browser signed in
#   ./scripts/dev.ps1 status             # is it up?
#   ./scripts/dev.ps1 stop               # stop the app (postgres keeps running)
#
# `login` skips the email dance: it requests a sign-in link and reads it back
# from the dev server log (SMTP is off in dev, so the link is logged instead
# of sent), then opens it in your default browser.

param(
    [Parameter(Position = 0)][string]$Command = 'start',
    [Parameter(Position = 1)][string]$Email = 'founder@example.com'
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

# Load .env into the process environment if available
if (Test-Path '.env') {
    Get-Content '.env' | ForEach-Object {
        $line = $_.Trim()
        if ($line -and -not $line.StartsWith('#') -and $line.Contains('=')) {
            $parts = $line.Split('=', 2)
            $k = $parts[0].Trim()
            $v = $parts[1].Trim().Trim('"', "'")
            if (-not [string]::IsNullOrEmpty($k) -and [string]::IsNullOrEmpty([System.Environment]::GetEnvironmentVariable($k))) {
                [System.Environment]::SetEnvironmentVariable($k, $v)
            }
        }
    }
}

$Port = if ($env:PORT) { $env:PORT } else { '8081' }
$BaseUrl = if ($env:APP_BASE_URL) { $env:APP_BASE_URL.TrimEnd('/') } else { "http://127.0.0.1:$Port" }
$LogDir = Join-Path (Get-Location) '.dev'
$Log = Join-Path $LogDir 'airobot-dev.log'

function Start-Dev {
    Write-Output "1. Ensuring PostgreSQL is running..."
    docker compose up postgres -d

    Write-Output "2. Checking web frontend assets..."
    if (-not (Test-Path "web/node_modules")) {
        Write-Output "Installing web dependencies..."
        npm --prefix web install
    }
    Write-Output "Building web SPA (Vite + React)..."
    npm --prefix web run build

    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    if (-not (Test-Path $Log)) { New-Item -ItemType File -Path $Log | Out-Null }

    # Stop any previous airobot instance to avoid port collisions
    Get-Process -Name airobot -ErrorAction SilentlyContinue | Stop-Process -Force

    Write-Output "3. Starting Airobot server process..."
    Start-Process pwsh -ArgumentList @('-NoExit', '-File', $PSCommandPath, 'serve') -WorkingDirectory (Get-Location)
    Write-Output "Starting $BaseUrl (log: $Log) ..."
    foreach ($i in 1..60) {
        try {
            $r = Invoke-WebRequest -Uri "$BaseUrl/health" -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -eq 200) {
                Write-Output "Airobot is up: $BaseUrl"
                Write-Output "Sign in with:  ./scripts/dev.ps1 login $Email"
                return
            }
        } catch { }
        Start-Sleep 2
    }
    Write-Output "Server did not come up in time; check $Log"
    if (Test-Path $Log) {
        Write-Output "Recent log output:"
        Get-Content $Log -Tail 25 | Write-Output
    }
    exit 1
}

function Invoke-Serve {
    # Hidden verb: runs the app in this window with SMTP disabled so magic
    # links land in the log (and in `dev.ps1 login`) instead of an inbox.
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $env:SMTP_HOST = ''
    # Local-only: enables the SPA's one-click dev sign-in (@airobot.local /
    # @example.com addresses only; the server refuses it in production).
    $env:AIR_DEV_LOGIN = '1'
    cargo run --bin airobot 2>&1 | ForEach-Object {
        $line = "$_" -replace "$([char]27)\[[0-9;]*m", ''
        [IO.File]::AppendAllText($Log, "$line`n")
        Write-Output $line
    }
}

function Invoke-Login {
    try {
        Invoke-WebRequest -Uri "$BaseUrl/health" -UseBasicParsing -TimeoutSec 3 | Out-Null
    } catch {
        Write-Output "Airobot is not running. Start it first: ./scripts/dev.ps1 start"
        exit 1
    }

    Write-Output "Requesting magic sign-in link for $Email..."
    Invoke-WebRequest -Uri "$BaseUrl/signin" -Method Post `
        -Body @{ email = $Email; idempotency_key = [guid]::NewGuid().ToString() } `
        -MaximumRedirection 0 -UseBasicParsing -ErrorAction SilentlyContinue | Out-Null

    $link = $null
    for ($attempt = 0; $attempt -lt 15; $attempt++) {
        Start-Sleep -Milliseconds 500
        if (Test-Path $Log) {
            $raw = (Get-Content $Log -Raw) -replace "$([char]27)\[[0-9;]*m", ''
            $matched = $raw -split "`r?`n" |
                Where-Object { $_ -match "email=[`"]?$([regex]::Escape($Email))[`"]?" -and $_ -match 'link=(\S+)' } |
                Select-Object -Last 1
            if ($matched) {
                $m = [regex]::Match($matched, 'link=([^\s"]+)')
                if ($m.Success) {
                    $link = $m.Groups[1].Value
                    break
                }
            }
        }
    }

    if (-not $link) {
        Write-Output "No sign-in link for $Email found in $Log yet; wait a second and retry."
        exit 1
    }
    Write-Output "Opening browser signed in as ${Email}: $link"
    try {
        Start-Process $link
    } catch {
        Write-Warning "Could not automatically launch browser: $_"
    }
}

function Show-Status {
    docker compose ps
    try {
        $r = Invoke-WebRequest -Uri "$BaseUrl/health" -UseBasicParsing -TimeoutSec 2
        Write-Output "Health: $($r.StatusCode) $($r.Content)"
    } catch {
        Write-Output "Health: Airobot is not responding on $BaseUrl"
    }
}

function Stop-Dev {
    Get-Process -Name airobot -ErrorAction SilentlyContinue | Stop-Process -Force
    Write-Output "Airobot app stopped (PostgreSQL left running; 'docker compose stop' to stop DB)."
}

function Invoke-Desktop {
    try {
        Invoke-WebRequest -Uri "$BaseUrl/health" -UseBasicParsing -TimeoutSec 3 | Out-Null
    } catch {
        Write-Output "Airobot server is not running on $BaseUrl. Starting it now..."
        Start-Dev
    }

    Write-Output "Launching Airobot Native Desktop App (Tauri v2 Low-RAM Mode)..."
    $env:APP_BASE_URL = $BaseUrl
    cargo run --manifest-path src-tauri/Cargo.toml
}

switch ($Command) {
    'start' { Start-Dev }
    'serve' { Invoke-Serve }
    'login' { Invoke-Login }
    'desktop' { Invoke-Desktop }
    'status' { Show-Status }
    'stop' { Stop-Dev }
    Default {
        Write-Output "Unknown command: $Command. Usage: ./scripts/dev.ps1 [start|login|desktop|status|stop]"
        exit 1
    }
}
