<#
.SYNOPSIS
    Serves Chronicle RPG MCP Server (Streamable HTTP + OAuth 2.1 + Web GM Studio)
    from your local Windows machine over a permanent HTTPS Tailscale Funnel URL.

.DESCRIPTION
    - Keeps your Chronicle code in Git (e.g., C:\dev\chronicle) while pointing
      -CampaignDir at any local or Google Drive-synced campaign folder.
    - Persists your GM OAuth passphrase & default CampaignDir in a gitignored
      `.chronicle-serve.env` file next to the repo root so you can launch with
      a single command (`.\scripts\serve-tailscale.ps1`).
    - Automatically turns off Tailscale Funnel and checkpoints SQLite WAL on Ctrl+C
      so nothing stays exposed and Google Drive receives a clean single-file DB.

.EXAMPLE
    .\scripts\serve-tailscale.ps1
    .\scripts\serve-tailscale.ps1 -CampaignDir "D:\Tgvt\rpg\my-campaign"
    .\scripts\serve-tailscale.ps1 -Build
    .\scripts\serve-tailscale.ps1 -Check
    .\scripts\serve-tailscale.ps1 -Stop
#>
[CmdletBinding()]
param(
    [string]$CampaignDir = "",
    [int]$Port = 7878,
    [string]$OAuthSecret = "",
    [string]$BearerToken = "",
    [switch]$Build,
    [switch]$RotateSecret,
    [switch]$AllowLocalFileImport,
    [switch]$TailscaleOnly,
    [switch]$KeepFunnelOnExit,
    [switch]$Check,
    [switch]$Stop
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$EnvFile = Join-Path $RepoRoot ".chronicle-serve.env"

function Read-EnvFile([string]$Path) {
    $map = @{}
    if (Test-Path $Path) {
        foreach ($line in Get-Content -Path $Path -Encoding UTF8) {
            $trimmed = $line.Trim()
            if (-not $trimmed -or $trimmed.StartsWith("#")) { continue }
            $idx = $trimmed.IndexOf("=")
            if ($idx -gt 0) {
                $k = $trimmed.Substring(0, $idx).Trim()
                $v = $trimmed.Substring($idx + 1).Trim().Trim('"').Trim("'")
                $map[$k] = $v
            }
        }
    }
    return $map
}

function Save-EnvFile([string]$Path, [hashtable]$Values) {
    $lines = @(
        "# Local Chronicle Tailscale Serve configuration (gitignored - keep private!)",
        "CHRONICLE_CAMPAIGN_DIR=$($Values['CHRONICLE_CAMPAIGN_DIR'])",
        "CHRONICLE_OAUTH_SECRET=$($Values['CHRONICLE_OAUTH_SECRET'])",
        "CHRONICLE_PORT=$($Values['CHRONICLE_PORT'])"
    )
    if ($Values.ContainsKey('CHRONICLE_MCP_TOKEN') -and $Values['CHRONICLE_MCP_TOKEN']) {
        $lines += "CHRONICLE_MCP_TOKEN=$($Values['CHRONICLE_MCP_TOKEN'])"
    }
    Set-Content -Path $Path -Value $lines -Encoding UTF8
}

function New-RandomSecret([string]$Prefix = "gm-") {
    $bytes = New-Object byte[] 18
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
    $b64 = [Convert]::ToBase64String($bytes).TrimEnd('=').Replace('+', '-').Replace('/', '_')
    return "$Prefix$b64"
}

function Find-TailscaleExe {
    $cmd = Get-Command tailscale -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        "C:\Program Files\Tailscale\tailscale.exe",
        "C:\Program Files (x86)\Tailscale\tailscale.exe",
        (Join-Path $env:LOCALAPPDATA "Tailscale\tailscale.exe")
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    return $null
}

function Invoke-SqliteCheckpoint([string]$DbPath) {
    if (-not (Test-Path $DbPath)) { return }
    $walPath = "$DbPath-wal"
    if (-not (Test-Path $walPath)) { return }
    $py = Get-Command python -ErrorAction SilentlyContinue
    if (-not $py) { return }
    try {
        $code = "import sqlite3, sys; c = sqlite3.connect(sys.argv[1], timeout=10.0); c.execute('PRAGMA wal_checkpoint(TRUNCATE);'); c.close()"
        & $py.Source -c $code $DbPath 2>$null
        Write-Host "[chronicle-serve] SQLite WAL checkpointed cleanly into campaign.db (Google Drive safe)." -ForegroundColor DarkGray
    } catch {
        Write-Host "[chronicle-serve] Note: WAL checkpoint skipped ($($_.Exception.Message))" -ForegroundColor DarkYellow
    }
}

# 1. Locate Tailscale
$TailscaleExe = Find-TailscaleExe
if (-not $TailscaleExe) {
    Write-Host ""
    Write-Host "Tailscale is not installed on this machine yet." -ForegroundColor Yellow
    Write-Host "Install it in one command, sign in, then re-run this script:" -ForegroundColor White
    Write-Host "  winget install Tailscale.Tailscale" -ForegroundColor Cyan
    Write-Host "  (or download from https://tailscale.com/download/windows)" -ForegroundColor Gray
    Write-Host ""
    exit 1
}

# Quick teardown mode (-Stop)
if ($Stop) {
    Write-Host "Turning off Tailscale Funnel / Serve on HTTPS port 443..." -ForegroundColor Yellow
    & $TailscaleExe funnel --https=443 off 2>$null
    & $TailscaleExe serve --https=443 off 2>$null
    Write-Host "Done. Current Tailscale Funnel status:" -ForegroundColor Green
    & $TailscaleExe funnel status
    exit 0
}

# 2. Load & resolve configuration
$saved = Read-EnvFile $EnvFile

if (-not $CampaignDir) {
    if ($env:CHRONICLE_CAMPAIGN_DIR) {
        $CampaignDir = $env:CHRONICLE_CAMPAIGN_DIR
    } elseif ($saved.ContainsKey("CHRONICLE_CAMPAIGN_DIR") -and $saved["CHRONICLE_CAMPAIGN_DIR"]) {
        $CampaignDir = $saved["CHRONICLE_CAMPAIGN_DIR"]
    } else {
        $CampaignDir = Join-Path $RepoRoot "my-campaign"
    }
}
$CampaignDir = [System.IO.Path]::GetFullPath($CampaignDir)

if (-not $PSBoundParameters.ContainsKey("Port")) {
    if ($env:CHRONICLE_PORT) {
        $Port = [int]$env:CHRONICLE_PORT
    } elseif ($saved.ContainsKey("CHRONICLE_PORT") -and $saved["CHRONICLE_PORT"]) {
        $Port = [int]$saved["CHRONICLE_PORT"]
    }
}

$envDirty = $false
if ($RotateSecret) {
    $OAuthSecret = New-RandomSecret "gm-"
    $envDirty = $true
} elseif (-not $OAuthSecret) {
    if ($env:CHRONICLE_OAUTH_SECRET) {
        $OAuthSecret = $env:CHRONICLE_OAUTH_SECRET
    } elseif ($saved.ContainsKey("CHRONICLE_OAUTH_SECRET") -and $saved["CHRONICLE_OAUTH_SECRET"]) {
        $OAuthSecret = $saved["CHRONICLE_OAUTH_SECRET"]
    } else {
        $OAuthSecret = New-RandomSecret "gm-"
        $envDirty = $true
    }
}

if (-not $BearerToken) {
    if ($env:CHRONICLE_MCP_TOKEN) {
        $BearerToken = $env:CHRONICLE_MCP_TOKEN
    } elseif ($saved.ContainsKey("CHRONICLE_MCP_TOKEN")) {
        $BearerToken = $saved["CHRONICLE_MCP_TOKEN"]
    }
}

if ($envDirty -or -not (Test-Path $EnvFile) -or ($PSBoundParameters.ContainsKey("CampaignDir") -and $saved["CHRONICLE_CAMPAIGN_DIR"] -ne $CampaignDir)) {
    Save-EnvFile $EnvFile @{
        CHRONICLE_CAMPAIGN_DIR = $CampaignDir
        CHRONICLE_OAUTH_SECRET = $OAuthSecret
        CHRONICLE_PORT         = $Port
        CHRONICLE_MCP_TOKEN    = $BearerToken
    }
    Write-Host "[chronicle-serve] Saved local config to $EnvFile (gitignored)." -ForegroundColor DarkGray
}

# 3. Check Tailscale login status & permanent HTTPS DNS name
$tsStatusRaw = & $TailscaleExe status --json 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "Tailscale CLI returned an error:" -ForegroundColor Red
    Write-Host $tsStatusRaw
    exit 1
}
$tsStatus = $tsStatusRaw | ConvertFrom-Json
if ($tsStatus.BackendState -ne "Running") {
    Write-Host "Tailscale is installed but not connected (BackendState = $($tsStatus.BackendState))." -ForegroundColor Yellow
    Write-Host "Open Tailscale in your system tray (or run 'tailscale up') and sign in, then re-run." -ForegroundColor White
    exit 1
}

$DnsName = ($tsStatus.Self.DNSName).TrimEnd('.')
if (-not $DnsName -or -not $DnsName.EndsWith(".ts.net")) {
    Write-Host "Tailscale MagicDNS is not enabled on your tailnet yet (DNSName='$DnsName')." -ForegroundColor Yellow
    Write-Host "Enable MagicDNS + HTTPS Certificates in https://login.tailscale.com/admin/dns and re-run." -ForegroundColor White
    exit 1
}
$PublicUrl = "https://$DnsName"

# 4. Verify / build Chronicle binary
$ChronicleBin = Join-Path $RepoRoot "target\release\chronicle.exe"
$needBuild = $Build.IsPresent -or (-not (Test-Path $ChronicleBin))

if (-not $needBuild -and (Test-Path (Join-Path $RepoRoot "src"))) {
    $binTime = (Get-Item $ChronicleBin).LastWriteTimeUtc
    $newestSrc = Get-ChildItem -Path (Join-Path $RepoRoot "src") -File |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($newestSrc -and $newestSrc.LastWriteTimeUtc -gt $binTime.AddSeconds(2)) {
        Write-Host "[chronicle-serve] Note: src/$($newestSrc.Name) is newer than target\release\chronicle.exe." -ForegroundColor Yellow
        Write-Host "                  Run with -Build to compile the latest changes (after pausing local stdio MCP if locked)." -ForegroundColor Yellow
    }
}

if ($needBuild) {
    Write-Host "[chronicle-serve] Building Chronicle release binary..." -ForegroundColor Cyan
    Push-Location $RepoRoot
    try {
        & cargo build --release --offline
        if ($LASTEXITCODE -ne 0) {
            Write-Host "Offline build failed; retrying with network crates..." -ForegroundColor Yellow
            & cargo build --release
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build --release failed (if Antigravity's local stdio chronicle-gm MCP is running, toggle it off briefly so chronicle.exe is unlocked)."
            }
        }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $ChronicleBin)) {
    throw "Chronicle binary not found at $ChronicleBin. Run with -Build first."
}

$DbPath = Join-Path $CampaignDir "campaign.db"
$DbExists = Test-Path $DbPath

if ($Check) {
    Write-Host ""
    Write-Host "=== Chronicle Tailscale Preflight Check ===" -ForegroundColor Cyan
    Write-Host "Tailscale Exe : $TailscaleExe"
    Write-Host "Tailscale State: $($tsStatus.BackendState)"
    Write-Host "Permanent URL : $PublicUrl/mcp"
    Write-Host "Chronicle Bin : $ChronicleBin"
    Write-Host "Campaign Dir  : $CampaignDir (campaign.db exists: $DbExists)"
    Write-Host "Local Port    : 127.0.0.1:$Port"
    Write-Host "OAuth Secret  : $OAuthSecret"
    Write-Host ""
    Write-Host "Current Tailscale Funnel status:" -ForegroundColor Gray
    & $TailscaleExe funnel status
    exit 0
}

if (-not $DbExists) {
    Write-Host "[chronicle-serve] WARNING: campaign.db not found in $CampaignDir." -ForegroundColor Yellow
    Write-Host "                  Pass -CampaignDir '<path-to-your-campaign>' or initialize a campaign first." -ForegroundColor Yellow
    exit 1
}

# 5. Configure Tailscale Funnel (public HTTPS -> 127.0.0.1:$Port) or Serve (tailnet-only)
$tsMode = if ($TailscaleOnly) { "serve" } else { "funnel" }
Write-Host "[chronicle-serve] Configuring Tailscale $tsMode -> http://127.0.0.1:$Port ..." -ForegroundColor Cyan
$tsOut = & $TailscaleExe $tsMode --bg $Port 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "Tailscale $tsMode failed. Output:" -ForegroundColor Red
    Write-Host $tsOut
    Write-Host ""
    Write-Host "One-time Tailscale setup checklist:" -ForegroundColor Yellow
    Write-Host "  1. Enable MagicDNS & HTTPS Certificates: https://login.tailscale.com/admin/dns"
    Write-Host "  2. If Tailscale printed a Funnel enablement URL above, open it once in your browser to allow Funnel."
    Write-Host "  3. Re-run: .\scripts\serve-tailscale.ps1"
    exit 1
}

# 6. Build Chronicle argument list (HTTP File Guard stays ON by default!)
$chronArgs = @(
    "mcp",
    "--campaign", $CampaignDir,
    "--transport", "http",
    "--host", "127.0.0.1",
    "--port", "$Port",
    "--oauth",
    "--oauth-secret", $OAuthSecret,
    "--public-url", $PublicUrl
)
if ($BearerToken) {
    $chronArgs += @("--bearer-token", $BearerToken)
}
if ($AllowLocalFileImport) {
    $chronArgs += "--allow-local-file-import"
}

$proc = $null
try {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ChronicleBin
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $false
    $psi.RedirectStandardError = $false
    # Quote arguments safely for Windows command-line parsing
    $quoted = $chronArgs | ForEach-Object {
        if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ }
    }
    $psi.Arguments = ($quoted -join ' ')

    $proc = [System.Diagnostics.Process]::Start($psi)

    # Wait for local /health
    $localReady = $false
    for ($i = 0; $i -lt 30; $i++) {
        if ($proc.HasExited) {
            throw "Chronicle exited unexpectedly with code $($proc.ExitCode)."
        }
        try {
            $r = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/health" -TimeoutSec 2
            if ($r.status -eq "ok") {
                $localReady = $true
                break
            }
        } catch {
            Start-Sleep -Milliseconds 300
        }
    }
    if (-not $localReady) {
        throw "Chronicle did not pass local /health on http://127.0.0.1:$Port/health."
    }

    # Public end-to-end HTTPS + OAuth 2.1 metadata verification
    Write-Host "[chronicle-serve] Verifying public HTTPS endpoint $PublicUrl/health ..." -ForegroundColor Cyan
    $publicOk = $false
    $lastErr = ""
    for ($attempt = 1; $attempt -le 8; $attempt++) {
        try {
            $hr = Invoke-RestMethod -Uri "$PublicUrl/health" -TimeoutSec 5
            $mr = Invoke-RestMethod -Uri "$PublicUrl/.well-known/oauth-protected-resource" -TimeoutSec 5
            if ($hr.status -eq "ok" -and $mr.resource -eq "$PublicUrl/mcp") {
                $publicOk = $true
                break
            }
        } catch {
            $lastErr = $_.Exception.Message
            Start-Sleep -Seconds 2
        }
    }

    Write-Host ""
    Write-Host ("=" * 76) -ForegroundColor DarkCyan
    if ($publicOk) {
        Write-Host "  CHRONICLE TAILSCALE $($tsMode.ToUpper()) VERIFIED END-TO-END! (HTTPS + OAuth 2.1 OK)" -ForegroundColor Green
    } else {
        Write-Host "  LOCAL CHRONICLE IS UP, BUT PUBLIC CHECK FAILED ($lastErr)" -ForegroundColor Yellow
        Write-Host "  (First-time Let's Encrypt cert provisioning on Tailscale can take ~10-20s; retry $PublicUrl/health)" -ForegroundColor Yellow
    }
    Write-Host ("=" * 76) -ForegroundColor DarkCyan
    Write-Host "  Permanent MCP URL   : $PublicUrl/mcp" -ForegroundColor White
    Write-Host "  Web GM Studio GUI   : $PublicUrl/" -ForegroundColor White
    Write-Host "  OAuth Discovery     : $PublicUrl/.well-known/oauth-protected-resource" -ForegroundColor Gray
    Write-Host "  GM OAuth Passphrase : $OAuthSecret" -ForegroundColor Yellow
    Write-Host "  Campaign Folder     : $CampaignDir" -ForegroundColor Gray
    Write-Host "  HTTP File Guard     : $(if ($AllowLocalFileImport) { 'DISABLED (--allow-local-file-import)' } else { 'ENABLED (host source_path reads blocked)' })" -ForegroundColor Gray
    Write-Host ("-" * 76) -ForegroundColor DarkCyan
    Write-Host "  ChatGPT (Settings -> Apps / Connectors -> Developer Mode):" -ForegroundColor Cyan
    Write-Host "    URL : $PublicUrl/mcp"
    Write-Host "    Auth: OAuth -> enter the GM OAuth Passphrase above"
    Write-Host ("=" * 76) -ForegroundColor DarkCyan
    Write-Host "Press Ctrl+C to stop serving (auto-disables Tailscale $tsMode & checkpoints SQLite)." -ForegroundColor DarkGray
    Write-Host ""

    $proc.WaitForExit()
} finally {
    Write-Host ""
    Write-Host "[chronicle-serve] Shutting down cleanly..." -ForegroundColor Yellow
    if ($proc -and -not $proc.HasExited) {
        try {
            $proc.Kill()
            $proc.WaitForExit(3000) | Out-Null
        } catch {}
    }
    if (-not $KeepFunnelOnExit) {
        Write-Host "[chronicle-serve] Disabling Tailscale $tsMode on HTTPS port 443..." -ForegroundColor DarkGray
        & $TailscaleExe $tsMode --https=443 off 2>$null
    }
    Invoke-SqliteCheckpoint $DbPath
    Write-Host "[chronicle-serve] Stopped cleanly." -ForegroundColor Green
}
