#Requires -Version 5.1
$ErrorActionPreference = 'Stop'

$Repo       = "nextlw/elai-code"
$Target     = "elai-windows-x86_64.exe"
$BinName    = "elai.exe"
$InstallDir = if ($env:ELAI_INSTALL_DIR) { $env:ELAI_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".elai\bin" }
$ElaiDir    = Join-Path $env:USERPROFILE ".elai"
$EnvFile    = Join-Path $ElaiDir ".env"

# ── Helpers ───────────────────────────────────────────────────────────────────
function Say   { param($m) Write-Host "  " -NoNewline; Write-Host ">" -ForegroundColor Cyan -NoNewline; Write-Host " $m" }
function Ok    { param($m) Write-Host "  " -NoNewline; Write-Host "✓" -ForegroundColor Green -NoNewline; Write-Host " $m" }
function Warn  { param($m) Write-Host "  " -NoNewline; Write-Host "!" -ForegroundColor Yellow -NoNewline; Write-Host " $m" }
function Fail  { param($m) Write-Host "  " -NoNewline; Write-Host "✗" -ForegroundColor Red -NoNewline; Write-Host " $m"; exit 1 }

function Read-Secret {
    param([string]$Prompt)
    Write-Host "  $Prompt" -NoNewline
    $ss = Read-Host -AsSecureString
    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($ss)
    try { return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }
}

# ── Banner ────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "  ██████████████████   ███████╗██╗      █████╗ ██╗" -ForegroundColor Cyan
Write-Host "  ████████  ▄▄  ▄▄     ██╔════╝██║     ██╔══██╗██║" -ForegroundColor Cyan
Write-Host "  ████████  ██  ██     █████╗  ██║     ███████║██║" -ForegroundColor Cyan
Write-Host "  ████████  ▀▀  ▀▀     ██╔══╝  ██║     ██╔══██║██║" -ForegroundColor Cyan
Write-Host "  ██████████████████   ███████╗███████╗██║  ██║██║" -ForegroundColor Cyan
Write-Host "        ████  ████     ╚══════╝╚══════╝╚═╝  ╚═╝╚═╝" -ForegroundColor Cyan
Write-Host ""
Write-Host "  Elai Code Installer" -ForegroundColor White
Write-Host ""

# ── Step 1: Provider selection ────────────────────────────────────────────────
Write-Host "  Step 1 — Choose your AI provider" -ForegroundColor White
Write-Host ""
Write-Host "    [1] Anthropic  (Claude opus / sonnet / haiku)"
Write-Host "    [2] OpenAI     (gpt-4o, gpt-4o-mini, o3...)"
Write-Host "    [3] Both"
Write-Host ""
$choice = Read-Host "  Choice [1]"
if ([string]::IsNullOrWhiteSpace($choice)) { $choice = "1" }

$AnthropicKey = ""
$OpenAIKey    = ""

switch ($choice) {
    "1" {
        $AnthropicKey = Read-Secret "Anthropic API key: "
        if ([string]::IsNullOrWhiteSpace($AnthropicKey)) { Fail "API key cannot be empty." }
    }
    "2" {
        $OpenAIKey = Read-Secret "OpenAI API key: "
        if ([string]::IsNullOrWhiteSpace($OpenAIKey)) { Fail "API key cannot be empty." }
    }
    "3" {
        $AnthropicKey = Read-Secret "Anthropic API key: "
        if ([string]::IsNullOrWhiteSpace($AnthropicKey)) { Fail "Anthropic API key cannot be empty." }
        $OpenAIKey = Read-Secret "OpenAI API key: "
        if ([string]::IsNullOrWhiteSpace($OpenAIKey)) { Fail "OpenAI API key cannot be empty." }
    }
    default { Fail "Invalid choice: $choice" }
}

# ── Step 2: Download binary ───────────────────────────────────────────────────
Write-Host ""
Write-Host "  Step 2 — Installing elai binary" -ForegroundColor White
Write-Host ""

Say "Downloading $Target..."

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$Url     = "https://github.com/$Repo/releases/latest/download/$Target"
$OutFile = Join-Path $InstallDir $BinName

try {
    Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
} catch {
    Fail "Download failed: $_"
}

Ok "Binary installed → $OutFile"

# Add install dir to user PATH (permanent)
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$UserPath;$InstallDir", "User")
    $env:PATH = "$env:PATH;$InstallDir"
    Ok "Added $InstallDir to PATH"
}

# ── Step 3: Save API keys ─────────────────────────────────────────────────────
Write-Host ""
Write-Host "  Step 3 — Saving API keys" -ForegroundColor White
Write-Host ""

if (-not (Test-Path $ElaiDir)) {
    New-Item -ItemType Directory -Path $ElaiDir -Force | Out-Null
}

# Write ~/.elai/.env  (read by elai on every run)
$lines = @("# Elai Code — API keys")
if ($AnthropicKey) { $lines += "ANTHROPIC_API_KEY=$AnthropicKey" }
if ($OpenAIKey)    { $lines += "OPENAI_API_KEY=$OpenAIKey" }
$lines | Set-Content -Path $EnvFile -Encoding UTF8

# Restrict file permissions to current user only
$acl = Get-Acl $EnvFile
$acl.SetAccessRuleProtection($true, $false)
$rule = New-Object Security.AccessControl.FileSystemAccessRule(
    [Security.Principal.WindowsIdentity]::GetCurrent().Name,
    "FullControl", "Allow"
)
$acl.SetAccessRule($rule)
Set-Acl $EnvFile $acl

Ok "Keys saved to $EnvFile"

# Also set as permanent user environment variables
if ($AnthropicKey) {
    [Environment]::SetEnvironmentVariable("ANTHROPIC_API_KEY", $AnthropicKey, "User")
    $env:ANTHROPIC_API_KEY = $AnthropicKey
    Ok "ANTHROPIC_API_KEY set in user environment"
}
if ($OpenAIKey) {
    [Environment]::SetEnvironmentVariable("OPENAI_API_KEY", $OpenAIKey, "User")
    $env:OPENAI_API_KEY = $OpenAIKey
    Ok "OPENAI_API_KEY set in user environment"
}

# ── Done ──────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "  Installation complete!" -ForegroundColor Green
Write-Host ""
Write-Host "  Restart your terminal, then run:"
Write-Host ""
Write-Host "    elai" -ForegroundColor Cyan
Write-Host ""
