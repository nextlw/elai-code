use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{Map, Value};

use crate::oauth::credentials_home_dir;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> io::Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> io::Result<()>;
    fn delete(&self, key: &str) -> io::Result<()>;
}

// ---------------------------------------------------------------------------
// FileStore
// ---------------------------------------------------------------------------

pub struct FileStore {
    base: PathBuf,
}

impl FileStore {
    #[must_use]
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    fn read_map(&self) -> io::Result<Map<String, Value>> {
        match std::fs::read_to_string(&self.base) {
            Ok(contents) => {
                if contents.trim().is_empty() {
                    return Ok(Map::new());
                }
                serde_json::from_str::<Value>(&contents)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                    .as_object()
                    .cloned()
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "secrets file must contain a JSON object",
                        )
                    })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
            Err(e) => Err(e),
        }
    }

    fn write_map(&self, map: &Map<String, Value>) -> io::Result<()> {
        if let Some(parent) = self.base.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let rendered = serde_json::to_string_pretty(&Value::Object(map.clone()))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.base.with_extension("json.tmp");
        std::fs::write(&tmp, format!("{rendered}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &self.base)
    }
}

impl SecretStore for FileStore {
    fn get(&self, key: &str) -> io::Result<Option<String>> {
        let map = self.read_map()?;
        Ok(map.get(key).and_then(Value::as_str).map(str::to_owned))
    }

    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let mut map = self.read_map()?;
        map.insert(key.to_owned(), Value::String(value.to_owned()));
        self.write_map(&map)
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        let mut map = self.read_map()?;
        map.remove(key);
        self.write_map(&map)
    }
}

// ---------------------------------------------------------------------------
// KeychainStore (macOS)
// ---------------------------------------------------------------------------

pub struct KeychainStore {
    service: String,
}

impl KeychainStore {
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    fn current_user() -> String {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default()
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0x0F), 16).unwrap_or('0'));
    }
    out
}

fn hex_decode(s: &str) -> io::Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "odd-length hex string"));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = decode_hex_nibble(chunk[0])?;
        let lo = decode_hex_nibble(chunk[1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn decode_hex_nibble(b: u8) -> io::Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid hex byte: {b}"),
        )),
    }
}

impl SecretStore for KeychainStore {
    fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let user = Self::current_user();
        let service = format!("{}-{}", self.service, key);
        let hex = hex_encode(value.as_bytes());
        // Pass value via stdin with `-i` flag — never in argv
        let cmd_input = format!("add-generic-password -U -a {user} -s {service} -X {hex}\n");
        let mut child = Command::new("security")
            .arg("-i")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        if let Some(stdin) = child.stdin.take() {
            use io::Write;
            let mut stdin = stdin;
            stdin.write_all(cmd_input.as_bytes())?;
        }
        child.wait()?;
        Ok(())
    }

    fn get(&self, key: &str) -> io::Result<Option<String>> {
        let user = Self::current_user();
        let service = format!("{}-{}", self.service, key);
        let output = Command::new("security")
            .args(["find-generic-password", "-a", &user, "-s", &service, "-w"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Ok(None);
        }
        let hex = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if hex.is_empty() {
            return Ok(None);
        }
        let bytes = hex_decode(&hex)?;
        let s = String::from_utf8(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some(s))
    }

    fn delete(&self, key: &str) -> io::Result<()> {
        let user = Self::current_user();
        let service = format!("{}-{}", self.service, key);
        // Ignore non-zero exit (entry may not exist)
        let _ = Command::new("security")
            .args(["delete-generic-password", "-a", &user, "-s", &service])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// auto_store
// ---------------------------------------------------------------------------

#[must_use]
pub fn auto_store() -> Box<dyn SecretStore> {
    #[cfg(target_os = "macos")]
    {
        let force_file = std::env::var("ELAI_FORCE_FILE_STORE").is_ok();
        let has_security = Command::new("which")
            .arg("security")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !force_file && has_security {
            return Box::new(KeychainStore::new("Elai Code-credentials"));
        }
    }
    let base = credentials_home_dir()
        .unwrap_or_else(|_| PathBuf::from(".elai"))
        .join("secrets.json");
    Box::new(FileStore::new(base))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_secrets_path() -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        std::env::temp_dir().join(format!(
            "elai-secret-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn file_store_round_trip() {
        let dir = temp_secrets_path();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("secrets.json");
        let store = FileStore::new(path);

        store.set("mykey", "myvalue").expect("set");
        assert_eq!(store.get("mykey").expect("get"), Some("myvalue".to_owned()));

        store.delete("mykey").expect("delete");
        assert_eq!(store.get("mykey").expect("get after delete"), None);

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn auto_store_falls_back_to_file_when_forced() {
        let _guard = crate::test_env_lock();
        std::env::set_var("ELAI_FORCE_FILE_STORE", "1");

        let dir = temp_secrets_path();
        std::fs::create_dir_all(&dir).expect("create temp dir");
        std::env::set_var("ELAI_CONFIG_HOME", &dir);

        let store = auto_store();
        store.set("testkey", "testval").expect("set via auto_store");
        let got = store.get("testkey").expect("get via auto_store");
        assert_eq!(got, Some("testval".to_owned()));

        std::env::remove_var("ELAI_FORCE_FILE_STORE");
        std::env::remove_var("ELAI_CONFIG_HOME");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
