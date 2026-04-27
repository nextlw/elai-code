use std::io;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ClaudeCodeCredentialsSnapshot {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<u64>,
    pub scopes: Vec<String>,
    pub subscription_type: Option<String>,
    pub source: ClaudeCodeCredSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeCodeCredSource {
    MacOsKeychain,
    HomeDirFile,
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

#[must_use]
pub fn detect_claude_code_credentials() -> Option<ClaudeCodeCredentialsSnapshot> {
    if let Some(snap) = read_from_keychain() {
        return Some(snap);
    }
    read_from_home_file()
}

// ---------------------------------------------------------------------------
// macOS Keychain reader
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn read_from_keychain() -> Option<ClaudeCodeCredentialsSnapshot> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json_str = String::from_utf8(output.stdout).ok()?;
    parse_credentials_json(&json_str, ClaudeCodeCredSource::MacOsKeychain)
}

#[cfg(not(target_os = "macos"))]
fn read_from_keychain() -> Option<ClaudeCodeCredentialsSnapshot> {
    None
}

// ---------------------------------------------------------------------------
// Home directory file reader
// ---------------------------------------------------------------------------

fn read_from_home_file() -> Option<ClaudeCodeCredentialsSnapshot> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let path = PathBuf::from(home).join(".claude").join(".credentials.json");
    let contents = std::fs::read_to_string(&path).ok()?;
    parse_credentials_json(&contents, ClaudeCodeCredSource::HomeDirFile)
}

// ---------------------------------------------------------------------------
// JSON parser
// ---------------------------------------------------------------------------

fn parse_credentials_json(
    s: &str,
    source: ClaudeCodeCredSource,
) -> Option<ClaudeCodeCredentialsSnapshot> {
    let root: serde_json::Value = serde_json::from_str(s).ok()?;
    let oauth = root.get("claudeAiOauth")?.as_object()?;

    let access_token = oauth.get("accessToken")?.as_str()?.to_owned();

    let refresh_token = oauth
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let expires_at_ms = oauth.get("expiresAt").and_then(serde_json::Value::as_u64);

    let scopes = oauth
        .get("scopes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let subscription_type = oauth
        .get("subscriptionType")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    Some(ClaudeCodeCredentialsSnapshot {
        access_token,
        refresh_token,
        expires_at_ms,
        scopes,
        subscription_type,
        source,
    })
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

pub fn import_claude_code_credentials() -> io::Result<Option<crate::AuthMethod>> {
    let Some(snap) = detect_claude_code_credentials() else {
        return Ok(None);
    };
    // expires_at_ms is in milliseconds; OAuthTokenSet::expires_at uses seconds
    let expires_at_secs = snap.expires_at_ms.map(|ms| ms / 1000);
    let token_set = crate::OAuthTokenSet {
        access_token: snap.access_token,
        refresh_token: snap.refresh_token,
        expires_at: expires_at_secs,
        scopes: snap.scopes,
    };
    let method = crate::AuthMethod::ClaudeAiOAuth {
        token_set,
        subscription: snap.subscription_type,
    };
    crate::save_auth_method(&method)?;
    Ok(Some(method))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_JSON: &str = r#"{
        "claudeAiOauth": {
            "accessToken": "acc-tok-123",
            "refreshToken": "ref-tok-456",
            "expiresAt": 1700000000000,
            "scopes": ["user:profile", "user:inference"],
            "subscriptionType": "max",
            "rateLimitTier": "standard"
        }
    }"#;

    const MINIMAL_JSON: &str = r#"{
        "claudeAiOauth": {
            "accessToken": "only-access"
        }
    }"#;

    #[test]
    fn parse_credentials_json_extracts_all_fields() {
        let snap =
            parse_credentials_json(FULL_JSON, ClaudeCodeCredSource::HomeDirFile).expect("should parse");
        assert_eq!(snap.access_token, "acc-tok-123");
        assert_eq!(snap.refresh_token.as_deref(), Some("ref-tok-456"));
        assert_eq!(snap.expires_at_ms, Some(1_700_000_000_000));
        assert_eq!(snap.scopes, vec!["user:profile", "user:inference"]);
        assert_eq!(snap.subscription_type.as_deref(), Some("max"));
        assert_eq!(snap.source, ClaudeCodeCredSource::HomeDirFile);
    }

    #[test]
    fn parse_credentials_json_handles_minimal() {
        let snap =
            parse_credentials_json(MINIMAL_JSON, ClaudeCodeCredSource::HomeDirFile).expect("should parse");
        assert_eq!(snap.access_token, "only-access");
        assert_eq!(snap.refresh_token, None);
        assert_eq!(snap.expires_at_ms, None);
        assert!(snap.scopes.is_empty());
        assert_eq!(snap.subscription_type, None);
    }

    #[test]
    fn parse_credentials_json_returns_none_for_missing_oauth_key() {
        let result = parse_credentials_json(r#"{"other": 1}"#, ClaudeCodeCredSource::HomeDirFile);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credentials_json_returns_none_for_invalid_json() {
        let result = parse_credentials_json("not json at all {{", ClaudeCodeCredSource::HomeDirFile);
        assert!(result.is_none());
    }

    #[test]
    fn parse_credentials_json_returns_none_when_access_token_missing() {
        let json = r#"{"claudeAiOauth": {"refreshToken": "ref"}}"#;
        let result = parse_credentials_json(json, ClaudeCodeCredSource::HomeDirFile);
        assert!(result.is_none());
    }

    #[test]
    fn read_from_home_file_returns_some_when_file_exists() {
        let _guard = crate::test_env_lock();

        let tmp = tempfile::tempdir().expect("tmpdir");
        let claude_dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("create .claude dir");
        std::fs::write(claude_dir.join(".credentials.json"), FULL_JSON).expect("write creds");

        std::env::set_var("HOME", tmp.path());
        std::env::remove_var("USERPROFILE");

        let result = read_from_home_file();

        std::env::remove_var("HOME");

        assert!(result.is_some());
        let snap = result.unwrap();
        assert_eq!(snap.access_token, "acc-tok-123");
        assert_eq!(snap.source, ClaudeCodeCredSource::HomeDirFile);
    }

    #[test]
    fn read_from_home_file_returns_none_when_file_missing() {
        let _guard = crate::test_env_lock();

        let tmp = tempfile::tempdir().expect("tmpdir");
        std::env::set_var("HOME", tmp.path());
        std::env::remove_var("USERPROFILE");

        let result = read_from_home_file();

        std::env::remove_var("HOME");

        assert!(result.is_none());
    }

    #[test]
    fn import_credentials_persists_via_save_auth_method() {
        let _guard = crate::test_env_lock();

        let tmp_home = tempfile::tempdir().expect("tmpdir home");
        let tmp_config = tempfile::tempdir().expect("tmpdir config");

        let claude_dir = tmp_home.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("create .claude dir");
        std::fs::write(claude_dir.join(".credentials.json"), FULL_JSON).expect("write creds");

        std::env::set_var("HOME", tmp_home.path());
        std::env::remove_var("USERPROFILE");
        std::env::set_var("ELAI_CONFIG_HOME", tmp_config.path());

        let result = import_claude_code_credentials().expect("import ok");
        assert!(result.is_some());

        let loaded = crate::load_auth_method().expect("load_auth_method").expect("some method");
        match loaded {
            crate::AuthMethod::ClaudeAiOAuth { token_set, subscription } => {
                assert_eq!(token_set.access_token, "acc-tok-123");
                assert_eq!(token_set.refresh_token.as_deref(), Some("ref-tok-456"));
                assert_eq!(subscription.as_deref(), Some("max"));
            }
            other => panic!("unexpected method: {other:?}"),
        }

        std::env::remove_var("HOME");
        std::env::remove_var("ELAI_CONFIG_HOME");
    }

    #[test]
    fn expires_at_ms_converted_to_seconds_in_import() {
        let _guard = crate::test_env_lock();

        let tmp_home = tempfile::tempdir().expect("tmpdir home");
        let tmp_config = tempfile::tempdir().expect("tmpdir config");

        let json = r#"{
            "claudeAiOauth": {
                "accessToken": "tok",
                "expiresAt": 1700000000000
            }
        }"#;

        let claude_dir = tmp_home.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).expect("create .claude dir");
        std::fs::write(claude_dir.join(".credentials.json"), json).expect("write creds");

        std::env::set_var("HOME", tmp_home.path());
        std::env::remove_var("USERPROFILE");
        std::env::set_var("ELAI_CONFIG_HOME", tmp_config.path());

        let result = import_claude_code_credentials().expect("import ok");
        assert!(result.is_some());

        let loaded = crate::load_auth_method().expect("load").expect("some");
        match loaded {
            crate::AuthMethod::ClaudeAiOAuth { token_set, .. } => {
                assert_eq!(token_set.expires_at, Some(1_700_000_000_u64));
            }
            other => panic!("unexpected method: {other:?}"),
        }

        std::env::remove_var("HOME");
        std::env::remove_var("ELAI_CONFIG_HOME");
    }
}
