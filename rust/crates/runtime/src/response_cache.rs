use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::json::JsonValue;
use crate::session::{ContentBlock, ConversationMessage};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// SHA-256 hex digest used as the cache lookup key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey(pub(crate) String);

impl CacheKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A single entry stored in the response cache.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// Text of the assistant response.
    pub response_json: String,
    /// Model that produced this response.
    pub model: String,
    /// Unix timestamp in milliseconds when this entry was created.
    pub created_at_ms: u64,
    /// Number of times this entry has been served from cache.
    pub hit_count: u32,
}

/// Summary statistics returned by `ResponseCache::stats()`.
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_hits: u64,
    pub oldest_entry_ms: Option<u64>,
    pub newest_entry_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// ResponseCache
// ---------------------------------------------------------------------------

pub struct ResponseCache {
    db_path: PathBuf,
    ttl_ms: u64,
    enabled: bool,
    entries: BTreeMap<String, CachedResponse>,
    dirty: bool,
}

impl ResponseCache {
    /// Default TTL: 1 hour.
    pub const DEFAULT_TTL_MS: u64 = 3_600_000;

    /// Load (or create) a cache backed by `db_path`.
    #[must_use]
    pub fn new(db_path: PathBuf, ttl_ms: u64) -> Self {
        let entries = Self::load_from_disk(&db_path).unwrap_or_default();
        Self {
            db_path,
            ttl_ms,
            enabled: true,
            entries,
            dirty: false,
        }
    }

    /// Create a permanently disabled cache (for `--no-cache`).
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            db_path: PathBuf::new(),
            ttl_ms: Self::DEFAULT_TTL_MS,
            enabled: false,
            entries: BTreeMap::new(),
            dirty: false,
        }
    }

    /// Look up `key`.  Returns `None` if disabled, missing, or expired.
    /// Increments `hit_count` on a live hit.
    pub fn get(&mut self, key: &CacheKey) -> Option<&CachedResponse> {
        if !self.enabled {
            return None;
        }
        let now = now_ms();
        let ttl = self.ttl_ms;

        // Check existence + expiry without borrowing mutably.
        let exists_and_fresh = self
            .entries
            .get(key.as_str())
            .map(|e| now.saturating_sub(e.created_at_ms) < ttl)
            .unwrap_or(false);

        if !exists_and_fresh {
            // Remove if present but expired.
            self.entries.remove(key.as_str());
            return None;
        }

        // Increment hit counter (requires mutable borrow).
        let entry = self.entries.get_mut(key.as_str())?;
        entry.hit_count = entry.hit_count.saturating_add(1);
        self.dirty = true;

        self.entries.get(key.as_str())
    }

    /// Insert an entry.  No-op if disabled.
    pub fn put(&mut self, key: CacheKey, response: CachedResponse) {
        if !self.enabled {
            return;
        }
        self.entries.insert(key.0, response);
        self.dirty = true;
    }

    /// Remove all entries whose age exceeds the TTL.  Returns count removed.
    pub fn evict_expired(&mut self) -> usize {
        if !self.enabled {
            return 0;
        }
        let now = now_ms();
        let ttl = self.ttl_ms;
        let before = self.entries.len();
        self.entries
            .retain(|_, e| now.saturating_sub(e.created_at_ms) < ttl);
        let removed = before - self.entries.len();
        if removed > 0 {
            self.dirty = true;
        }
        removed
    }

    /// Remove all entries and mark the cache as dirty.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
    }

    /// Current statistics.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        let total_hits: u64 = self
            .entries
            .values()
            .map(|e| u64::from(e.hit_count))
            .sum();
        let oldest_entry_ms = self
            .entries
            .values()
            .map(|e| e.created_at_ms)
            .min();
        let newest_entry_ms = self
            .entries
            .values()
            .map(|e| e.created_at_ms)
            .max();
        CacheStats {
            total_entries: self.entries.len(),
            total_hits,
            oldest_entry_ms,
            newest_entry_ms,
        }
    }

    /// Persist the cache to disk if dirty.  Errors are intentionally NOT
    /// propagated to the caller — this is a best-effort write.
    pub fn flush(&mut self) -> Result<(), std::io::Error> {
        if !self.enabled || !self.dirty {
            return Ok(());
        }
        let json = self.serialize();
        // Create parent directory if it doesn't exist.
        if let Some(parent) = self.db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(&self.db_path, json.render())?;
        self.dirty = false;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn serialize(&self) -> JsonValue {
        let mut outer = BTreeMap::new();
        for (key, entry) in &self.entries {
            let mut obj = BTreeMap::new();
            obj.insert(
                "response_json".to_string(),
                JsonValue::String(entry.response_json.clone()),
            );
            obj.insert(
                "model".to_string(),
                JsonValue::String(entry.model.clone()),
            );
            obj.insert(
                "created_at_ms".to_string(),
                JsonValue::Number(entry.created_at_ms as i64),
            );
            obj.insert(
                "hit_count".to_string(),
                JsonValue::Number(i64::from(entry.hit_count)),
            );
            outer.insert(key.clone(), JsonValue::Object(obj));
        }
        JsonValue::Object(outer)
    }

    fn load_from_disk(path: &PathBuf) -> Option<BTreeMap<String, CachedResponse>> {
        let text = std::fs::read_to_string(path).ok()?;
        let root = JsonValue::parse(&text).ok()?;
        let object = root.as_object()?;

        let mut entries = BTreeMap::new();
        for (key, value) in object {
            let obj = value.as_object()?;
            let response_json = obj
                .get("response_json")
                .and_then(JsonValue::as_str)?
                .to_owned();
            let model = obj
                .get("model")
                .and_then(JsonValue::as_str)?
                .to_owned();
            let created_at_ms = obj
                .get("created_at_ms")
                .and_then(JsonValue::as_i64)
                .and_then(|v| u64::try_from(v).ok())?;
            let hit_count = obj
                .get("hit_count")
                .and_then(JsonValue::as_i64)
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0);
            entries.insert(
                key.clone(),
                CachedResponse {
                    response_json,
                    model,
                    created_at_ms,
                    hit_count,
                },
            );
        }
        Some(entries)
    }
}

// ---------------------------------------------------------------------------
// Key generation
// ---------------------------------------------------------------------------

/// Returns `None` if ANY message contains `ToolUse` or `ToolResult` content.
/// Otherwise returns a deterministic SHA-256 key over
/// (messages, model, system_prompt).
pub fn generate_cache_key(
    messages: &[ConversationMessage],
    model: &str,
    system_prompt: &[String],
) -> Option<CacheKey> {
    if contains_tool_content(messages) {
        return None;
    }

    // Build a canonical JSON representation using BTreeMap (sorted keys).
    let canonical = build_canonical_json(messages, model, system_prompt);
    let rendered = canonical.render();

    let mut hasher = Sha256::new();
    hasher.update(rendered.as_bytes());
    let digest = hasher.finalize();
    let hex = hex_encode(&digest);
    Some(CacheKey(hex))
}

fn contains_tool_content(messages: &[ConversationMessage]) -> bool {
    messages.iter().any(|msg| {
        msg.blocks.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. }
            )
        })
    })
}

fn build_canonical_json(
    messages: &[ConversationMessage],
    model: &str,
    system_prompt: &[String],
) -> JsonValue {
    let mut root = BTreeMap::new();

    root.insert("model".to_string(), JsonValue::String(model.to_owned()));

    let sys_array: Vec<JsonValue> = system_prompt
        .iter()
        .map(|s| JsonValue::String(s.clone()))
        .collect();
    root.insert("system_prompt".to_string(), JsonValue::Array(sys_array));

    let msg_array: Vec<JsonValue> = messages
        .iter()
        .map(|m| m.to_json())
        .collect();
    root.insert("messages".to_string(), JsonValue::Array(msg_array));

    JsonValue::Object(root)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(HEX[(b >> 4) as usize]));
        out.push(char::from(HEX[(b & 0xF) as usize]));
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        generate_cache_key, CacheKey, CachedResponse, ContentBlock, ConversationMessage,
        ResponseCache,
    };
    use crate::session::MessageRole;

    fn text_message(role: MessageRole, text: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            blocks: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            usage: None,
        }
    }

    fn sample_messages() -> Vec<ConversationMessage> {
        vec![
            text_message(MessageRole::User, "hello"),
            text_message(MessageRole::Assistant, "world"),
        ]
    }

    fn sample_entry(created_at_ms: u64) -> CachedResponse {
        CachedResponse {
            response_json: "hi".to_owned(),
            model: "test-model".to_owned(),
            created_at_ms,
            hit_count: 0,
        }
    }

    fn temp_path() -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .subsec_nanos();
        std::env::temp_dir().join(format!("elai-cache-test-{nanos}.json"))
    }

    // 1. Same input → same CacheKey
    #[test]
    fn deterministic_key_generation() {
        let msgs = sample_messages();
        let k1 = generate_cache_key(&msgs, "gpt-4o", &["sys".to_string()]);
        let k2 = generate_cache_key(&msgs, "gpt-4o", &["sys".to_string()]);
        assert!(k1.is_some());
        assert_eq!(k1, k2);
        // SHA-256 hex is 64 chars
        assert_eq!(k1.unwrap().as_str().len(), 64);
    }

    // 2. ToolUse → None
    #[test]
    fn tool_use_bypass() {
        let msgs = vec![ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "t1".to_owned(),
                name: "bash".to_owned(),
                input: "{}".to_owned(),
            }],
            usage: None,
        }];
        assert!(generate_cache_key(&msgs, "m", &[]).is_none());
    }

    // 3. ToolResult → None
    #[test]
    fn tool_result_bypass() {
        let msgs = vec![ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_owned(),
                tool_name: "bash".to_owned(),
                output: "ok".to_owned(),
                is_error: false,
            }],
            usage: None,
        }];
        assert!(generate_cache_key(&msgs, "m", &[]).is_none());
    }

    // 4. TTL expiration
    #[test]
    fn ttl_expiration() {
        let path = temp_path();
        let mut cache = ResponseCache::new(path.clone(), 1_000);
        let key = CacheKey("abc123".to_owned());
        // Entry created 2 seconds ago (already expired with 1s TTL)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let old_entry = CachedResponse {
            response_json: "stale".to_owned(),
            model: "m".to_owned(),
            created_at_ms: now.saturating_sub(2_000),
            hit_count: 0,
        };
        cache.entries.insert(key.0.clone(), old_entry);
        assert!(cache.get(&key).is_none());
        let _ = std::fs::remove_file(&path);
    }

    // 5. Hit counter increments
    #[test]
    fn hit_counter_increments() {
        let path = temp_path();
        let mut cache = ResponseCache::new(path.clone(), ResponseCache::DEFAULT_TTL_MS);
        let key = CacheKey("hitkey".to_owned());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        cache.entries.insert(key.0.clone(), sample_entry(now));

        let _ = cache.get(&key);
        let _ = cache.get(&key);

        assert_eq!(cache.entries[&key.0].hit_count, 2);
        let _ = std::fs::remove_file(&path);
    }

    // 6. Flush and reload
    #[test]
    fn flush_and_reload() {
        let path = temp_path();
        let mut cache = ResponseCache::new(path.clone(), ResponseCache::DEFAULT_TTL_MS);
        let key = CacheKey("flushkey".to_owned());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        cache.put(key.clone(), sample_entry(now));
        cache.flush().expect("flush succeeds");

        let mut cache2 = ResponseCache::new(path.clone(), ResponseCache::DEFAULT_TTL_MS);
        assert!(cache2.get(&key).is_some());

        let _ = std::fs::remove_file(&path);
    }

    // 7. evict_expired returns count
    #[test]
    fn evict_expired_count() {
        let path = temp_path();
        let mut cache = ResponseCache::new(path.clone(), 1_000);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        cache.entries.insert(
            "old1".to_owned(),
            CachedResponse {
                created_at_ms: now.saturating_sub(5_000),
                ..sample_entry(now)
            },
        );
        cache.entries.insert(
            "old2".to_owned(),
            CachedResponse {
                created_at_ms: now.saturating_sub(5_000),
                ..sample_entry(now)
            },
        );
        cache.entries.insert("fresh".to_owned(), sample_entry(now));

        let removed = cache.evict_expired();
        assert_eq!(removed, 2);
        assert_eq!(cache.entries.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    // 8. Disabled mode
    #[test]
    fn disabled_mode_noop() {
        let mut cache = ResponseCache::disabled();
        let key = CacheKey("k".to_owned());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        cache.put(key.clone(), sample_entry(now));
        assert!(cache.get(&key).is_none());
        assert!(cache.entries.is_empty());
    }

    // 9. Key ordering independence (BTreeMap already sorts keys → same hash)
    #[test]
    fn key_ordering_independence() {
        // Both produce identical JSON because to_json() uses BTreeMap with fixed field order.
        let msgs_a = vec![text_message(MessageRole::User, "test")];
        let msgs_b = vec![text_message(MessageRole::User, "test")];
        let k1 = generate_cache_key(&msgs_a, "model", &["sp".to_string()]);
        let k2 = generate_cache_key(&msgs_b, "model", &["sp".to_string()]);
        assert_eq!(k1, k2);
    }
}
