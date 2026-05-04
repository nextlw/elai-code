//! Rate limiting via rolling window bucket (in-memory, per session).
//!
//! `RateLimitBucket` tracks call timestamps in a `VecDeque` and evicts entries
//! older than the configured window before each check. Thread-safety is the
//! caller's responsibility (typically a Mutex in `RateLimiter`).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

// ─── Core types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RateLimit {
    pub max_calls: u32,
    pub window: Duration,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            max_calls: 60,
            window: Duration::from_mins(1),
        }
    }
}

/// Rolling window bucket per tool id. Thread-safe via external Mutex (called from `ToolExecutor`).
pub struct RateLimitBucket {
    pub config: RateLimit,
    calls: VecDeque<Instant>,
}

impl RateLimitBucket {
    #[must_use] 
    pub fn new(config: RateLimit) -> Self {
        Self {
            config,
            calls: VecDeque::new(),
        }
    }

    /// Attempts to register a call. Returns `Ok(())` or `Err(retry_after)`.
    pub fn try_call(&mut self) -> Result<(), Duration> {
        let now = Instant::now();
        // Remove expired entries from the window.
        while self
            .calls
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.config.window)
        {
            self.calls.pop_front();
        }

        if self.calls.len() >= self.config.max_calls as usize {
            // Time until the window opens = oldest + window - now
            let oldest = *self.calls.front().unwrap();
            let retry_after = self.config.window.checked_sub(now.duration_since(oldest)).unwrap();
            Err(retry_after)
        } else {
            self.calls.push_back(now);
            Ok(())
        }
    }
}

// ─── Registry ────────────────────────────────────────────────────────────────

pub struct RateLimiter {
    buckets: HashMap<String, RateLimitBucket>,
    default_config: RateLimit,
    /// Per-tool overrides loaded from the catalog.
    overrides: HashMap<String, RateLimit>,
}

impl RateLimiter {
    #[must_use] 
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
            default_config: RateLimit::default(),
            overrides: HashMap::new(),
        }
    }

    #[must_use] 
    pub fn with_overrides(overrides: HashMap<String, RateLimit>) -> Self {
        Self {
            overrides,
            ..Self::new()
        }
    }

    /// Checks the rate limit before execution. Returns `Err(retry_after)` if exceeded.
    pub fn check(&mut self, tool_id: &str) -> Result<(), Duration> {
        let config = self
            .overrides
            .get(tool_id)
            .cloned()
            .unwrap_or_else(|| self.default_config.clone());
        self.buckets
            .entry(tool_id.to_string())
            .or_insert_with(|| RateLimitBucket::new(config))
            .try_call()
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Session singleton ────────────────────────────────────────────────────────

use std::sync::Mutex;

static SESSION_RATE_LIMITER: Mutex<Option<RateLimiter>> = Mutex::new(None);

/// Initialises the session-scoped rate limiter with per-tool overrides.
/// Call once during bootstrap, before any tool executions.
#[allow(clippy::implicit_hasher)]
pub fn init_rate_limiter(overrides: HashMap<String, RateLimit>) {
    *SESSION_RATE_LIMITER.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(RateLimiter::with_overrides(overrides));
}

/// Checks the rate limit for `tool_id`.
/// Initialises a default limiter on first call if `init_rate_limiter` was not called.
pub fn check_rate_limit(tool_id: &str) -> Result<(), Duration> {
    let mut guard =
        SESSION_RATE_LIMITER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let limiter = guard.get_or_insert_with(RateLimiter::new);
    limiter.check(tool_id)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_blocks_after_max_calls() {
        let mut bucket = RateLimitBucket::new(RateLimit {
            max_calls: 3,
            window: Duration::from_secs(60),
        });
        assert!(bucket.try_call().is_ok());
        assert!(bucket.try_call().is_ok());
        assert!(bucket.try_call().is_ok());
        assert!(bucket.try_call().is_err()); // 4th call blocked
    }

    #[test]
    fn rate_limit_resets_after_window() {
        let mut bucket = RateLimitBucket::new(RateLimit {
            max_calls: 1,
            window: Duration::from_millis(50),
        });
        assert!(bucket.try_call().is_ok());
        assert!(bucket.try_call().is_err());
        std::thread::sleep(Duration::from_millis(60));
        assert!(bucket.try_call().is_ok()); // window expired, reset
    }

    #[test]
    fn rate_limiter_per_tool_isolation() {
        let mut limiter = RateLimiter::new();
        // Exhaust read_file (default 60 calls)
        for _ in 0..60 {
            limiter.check("read_file").ok();
        }
        // write_file has its own independent bucket
        assert!(limiter.check("write_file").is_ok());
    }

    #[test]
    fn rate_limiter_with_overrides_uses_custom_limit() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "execute_bash".to_string(),
            RateLimit { max_calls: 2, window: Duration::from_secs(60) },
        );
        let mut limiter = RateLimiter::with_overrides(overrides);
        assert!(limiter.check("execute_bash").is_ok());
        assert!(limiter.check("execute_bash").is_ok());
        assert!(limiter.check("execute_bash").is_err()); // 3rd blocked
    }

    #[test]
    fn retry_after_is_positive() {
        let mut bucket = RateLimitBucket::new(RateLimit {
            max_calls: 1,
            window: Duration::from_secs(10),
        });
        bucket.try_call().unwrap();
        let err = bucket.try_call().unwrap_err();
        assert!(err.as_secs() > 0, "retry_after should be positive, got {err:?}");
    }
}
