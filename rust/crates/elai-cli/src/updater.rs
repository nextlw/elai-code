use std::env;
use std::fs;
use std::io::{self, Write};

const LATEST_API: &str =
    "https://api.github.com/repos/nextlw/elai-code/releases/latest";
const INSTALL_URL: &str =
    "https://raw.githubusercontent.com/nextlw/elai-code/main/scripts/install.sh";

struct Release {
    version: String,
    notes: String,
    download_url: String,
}

/// Extracts only the bullet-list changelog from the release body.
/// Looks for a "## Changelog" heading and takes lines until the next "---" separator.
/// Falls back to the full body if the heading is not found.
fn extract_changelog(body: &str) -> &str {
    let heading = "## Changelog";
    if let Some(start) = body.find(heading) {
        let after_heading = &body[start + heading.len()..];
        let content = after_heading.trim_start_matches('\n');
        if let Some(end) = content.find("\n---") {
            return content[..end].trim();
        }
        return content.trim();
    }
    // No structured changelog found — return the body up to the first "---"
    if let Some(end) = body.find("\n---") {
        return body[..end].trim();
    }
    body.trim()
}

/// Returned by [`check_available`] when a newer release exists.
pub struct UpdateAvailable {
    pub current: &'static str,
    pub latest: String,
}


fn asset_name() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Some("elai-macos-arm64"),
        ("macos", "x86_64") => Some("elai-macos-x86_64"),
        ("linux", "x86_64") => Some("elai-linux-x86_64"),
        ("linux", "aarch64") => Some("elai-linux-arm64"),
        ("windows", _) => Some("elai-windows-x86_64.exe"),
        _ => None,
    }
}

fn http_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("elai-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(6))
        .build()
        .ok()
}

fn fetch_latest() -> Option<Release> {
    let json: serde_json::Value = http_client()?.get(LATEST_API).send().ok()?.json().ok()?;

    let tag = json["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    let raw_body = json["body"].as_str().unwrap_or("");
    let notes = extract_changelog(raw_body).trim().to_string();

    let asset = asset_name()?;
    let download_url = json["assets"]
        .as_array()?
        .iter()
        .find(|a| a["name"].as_str() == Some(asset))?["browser_download_url"]
        .as_str()?
        .to_string();

    Some(Release { version, notes, download_url })
}

fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split('.').filter_map(|s| s.parse().ok()).collect()
    };
    parse(latest) > parse(current)
}

fn do_update(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let exe = env::current_exe()?;
    let tmp = exe.with_extension("update.tmp");

    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("elai-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_mins(2))
        .build()?;

    print!("  Baixando novo binário");
    io::stdout().flush()?;
    let bytes = client.get(url).send()?.bytes()?;
    println!(" ({} KB)  ✓", bytes.len() / 1024);

    fs::write(&tmp, &bytes)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }

    #[cfg(unix)]
    fs::rename(&tmp, &exe)?;

    #[cfg(windows)]
    {
        let old = exe.with_extension("old");
        let _ = fs::rename(&exe, &old);
        fs::rename(&tmp, &exe)?;
    }

    Ok(())
}

fn print_notes(notes: &str) {
    if notes.is_empty() {
        return;
    }
    println!("  O que há de novo:\n");
    for line in notes.lines() {
        println!("    {line}");
    }
    println!();
}

fn apply(release: &Release) {
    match do_update(&release.download_url) {
        Ok(()) => {
            println!(
                "\n  ✓ Elai Code v{} instalado com sucesso!",
                release.version
            );
            println!("  Execute `elai` novamente para usar a nova versão.\n");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("\n  ✗ Falha na atualização: {e}");
            eprintln!("  Instale manualmente:");
            eprintln!("    curl -fsSL {INSTALL_URL} | sh\n");
            std::process::exit(1);
        }
    }
}

/// Non-blocking check used by TUI mode. Returns `Some` when a newer release
/// exists so the caller can surface the notification inside the UI.
pub fn check_available() -> Option<UpdateAvailable> {
    if env::var("CI").is_ok() || env::var("ELAI_SKIP_UPDATE").is_ok() {
        return None;
    }
    let current = env!("CARGO_PKG_VERSION");
    let release = fetch_latest()?;
    if !is_newer(&release.version, current) {
        return None;
    }
    Some(UpdateAvailable { current, latest: release.version })
}

/// Chamado no boot. Bloqueia e força update quando há versão mais nova.
pub fn check_and_enforce() {
    if env::var("CI").is_ok() || env::var("ELAI_SKIP_UPDATE").is_ok() {
        return;
    }

    let current = env!("CARGO_PKG_VERSION");

    print!("  Verificando atualizações...");
    io::stdout().flush().ok();

    let Some(release) = fetch_latest() else {
        // limpa a linha e continua silenciosamente se offline
        print!("\r                              \r");
        io::stdout().flush().ok();
        return;
    };

    if !is_newer(&release.version, current) {
        print!("\r                              \r");
        io::stdout().flush().ok();
        return;
    }

    // Há versão nova — bloqueia
    println!("\r");
    let title = format!("  Atualização obrigatória: v{current} → v{}", release.version);
    let bar = "─".repeat(title.chars().count() - 2);
    println!("  ┌{bar}┐");
    println!("{title}");
    println!("  └{bar}┘\n");

    print_notes(&release.notes);

    println!("  Você precisa atualizar antes de continuar.");
    print!("  Pressione Enter para atualizar agora (Ctrl+C para cancelar): ");
    io::stdout().flush().ok();

    let mut buf = String::new();
    io::stdin().read_line(&mut buf).ok();

    println!();
    apply(&release);
}

/// `elai update` — atualização manual.
pub fn run_update() {
    let current = env!("CARGO_PKG_VERSION");
    println!("  Verificando atualizações (versão atual: v{current})...\n");

    let Some(release) = fetch_latest() else {
        eprintln!("  ✗ Sem conexão ou erro ao verificar atualizações.");
        std::process::exit(1);
    };

    if !is_newer(&release.version, current) {
        println!("  ✓ Já está na versão mais recente (v{current}).");
        return;
    }

    println!("  Nova versão disponível: v{}\n", release.version);
    print_notes(&release.notes);
    apply(&release);
}
