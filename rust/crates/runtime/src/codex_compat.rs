use std::io;
use std::path::PathBuf;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

#[derive(Debug, Clone)]
pub struct CodexCredentialsSnapshot {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub scopes: Vec<String>,
    pub last_refresh: Option<u64>,
    pub source: CodexCredSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexCredSource {
    CodexHomeFile,
}

#[must_use]
pub fn detect_codex_credentials() -> Option<CodexCredentialsSnapshot> {
    let path = codex_auth_path()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    parse_auth_json(&contents, CodexCredSource::CodexHomeFile)
}

pub fn import_codex_credentials() -> io::Result<Option<crate::AuthMethod>> {
    let Some(snapshot) = detect_codex_credentials() else {
        return Ok(None);
    };
    let token_set = crate::OAuthTokenSet {
        access_token: snapshot.access_token,
        refresh_token: snapshot.refresh_token,
        expires_at: snapshot.expires_at,
        scopes: snapshot.scopes,
    };
    let method = crate::AuthMethod::OpenAiCodexOAuth {
        token_set,
        last_refresh: snapshot.last_refresh,
    };
    crate::save_auth_method(&method)?;
    Ok(Some(method))
}

fn codex_auth_path() -> Option<PathBuf> {
    if let Some(codex_home) = std::env::var_os("CODEX_HOME") {
        return Some(PathBuf::from(codex_home).join("auth.json"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".codex").join("auth.json"))
}

fn parse_auth_json(s: &str, source: CodexCredSource) -> Option<CodexCredentialsSnapshot> {
    let root: serde_json::Value = serde_json::from_str(s).ok()?;
    if let Some(auth_mode) = root.get("auth_mode").and_then(|v| v.as_str()) {
        let supported = matches!(auth_mode, "chatgpt" | "chatgptAuthTokens");
        if !supported {
            return None;
        }
    }
    let tokens = root.get("tokens")?.as_object()?;
    let access_token = tokens.get("access_token")?.as_str()?.to_owned();
    let refresh_token = tokens
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let expires_at = tokens
        .get("expires_at")
        .and_then(parse_unix_like_timestamp)
        .map(normalize_epoch_seconds);
    let scopes = tokens
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|scope| {
            scope
                .split_whitespace()
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            tokens
                .get("scopes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_else(|| parse_scopes_from_access_token(&access_token));
    let last_refresh = root
        .get("last_refresh")
        .and_then(parse_unix_like_timestamp)
        .map(normalize_epoch_seconds);

    Some(CodexCredentialsSnapshot {
        access_token,
        refresh_token,
        expires_at,
        scopes,
        last_refresh,
        source,
    })
}

fn parse_scopes_from_access_token(access_token: &str) -> Vec<String> {
    let Some(payload_b64) = access_token.split('.').nth(1) else {
        return Vec::new();
    };
    let Ok(payload_raw) = URL_SAFE_NO_PAD.decode(payload_b64) else {
        return Vec::new();
    };
    let Ok(payload_json) = serde_json::from_slice::<serde_json::Value>(&payload_raw) else {
        return Vec::new();
    };
    let Some(scp) = payload_json.get("scp") else {
        return Vec::new();
    };
    if let Some(values) = scp.as_array() {
        return values
            .iter()
            .filter_map(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    if let Some(value) = scp.as_str() {
        return value
            .split_whitespace()
            .filter(|entry| !entry.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    Vec::new()
}

fn normalize_epoch_seconds(value: u64) -> u64 {
    if value >= 1_000_000_000_000 {
        value / 1000
    } else {
        value
    }
}

fn parse_unix_like_timestamp(value: &serde_json::Value) -> Option<u64> {
    if let Some(raw) = value.as_u64() {
        return Some(raw);
    }
    value
        .as_str()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_JSON: &str = r#"{
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "acc-123",
            "refresh_token": "ref-456",
            "expires_at": 1700000000000,
            "scope": "model.request model.read user.profile"
        },
        "last_refresh": 1700000100000
    }"#;

    const MINIMAL_JSON: &str = r#"{
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "acc-only"
        }
    }"#;

    const NO_AUTH_MODE_ISO_REFRESH_JSON: &str = r#"{
        "tokens": {
            "access_token": "acc-legacy",
            "refresh_token": "ref-legacy"
        },
        "last_refresh": "2026-04-24T18:42:25.440708Z"
    }"#;

    const JWT_WITH_SCP_JSON: &str = r#"{
        "tokens": {
            "access_token": "eyJhbGciOiJub25lIn0.eyJzY3AiOlsib3BlbmlkIiwibW9kZWwucmVxdWVzdCJdfQ.sig"
        }
    }"#;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    #[test]
    fn parse_auth_json_extracts_expected_fields() {
        let snap = parse_auth_json(FULL_JSON, CodexCredSource::CodexHomeFile).expect("parse");
        assert_eq!(snap.access_token, "acc-123");
        assert_eq!(snap.refresh_token.as_deref(), Some("ref-456"));
        assert_eq!(snap.expires_at, Some(1_700_000_000));
        assert_eq!(snap.last_refresh, Some(1_700_000_100));
        assert_eq!(snap.scopes, vec!["model.request", "model.read", "user.profile"]);
        assert_eq!(snap.source, CodexCredSource::CodexHomeFile);
    }

    #[test]
    fn parse_auth_json_handles_minimal_payload() {
        let snap = parse_auth_json(MINIMAL_JSON, CodexCredSource::CodexHomeFile).expect("parse");
        assert_eq!(snap.access_token, "acc-only");
        assert_eq!(snap.refresh_token, None);
        assert_eq!(snap.expires_at, None);
        assert!(snap.scopes.is_empty());
        assert_eq!(snap.last_refresh, None);
    }

    #[test]
    fn parse_auth_json_rejects_non_chatgpt_mode() {
        let bad = r#"{"auth_mode":"api","tokens":{"access_token":"x"}}"#;
        assert!(parse_auth_json(bad, CodexCredSource::CodexHomeFile).is_none());
    }

    #[test]
    fn parse_auth_json_accepts_missing_auth_mode() {
        let snap = parse_auth_json(NO_AUTH_MODE_ISO_REFRESH_JSON, CodexCredSource::CodexHomeFile)
            .expect("parse");
        assert_eq!(snap.access_token, "acc-legacy");
        assert_eq!(snap.refresh_token.as_deref(), Some("ref-legacy"));
        assert_eq!(snap.last_refresh, None);
    }

    #[test]
    fn parse_auth_json_falls_back_to_scp_claim_when_scope_missing() {
        let snap = parse_auth_json(JWT_WITH_SCP_JSON, CodexCredSource::CodexHomeFile)
            .expect("parse");
        assert_eq!(snap.scopes, vec!["openid", "model.request"]);
    }

    #[test]
    fn detect_codex_credentials_reads_codex_home_file() {
        let _guard = env_lock();
        let tmp = tempfile::tempdir().expect("tmpdir");
        std::fs::write(tmp.path().join("auth.json"), FULL_JSON).expect("write auth");
        std::env::set_var("CODEX_HOME", tmp.path());
        std::env::remove_var("HOME");
        std::env::remove_var("USERPROFILE");

        let snap = detect_codex_credentials().expect("detected");
        assert_eq!(snap.access_token, "acc-123");

        std::env::remove_var("CODEX_HOME");
    }

    #[test]
    fn import_codex_credentials_persists_auth_method() {
        let _guard = env_lock();
        let tmp_home = tempfile::tempdir().expect("tmp home");
        let tmp_config = tempfile::tempdir().expect("tmp config");
        let codex_dir = tmp_home.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).expect("create dir");
        std::fs::write(codex_dir.join("auth.json"), FULL_JSON).expect("write auth");

        std::env::set_var("HOME", tmp_home.path());
        std::env::remove_var("USERPROFILE");
        std::env::set_var("ELAI_CONFIG_HOME", tmp_config.path());
        std::env::remove_var("CODEX_HOME");

        let imported = import_codex_credentials().expect("import");
        assert!(imported.is_some());

        let loaded = crate::load_auth_method().expect("load").expect("some");
        match loaded {
            crate::AuthMethod::OpenAiCodexOAuth {
                token_set,
                last_refresh,
            } => {
                assert_eq!(token_set.access_token, "acc-123");
                assert_eq!(token_set.refresh_token.as_deref(), Some("ref-456"));
                assert_eq!(token_set.expires_at, Some(1_700_000_000));
                assert_eq!(last_refresh, Some(1_700_000_100));
            }
            other => panic!("unexpected method: {other:?}"),
        }

        std::env::remove_var("HOME");
        std::env::remove_var("ELAI_CONFIG_HOME");
    }

    #[test]
    fn import_codex_credentials_accepts_token_without_model_request_scope() {
        let _guard = env_lock();
        let tmp_home = tempfile::tempdir().expect("tmp home");
        let tmp_config = tempfile::tempdir().expect("tmp config");
        let codex_dir = tmp_home.path().join(".codex");
        std::fs::create_dir_all(&codex_dir).expect("create dir");
        std::fs::write(codex_dir.join("auth.json"), MINIMAL_JSON).expect("write auth");

        std::env::set_var("HOME", tmp_home.path());
        std::env::remove_var("USERPROFILE");
        std::env::set_var("ELAI_CONFIG_HOME", tmp_config.path());
        std::env::remove_var("CODEX_HOME");

        let imported = import_codex_credentials().expect("import");
        assert!(imported.is_some());

        std::env::remove_var("HOME");
        std::env::remove_var("ELAI_CONFIG_HOME");
    }
}
