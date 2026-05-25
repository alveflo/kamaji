//! Version checking and self-update against GitHub Releases.

// These functions are public API for later tasks (5, 6, 9) that add the
// network check and update flow.  They are exercised by the unit tests
// below; allow dead_code so clippy stays clean on the binary crate until
// those callers land.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// This binary's version, baked in at compile time.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Parse a `vX.Y.Z` (or `X.Y.Z`) string into a comparable tuple.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().strip_prefix('v').unwrap_or(s.trim());
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// True when `latest` is a strictly newer version than `current`.
/// Unparseable inputs yield `false` (never nag on garbage).
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// The release asset target triple for this build. Linux always maps to the
/// musl asset (that is what the release workflow ships), regardless of the
/// toolchain used to compile this binary.
pub fn current_target() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-musl"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-musl"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
}

/// 24h between network checks.
pub const TTL_SECS: u64 = 24 * 60 * 60;

/// Extract `tag_name` from a GitHub `releases/latest` response body.
pub fn parse_latest_tag(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    value.get("tag_name")?.as_str().map(|s| s.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Unix seconds when the check ran.
    pub checked_at: u64,
    /// The latest version string observed (tag, e.g. "v0.3.0").
    pub latest_version: String,
}

pub fn read_cache(path: &Path) -> Option<CacheEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_cache(path: &Path, entry: &CacheEntry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(entry).expect("serializing cache entry");
    std::fs::write(path, text)
}

/// True if `entry` was written within `ttl_secs` of `now` (both unix seconds).
pub fn is_fresh(entry: &CacheEntry, now: u64, ttl_secs: u64) -> bool {
    now.saturating_sub(entry.checked_at) < ttl_secs
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_with_and_without_v() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("v0.10.0"), Some((0, 10, 0)));
    }

    #[test]
    fn rejects_malformed_versions() {
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("nope"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn is_newer_compares_correctly() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn current_target_is_one_of_the_known_triples() {
        let known = [
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
        ];
        assert!(known.contains(&current_target()));
    }

    #[test]
    fn parses_tag_from_release_json() {
        let json = r#"{"url":"x","tag_name":"v0.3.1","name":"0.3.1","draft":false}"#;
        assert_eq!(parse_latest_tag(json).as_deref(), Some("v0.3.1"));
    }

    #[test]
    fn parse_tag_returns_none_on_bad_json() {
        assert_eq!(parse_latest_tag("not json"), None);
        assert_eq!(parse_latest_tag("{}"), None);
    }

    #[test]
    fn cache_round_trips_and_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-check.json");
        let entry = CacheEntry {
            checked_at: 1000,
            latest_version: "0.3.0".into(),
        };
        write_cache(&path, &entry).unwrap();

        let read = read_cache(&path).unwrap();
        assert_eq!(read.latest_version, "0.3.0");
        assert_eq!(read.checked_at, 1000);

        // Fresh within TTL, stale past it.
        assert!(is_fresh(&read, 1000 + 100, 3600));
        assert!(!is_fresh(&read, 1000 + 4000, 3600));
    }
}
