//! Version checking and self-update against GitHub Releases.

// These functions are public API for later tasks (5, 6, 9) that add the
// network check and update flow.  They are exercised by the unit tests
// below; allow dead_code so clippy stays clean on the binary crate until
// those callers land.
#![allow(dead_code)]

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
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

const RELEASES_API: &str = "https://api.github.com/repos/alveflo/kamaji/releases/latest";

/// On-disk cache path: `<cache_dir>/update-check.json`.
pub fn cache_path() -> Option<PathBuf> {
    let dirs = ProjectDirs::from("", "", "kamaji")?;
    Some(dirs.cache_dir().join("update-check.json"))
}

/// GET the latest release tag from the GitHub API. GitHub rejects requests
/// without a User-Agent. Timeouts bound the worst-case lifetime of the
/// background check thread if a connection hangs.
fn fetch_latest_tag() -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5))
        .timeout_read(std::time::Duration::from_secs(10))
        .build();
    let body = agent
        .get(RELEASES_API)
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("requesting latest release")?
        .into_string()
        .context("reading release response")?;
    parse_latest_tag(&body).context("no tag_name in release response")
}

/// Return `Some(version)` if a newer release than this build is available.
/// Uses the on-disk cache (TTL `TTL_SECS`); refreshes it on a miss. Any error
/// (network down, no cache dir, rate-limited) yields `None` — the check is
/// best-effort and never surfaces failures.
pub fn check(cache_path: &Path) -> Option<String> {
    let now = now_secs();

    let latest = match read_cache(cache_path) {
        Some(entry) if is_fresh(&entry, now, TTL_SECS) => entry.latest_version,
        _ => {
            let tag = fetch_latest_tag().ok()?;
            let _ = write_cache(
                cache_path,
                &CacheEntry {
                    checked_at: now,
                    latest_version: tag.clone(),
                },
            );
            tag
        }
    };

    if is_newer(&latest, current_version()) {
        Some(latest)
    } else {
        None
    }
}

/// Download the latest release asset for this platform, verify its checksum,
/// and atomically replace the running executable. The new binary takes effect
/// on the next launch (the caller should ask the user to restart).
///
/// The `.sha256` is fetched from the same origin, so it guards integrity (a
/// corrupted or truncated download) but not authenticity — it trusts whoever
/// publishes the repo's GitHub releases, exactly as `install.sh` does.
pub fn self_update() -> Result<()> {
    let exe = std::env::current_exe().context("locating current executable")?;
    let dir = exe.parent().context("executable has no parent dir")?;

    let asset = format!("kamaji-{}.tar.gz", current_target());
    let base = "https://github.com/alveflo/kamaji/releases/latest/download";

    // Timeouts bound a stalled connection so an interrupted download can't hang
    // the process after the TUI has already torn down the terminal.
    let agent = download_agent();

    // Download tarball bytes.
    let tarball = http_get_bytes(&agent, &format!("{base}/{asset}"))
        .context("downloading release archive")?;

    // Download + verify checksum.
    let sums = agent
        .get(&format!("{base}/{asset}.sha256"))
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("downloading checksum")?
        .into_string()
        .context("reading checksum")?;
    let expected = sums
        .split_whitespace()
        .next()
        .context("empty checksum file")?;
    let mut hasher = Sha256::new();
    hasher.update(&tarball);
    let actual = hasher.finalize();
    let actual_hex: String = actual.iter().map(|b| format!("{b:02x}")).collect();
    if !actual_hex.eq_ignore_ascii_case(expected) {
        anyhow::bail!("checksum mismatch (expected {expected}, got {actual_hex})");
    }

    // Extract into a temp dir on the same filesystem as the executable, so the
    // final rename is atomic.
    let tmp = dir.join(".kamaji-update-tmp");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).context("creating temp dir")?;
    let archive = tmp.join(&asset);
    std::fs::write(&archive, &tarball).context("writing archive")?;

    let status = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&tmp)
        .status()
        .context("running tar")?;
    if !status.success() {
        anyhow::bail!("tar extraction failed");
    }

    let new_bin = tmp.join("kamaji");
    if !new_bin.exists() {
        anyhow::bail!("release archive did not contain a 'kamaji' binary");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_bin, std::fs::Permissions::from_mode(0o755))
            .context("setting executable bit")?;
    }

    std::fs::rename(&new_bin, &exe).context("replacing executable")?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

/// HTTP agent for release downloads, with timeouts so a stalled connection
/// aborts rather than hanging indefinitely.
fn download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30))
        .build()
}

fn http_get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    // Cap the body so a wrong/huge URL can't exhaust memory. Release binaries
    // are a few MB; 200 MB is comfortably above any real asset.
    const MAX_BYTES: u64 = 200 * 1024 * 1024;
    let resp = agent
        .get(url)
        .set("User-Agent", concat!("kamaji/", env!("CARGO_PKG_VERSION")))
        .call()?;
    let mut buf = Vec::new();
    resp.into_reader().take(MAX_BYTES).read_to_end(&mut buf)?;
    Ok(buf)
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
