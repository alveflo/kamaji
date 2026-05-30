//! Centralized application directory resolution.
//!
//! Unix (Linux **and** macOS) uses the XDG Base Directory spec so kamaji's
//! files live in the same `~/.config`, `~/.local/share`, and `~/.cache`
//! locations on both platforms. This fixes issue #60, where macOS previously
//! diverged to `~/Library/Application Support` (the `directories` crate's
//! native macOS convention) and users could not find `~/.config/kamaji/`.
//!
//! Windows keeps its native `AppData` directories — `~/.config` is not a
//! Windows convention and `$HOME` is unreliable there.

use std::path::PathBuf;

/// Application directory leaf, appended to every base directory.
const APP: &str = "kamaji";

/// `<config>/kamaji` — `$XDG_CONFIG_HOME` or `~/.config` on Unix,
/// the native config dir on Windows. `None` if no home can be determined.
pub fn config_dir() -> Option<PathBuf> {
    base_dir(BaseKind::Config)
}

/// `<data>/kamaji` — `$XDG_DATA_HOME` or `~/.local/share` on Unix,
/// the native data dir on Windows.
pub fn data_dir() -> Option<PathBuf> {
    base_dir(BaseKind::Data)
}

/// `<cache>/kamaji` — `$XDG_CACHE_HOME` or `~/.cache` on Unix,
/// the native cache dir on Windows.
pub fn cache_dir() -> Option<PathBuf> {
    base_dir(BaseKind::Cache)
}

/// `<runtime>/kamaji` for ephemeral runtime files (pidfile, addr). Uses
/// `$XDG_RUNTIME_DIR` when set to an absolute path, otherwise falls back to
/// `cache_dir()`. The `kamaji` leaf is appended to an `$XDG_RUNTIME_DIR` base;
/// the cache fallback already carries it.
#[cfg(not(windows))]
pub fn runtime_dir() -> Option<PathBuf> {
    resolve_runtime(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), cache_dir())
}

#[cfg(windows)]
pub fn runtime_dir() -> Option<PathBuf> {
    cache_dir()
}

#[cfg(not(windows))]
fn resolve_runtime(
    xdg_runtime: Option<&std::ffi::OsStr>,
    cache: Option<PathBuf>,
) -> Option<PathBuf> {
    xdg_runtime
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(APP))
        .or(cache)
}

/// Which of the three base directories to resolve.
#[derive(Clone, Copy)]
enum BaseKind {
    Config,
    Data,
    Cache,
}

#[cfg(not(windows))]
fn base_dir(kind: BaseKind) -> Option<PathBuf> {
    let (env_var, fallback) = match kind {
        BaseKind::Config => ("XDG_CONFIG_HOME", ".config"),
        BaseKind::Data => ("XDG_DATA_HOME", ".local/share"),
        BaseKind::Cache => ("XDG_CACHE_HOME", ".cache"),
    };
    resolve_xdg(
        std::env::var_os(env_var).as_deref(),
        std::env::var_os("HOME").as_deref(),
        fallback,
    )
}

#[cfg(windows)]
fn base_dir(kind: BaseKind) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", APP)?;
    let base = match kind {
        BaseKind::Config => dirs.config_dir(),
        BaseKind::Data => dirs.data_dir(),
        BaseKind::Cache => dirs.cache_dir(),
    };
    Some(base.to_path_buf())
}

/// Pure XDG resolver: honor `$XDG_*` when set to an *absolute* path (the spec
/// requires relative values be ignored), otherwise fall back to `$HOME`/`fallback`.
/// The `kamaji` leaf is appended to the resolved base. `None` when no usable
/// base exists (no absolute override and no `$HOME`).
#[cfg(not(windows))]
fn resolve_xdg(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
    fallback: &str,
) -> Option<PathBuf> {
    let base = xdg
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            let home = home.filter(|h| !h.is_empty())?;
            Some(PathBuf::from(home).join(fallback))
        })?;
    Some(base.join(APP))
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn absolute_xdg_override_wins() {
        let got = resolve_xdg(
            Some(OsStr::new("/xdg/cfg")),
            Some(OsStr::new("/home/u")),
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/xdg/cfg/kamaji")));
    }

    #[test]
    fn relative_xdg_override_is_ignored() {
        // The XDG spec says relative values must be ignored; fall back to $HOME.
        let got = resolve_xdg(
            Some(OsStr::new("rel/cfg")),
            Some(OsStr::new("/home/u")),
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/home/u/.config/kamaji")));
    }

    #[test]
    fn empty_xdg_override_is_ignored() {
        let got = resolve_xdg(Some(OsStr::new("")), Some(OsStr::new("/home/u")), ".config");
        assert_eq!(got, Some(PathBuf::from("/home/u/.config/kamaji")));
    }

    #[test]
    fn falls_back_to_home_when_no_override() {
        let got = resolve_xdg(None, Some(OsStr::new("/home/u")), ".local/share");
        assert_eq!(got, Some(PathBuf::from("/home/u/.local/share/kamaji")));
    }

    #[test]
    fn none_without_override_or_home() {
        assert_eq!(resolve_xdg(None, None, ".config"), None);
    }

    #[test]
    fn none_when_home_is_empty() {
        assert_eq!(resolve_xdg(None, Some(OsStr::new("")), ".config"), None);
    }

    #[test]
    fn runtime_dir_prefers_xdg_runtime_dir() {
        let got = resolve_runtime(
            Some(OsStr::new("/run/user/1000")),
            Some(PathBuf::from("/home/u/.cache/kamaji")),
        );
        assert_eq!(got, Some(PathBuf::from("/run/user/1000/kamaji")));
    }

    #[test]
    fn runtime_dir_falls_back_to_cache_when_no_xdg_runtime() {
        let got = resolve_runtime(None, Some(PathBuf::from("/home/u/.cache/kamaji")));
        assert_eq!(got, Some(PathBuf::from("/home/u/.cache/kamaji")));
    }

    #[test]
    fn runtime_dir_relative_xdg_runtime_is_ignored() {
        let got = resolve_runtime(
            Some(OsStr::new("rel/run")),
            Some(PathBuf::from("/home/u/.cache/kamaji")),
        );
        assert_eq!(got, Some(PathBuf::from("/home/u/.cache/kamaji")));
    }
}
