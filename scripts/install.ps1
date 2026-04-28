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
function Ok    { param($m) Write-Host "  " -NoNewline; Write-Host "v" -ForegroundColor Green -NoNewline; Write-Host " $m" }
function Warn  { param($m) Write-Host "  " -NoNewline; Write-Host "!" -ForegroundColor Yellow -NoNewline; Write-Host " $m" }
function Fail  { param($m) Write-Host "  " -NoNewline; Write-Host "x" -ForegroundColor Red -NoNewline; Write-Host " $m"; exit 1 }

# ── Pre-flight safety checks ──────────────────────────────────────────────────
# Sessões elevadas e/ou perfis de sistema deixam o Elai inacessível ao usuário
# normal. Falhamos cedo com uma mensagem clara em vez de instalar errado.

# 1. Sem sessão elevada (Administrator).
$currentPrincipal = New-Object Security.Principal.WindowsPrincipal(
    [Security.Principal.WindowsIdentity]::GetCurrent()
)
if ($currentPrincipal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host ""
    Write-Host "  x  Não execute o instalador como Administrador." -ForegroundColor Red
    Write-Host ""
    Write-Host "     PowerShell elevado escreve no perfil do administrador," -ForegroundColor Yellow
    Write-Host "     o que impede:" -ForegroundColor Yellow
    Write-Host "       - executar 'elai' em terminais normais" -ForegroundColor Yellow
    Write-Host "       - importar credenciais do Claude Code (~/.claude/...)" -ForegroundColor Yellow
    Write-Host "       - usar o PATH do seu usuário" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "     Abra um PowerShell *normal* (sem 'Executar como administrador')" -ForegroundColor White
    Write-Host "     e rode novamente." -ForegroundColor White
    Write-Host ""
    exit 1
}

# 2. USERPROFILE deve apontar para uma pasta real do usuário (C:\Users\<nome>),
#    não para systemprofile / Default / Public.
if (-not $env:USERPROFILE -or
    $env:USERPROFILE -like "*\config\systemprofile*" -or
    $env:USERPROFILE -like "*\Default*" -or
    $env:USERPROFILE -like "*\Public*") {
    Write-Host ""
    Write-Host "  x  USERPROFILE inválido: '$env:USERPROFILE'" -ForegroundColor Red
    Write-Host "     Esperado algo como C:\Users\<seu-nome>\." -ForegroundColor Yellow
    Write-Host "     Abra um PowerShell na sua sessão de usuário e rode novamente." -ForegroundColor White
    Write-Host ""
    exit 1
}

# 3. Sair de System32 se o terminal abriu lá — confunde caminhos relativos
#    e dá a falsa impressão de que o Elai precisa ser instalado em local de sistema.
if ($PWD.Path -like "*\System32*" -or $PWD.Path -like "*\system32*") {
    Set-Location $env:USERPROFILE
}

# 4. Encerrar processos elai.exe ativos — Windows mantém lock no executável
#    enquanto rodando, e o download falha silenciosamente ou corrompe o binário.
$runningElai = Get-Process -Name "elai" -ErrorAction SilentlyContinue
if ($runningElai) {
    Write-Host ""
    Warn "Detectado(s) $($runningElai.Count) processo(s) elai.exe em execução."
    Write-Host "     Eles precisam ser encerrados para que o binário seja substituído." -ForegroundColor Yellow
    Write-Host ""
    $kill = Read-Host "  Encerrar agora? [Y/n]"
    if ([string]::IsNullOrWhiteSpace($kill)) { $kill = "y" }
    if ($kill -match '^[Yy]') {
        try {
            $runningElai | Stop-Process -Force -ErrorAction Stop
            Start-Sleep -Milliseconds 500
            Ok "Processos encerrados."
        } catch {
            Fail "Não foi possível encerrar elai.exe: $_"
        }
    } else {
        Fail "Feche o(s) processo(s) elai e rode novamente."
    }
}

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

# ── Detect existing installation ──────────────────────────────────────────────
$CurrentVersion = ""
$IsUpdate = $false

try {
    $verOutput = & elai --version 2>$null
    if ($verOutput -match '(\d+\.\d+\.\d+)') {
        $CurrentVersion = $Matches[1]
        $IsUpdate = $true
        Write-Host "  Instalação existente detectada: v$CurrentVersion" -ForegroundColor White
        Write-Host ""
    }
} catch { }

# ── Step 1: Download binary ───────────────────────────────────────────────────
Write-Host "  Step 1 — Instalando binário" -ForegroundColor White
Write-Host ""

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# Fetch latest version from GitHub API.
$LatestVersion = ""
try {
    $apiResponse = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -UseBasicParsing
    if ($apiResponse.tag_name -match '(\d+\.\d+\.\d+)') {
        $LatestVersion = $Matches[1]
    }
} catch { }

$OutFile = Join-Path $InstallDir $BinName

if ($CurrentVersion -and $LatestVersion -and ($CurrentVersion -eq $LatestVersion)) {
    Ok "Binário já está na versão mais recente (v$CurrentVersion). Nada a fazer."
} else {
    if ($LatestVersion) {
        Say "Baixando elai v$LatestVersion ($Target)..."
    } else {
        Say "Baixando $Target..."
    }

    $Url = "https://github.com/$Repo/releases/latest/download/$Target"
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        Fail "Download failed: $_"
    }
    Ok "Binário instalado → $OutFile"
}

# Add install dir to user PATH (permanent).
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$UserPath;$InstallDir", "User")
    $env:PATH = "$env:PATH;$InstallDir"
    Ok "Added $InstallDir to PATH"
}

$ElaiBin = $OutFile

# ── Step 2: Authentication ────────────────────────────────────────────────────
Write-Host ""
Write-Host "  Step 2 — Authentication" -ForegroundColor White
Write-Host ""

# If this is an update, ask whether to reconfigure auth.
if ($IsUpdate) {
    $updateAuth = Read-Host "  Atualizar autenticação? [y/N]"
    if ([string]::IsNullOrWhiteSpace($updateAuth)) { $updateAuth = "n" }
    if ($updateAuth -notmatch '^[Yy]') {
        Ok "Mantendo autenticação existente."
        Write-Host ""
        Write-Host "  Instalação/Atualização concluída!" -ForegroundColor Green
        Write-Host ""
        Write-Host "  Inicie o Elai com:"
        Write-Host ""
        Write-Host "    elai" -ForegroundColor Cyan
        Write-Host ""
        Write-Host "  Para trocar o método de auth depois:"
        Write-Host ""
        Write-Host "    elai login --claudeai|--console|--sso|--api-key|--token|--use-bedrock|..."
        Write-Host "    elai auth status     # ver método ativo"
        Write-Host "    elai auth list       # ver todos os métodos"
        Write-Host ""
        exit 0
    }
}

# Detect existing Claude Code credentials and show hint.
$credFile = Join-Path $env:USERPROFILE ".claude\.credentials.json"
if (Test-Path $credFile) {
    Write-Host "  ! Credenciais Claude Code detectadas. Para importá-las, após a instalação:" -ForegroundColor Yellow
    Write-Host "       elai login --import-claude-code   (em breve)"
    Write-Host "     Ou escolha [1] para fazer um novo login."
    Write-Host ""
}

# Display auth menu.
Write-Host "  How would you like to authenticate?"
Write-Host ""
Write-Host "    [1] Claude Pro/Max — log in to claude.ai (recommended)"
Write-Host "    [2] Anthropic Console — generate an API key via OAuth"
Write-Host "    [3] SSO (asks for e-mail)"
Write-Host "    [4] Paste an Anthropic API key (sk-ant-...)"
Write-Host "    [5] Paste an ANTHROPIC_AUTH_TOKEN"
Write-Host "    [6] AWS Bedrock / Google Vertex / Azure Foundry"
Write-Host "    [7] OpenAI only (no Anthropic) — keys go to ~/.elai/.env"
Write-Host "    [8] Skip — configure later with ``elai login``"
Write-Host ""
$authChoice = Read-Host "  Choose [1]"
if ([string]::IsNullOrWhiteSpace($authChoice)) { $authChoice = "1" }

switch ($authChoice) {
    "1" {
        # Claude Pro/Max — OAuth claude.ai
        Say "Opening claude.ai login..."
        & $ElaiBin login --claudeai
        Ok "Authentication via claude.ai complete."
    }
    "2" {
        # Anthropic Console — OAuth
        Say "Opening Anthropic Console login..."
        & $ElaiBin login --console
        Ok "Authentication via Anthropic Console complete."
    }
    "3" {
        # SSO
        $ssoEmail = Read-Host "  E-mail SSO"
        if ([string]::IsNullOrWhiteSpace($ssoEmail)) { Fail "E-mail cannot be empty." }
        Say "Starting SSO login for $ssoEmail..."
        & $ElaiBin login --sso --email $ssoEmail
        Ok "SSO authentication complete."
    }
    "4" {
        # Paste Anthropic API key
        $anthropicKey = Read-Secret "Anthropic API key (sk-ant-...): "
        if ([string]::IsNullOrWhiteSpace($anthropicKey)) { Fail "API key cannot be empty." }
        $anthropicKey | & $ElaiBin login --api-key --stdin
        Ok "API key saved."
    }
    "5" {
        # Paste ANTHROPIC_AUTH_TOKEN
        $authToken = Read-Secret "ANTHROPIC_AUTH_TOKEN: "
        if ([string]::IsNullOrWhiteSpace($authToken)) { Fail "Auth token cannot be empty." }
        $authToken | & $ElaiBin login --token --stdin
        Ok "Auth token saved."
    }
    "6" {
        # Third-party: Bedrock / Vertex / Foundry
        Write-Host ""
        Write-Host "    [a] AWS Bedrock"
        Write-Host "    [b] Google Vertex"
        Write-Host "    [c] Azure Foundry"
        Write-Host ""
        $threePChoice = Read-Host "  Choose [a]"
        if ([string]::IsNullOrWhiteSpace($threePChoice)) { $threePChoice = "a" }

        switch ($threePChoice.ToLower()) {
            "a" {
                $threePFlag = "--use-bedrock"
                $threePVar  = "CLAUDE_CODE_USE_BEDROCK"
            }
            "b" {
                $threePFlag = "--use-vertex"
                $threePVar  = "CLAUDE_CODE_USE_VERTEX"
            }
            "c" {
                $threePFlag = "--use-foundry"
                $threePVar  = "CLAUDE_CODE_USE_FOUNDRY"
            }
            default { Fail "Invalid choice: $threePChoice" }
        }

        & $ElaiBin login $threePFlag

        $addEnv = Read-Host "  Adicionar '$threePVar=1' como variável de ambiente do usuário? [y/N]"
        if ($addEnv -match '^[Yy]') {
            [Environment]::SetEnvironmentVariable($threePVar, "1", "User")
            Set-Item -Path "Env:$threePVar" -Value "1"
            Ok "Variável $threePVar=1 adicionada ao ambiente do usuário."
        } else {
            Warn "Variável não adicionada. Adicione manualmente se necessário."
        }
    }
    "7" {
        # OpenAI only
        $openAIKey = Read-Secret "OpenAI API key: "
        if ([string]::IsNullOrWhiteSpace($openAIKey)) { Fail "API key cannot be empty." }

        if (-not (Test-Path $ElaiDir)) {
            New-Item -ItemType Directory -Path $ElaiDir -Force | Out-Null
        }

        $lines = @("# Elai Code — API keys", "OPENAI_API_KEY=$openAIKey")
        $lines | Set-Content -Path $EnvFile -Encoding UTF8

        # Restrict file permissions to current user only.
        $acl = Get-Acl $EnvFile
        $acl.SetAccessRuleProtection($true, $false)
        $rule = New-Object Security.AccessControl.FileSystemAccessRule(
            [Security.Principal.WindowsIdentity]::GetCurrent().Name,
            "FullControl", "Allow"
        )
        $acl.SetAccessRule($rule)
        Set-Acl $EnvFile $acl

        Ok "OpenAI API key salva em $EnvFile"

        [Environment]::SetEnvironmentVariable("OPENAI_API_KEY", $openAIKey, "User")
        $env:OPENAI_API_KEY = $openAIKey
        Ok "OPENAI_API_KEY set in user environment"
    }
    "8" {
        Warn "Pulando autenticação. Use 'elai login' para configurar depois."
    }
    default {
        Fail "Invalid choice: $authChoice"
    }
}

# ── Done ──────────────────────────────────────────────────────────────────────
Write-Host ""
if ($IsUpdate) {
    Write-Host "  Atualização concluída!" -ForegroundColor Green
} else {
    Write-Host "  Instalação concluída!" -ForegroundColor Green
}
Write-Host ""
Write-Host "  Inicie o Elai com:"
Write-Host ""
Write-Host "    elai" -ForegroundColor Cyan
Write-Host ""
Write-Host "  Para trocar o método de auth depois:"
Write-Host ""
Write-Host "    elai login --claudeai|--console|--sso|--api-key|--token|--use-bedrock|..."
Write-Host "    elai auth status     # ver método ativo"
Write-Host "    elai auth list       # ver todos os métodos"
Write-Host ""

if ($authChoice -eq "7") {
    Write-Host "  Reinicie o terminal para que as variáveis de ambiente tenham efeito."
    Write-Host ""
}
