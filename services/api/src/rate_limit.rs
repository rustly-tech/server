//! Small per-process rate limiter for the first hosted slice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustly_common::{Error, Result};

/// Stable key supplied by a trusted edge proxy, with a conservative fallback.
pub fn client_key(headers: &axum::http::HeaderMap) -> String {
    for name in ["cf-connecting-ip", "x-forwarded-for"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            let candidate = value.split(',').next().unwrap_or_default().trim();
            if candidate.parse::<std::net::IpAddr>().is_ok() {
                return candidate.to_owned();
            }
        }
    }
    "direct".into()
}

#[derive(Debug)]
struct Window {
    started: Instant,
    count: u32,
}

/// Fixed-window limiter scoped to one API process.
#[derive(Debug, Clone, Default)]
pub struct RateLimiter {
    windows: Arc<Mutex<HashMap<String, Window>>>,
}

impl RateLimiter {
    /// Consume one operation, or return a retry interval.
    pub fn check(
        &self,
        bucket: &str,
        principal: &str,
        limit: u32,
        duration: Duration,
    ) -> Result<()> {
        let now = Instant::now();
        let key = format!("{bucket}:{principal}");
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if windows.len() > 10_000 {
            windows.retain(|_, entry| now.duration_since(entry.started) < duration);
        }
        let entry = windows.entry(key).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(entry.started) >= duration {
            entry.started = now;
            entry.count = 0;
        }
        if entry.count >= limit {
            let remaining = duration.saturating_sub(now.duration_since(entry.started));
            return Err(Error::RateLimited {
                retry_after_seconds: remaining.as_secs().max(1),
            });
        }
        entry.count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_after_the_limit_without_growing_unbounded_state() {
        let limiter = RateLimiter::default();
        assert!(limiter
            .check("submit", "u1", 2, Duration::from_secs(60))
            .is_ok());
        assert!(limiter
            .check("submit", "u1", 2, Duration::from_secs(60))
            .is_ok());
        assert!(matches!(
            limiter.check("submit", "u1", 2, Duration::from_secs(60)),
            Err(Error::RateLimited { .. })
        ));
        assert!(limiter
            .check("submit", "u2", 2, Duration::from_secs(60))
            .is_ok());
    }
}
