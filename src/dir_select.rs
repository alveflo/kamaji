//! Shared fuzzy directory-selection logic: a text input that suggests
//! subdirectories of the parent segment as you type, completes them, and offers
//! to create a missing directory on submit. Used by both the project-root field
//! in the project picker and the worktree-location field in the board's
//! settings modal so both behave identically.

use std::path::{Path, PathBuf};

/// A directory text input with live fuzzy subdirectory suggestions. `value` is
/// the raw text the user typed (a leading `~` is allowed and only expanded to
/// read the filesystem).
#[derive(Debug, Clone, Default)]
pub struct DirField {
    /// Raw path text (may contain a leading `~`).
    pub value: String,
    /// Subdirectory names matching the current final segment.
    pub suggestions: Vec<String>,
    /// Highlighted entry in `suggestions`.
    pub suggestion_idx: usize,
    /// `Some(path)` once the user has submitted a directory that doesn't exist
    /// yet and we're awaiting their confirmation to create it.
    pub pending_create: Option<PathBuf>,
}

impl DirField {
    pub fn new() -> Self {
        DirField::default()
    }

    /// A field pre-filled with `value`, with suggestions computed for it.
    pub fn with_value(value: impl Into<String>) -> Self {
        let mut f = DirField {
            value: value.into(),
            ..DirField::default()
        };
        f.refresh();
        f
    }

    /// Resolve the entered path, expanding a leading `~`.
    pub fn resolved(&self) -> PathBuf {
        PathBuf::from(shellexpand(&self.value))
    }

    /// Recompute suggestions from the current text, expanding a leading `~` only
    /// to read the filesystem. Resets the highlight to the top.
    pub fn refresh(&mut self) {
        let (parent, partial) = split_path(&self.value);
        let parent_expanded = if parent.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(shellexpand(parent))
        };
        self.suggestions = dir_suggestions(&parent_expanded, partial);
        self.suggestion_idx = 0;
    }

    /// Move the suggestion highlight by `delta`, clamped to the list bounds.
    pub fn move_suggestion(&mut self, delta: isize) {
        if self.suggestions.is_empty() {
            return;
        }
        let max = self.suggestions.len() as isize - 1;
        let next = (self.suggestion_idx as isize + delta).clamp(0, max);
        self.suggestion_idx = next as usize;
    }

    /// Accept the highlighted suggestion: replace the in-progress segment with
    /// the chosen directory name plus a trailing `/`, keeping the literal parent
    /// text (e.g. a `~/` prefix). Then refresh suggestions for the new level.
    pub fn accept_suggestion(&mut self) {
        // Completing the path edits the value, which invalidates a pending
        // "create this directory?" prompt against the old value.
        self.pending_create = None;
        let Some(name) = self.suggestions.get(self.suggestion_idx).cloned() else {
            return;
        };
        let (parent, _partial) = split_path(&self.value);
        self.value = format!("{parent}{name}/");
        self.refresh();
    }

    /// Append a character, invalidate any pending confirmation, and refresh
    /// suggestions for the new text.
    pub fn input_char(&mut self, c: char) {
        self.pending_create = None;
        self.value.push(c);
        self.refresh();
    }

    /// Remove the last character, invalidate any pending confirmation, and
    /// refresh suggestions.
    pub fn backspace(&mut self) {
        self.pending_create = None;
        self.value.pop();
        self.refresh();
    }

    /// Handle Esc. Returns `true` when the field's owner should close; when a
    /// directory-creation prompt is pending, Esc only dismisses that prompt and
    /// returns `false` so editing can continue.
    pub fn escape(&mut self) -> bool {
        if self.pending_create.is_some() {
            self.pending_create = None;
            false
        } else {
            true
        }
    }

    /// Create the directory awaiting confirmation (parents included) and return
    /// it. Returns `Ok(None)` when nothing was pending.
    pub fn confirm_create(&mut self) -> std::io::Result<Option<PathBuf>> {
        match self.pending_create.take() {
            Some(path) => {
                std::fs::create_dir_all(&path)?;
                Ok(Some(path))
            }
            None => Ok(None),
        }
    }
}

/// Outcome of validating a submitted directory path.
pub(crate) enum RootCheck {
    /// Exists and is a directory — ready to use.
    Ready(PathBuf),
    /// Does not exist — offer to create it.
    NeedsConfirm(PathBuf),
    /// Exists but is not a directory (e.g. a file) — unusable, with a message.
    Invalid(String),
}

pub(crate) fn check_root(path: PathBuf) -> RootCheck {
    if path.is_dir() {
        RootCheck::Ready(path)
    } else if path.exists() {
        RootCheck::Invalid(format!("Not a directory: {}", contract_home(&path)))
    } else {
        RootCheck::NeedsConfirm(path)
    }
}

/// Expand a leading `~` to the home directory.
pub(crate) fn shellexpand(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    input.to_string()
}

/// Split a raw path string at its last `/` into `(parent, partial)`.
/// `parent` keeps its trailing slash (or is empty when there is no slash);
/// `partial` is the in-progress final segment.
pub(crate) fn split_path(raw: &str) -> (&str, &str) {
    match raw.rfind('/') {
        Some(i) => (&raw[..=i], &raw[i + 1..]),
        None => ("", raw),
    }
}

/// Case-insensitive subsequence test: are all chars of `partial` found in
/// `candidate` in order (not necessarily contiguous)? Empty `partial` matches.
pub(crate) fn fuzzy_subsequence(partial: &str, candidate: &str) -> bool {
    let mut cand = candidate.chars().flat_map(char::to_lowercase);
    'outer: for pc in partial.chars().flat_map(char::to_lowercase) {
        for cc in cand.by_ref() {
            if cc == pc {
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// List subdirectory names of `parent` whose name fuzzy-matches `partial`.
/// Names that start with `partial` (case-insensitive) sort first; the rest
/// follow, each group alphabetical (case-insensitive). A parent that cannot be
/// read yields an empty list.
pub(crate) fn dir_suggestions(parent: &Path, partial: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let lower_partial = partial.to_lowercase();
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| fuzzy_subsequence(partial, name))
        .collect();
    names.sort_by(|a, b| {
        let a_pref = a.to_lowercase().starts_with(&lower_partial);
        let b_pref = b.to_lowercase().starts_with(&lower_partial);
        b_pref
            .cmp(&a_pref)
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });
    names
}

/// Contract a leading home-directory prefix to `~` for display (inverse of `shellexpand`).
pub(crate) fn contract_home(path: &Path) -> String {
    if let Some(home) = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()) {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_expands_leading_tilde() {
        let mut f = DirField::new();
        for c in "~/foo".chars() {
            f.input_char(c);
        }
        let resolved = f.resolved();
        assert!(!resolved.to_string_lossy().starts_with('~'));
        // Assert on the final component, not a "/foo" suffix: the separator is
        // "\" on Windows, so a literal "/foo" check fails there.
        assert_eq!(resolved.file_name().and_then(|s| s.to_str()), Some("foo"));
    }

    #[test]
    fn split_path_splits_at_last_slash() {
        assert_eq!(split_path("~/dev/kam"), ("~/dev/", "kam"));
        assert_eq!(split_path("~/dev/"), ("~/dev/", ""));
        assert_eq!(split_path("/abs/path/to/x"), ("/abs/path/to/", "x"));
    }

    #[test]
    fn split_path_with_no_slash_has_empty_parent() {
        assert_eq!(split_path("kam"), ("", "kam"));
        assert_eq!(split_path(""), ("", ""));
    }

    #[test]
    fn fuzzy_subsequence_matches_in_order() {
        assert!(fuzzy_subsequence("km", "kamaji"));
        assert!(fuzzy_subsequence("kam", "kamaji"));
        assert!(!fuzzy_subsequence("mk", "kamaji")); // wrong order
        assert!(!fuzzy_subsequence("xyz", "kamaji"));
    }

    #[test]
    fn fuzzy_subsequence_is_case_insensitive() {
        assert!(fuzzy_subsequence("KM", "kamaji"));
        assert!(fuzzy_subsequence("km", "KamAji"));
    }

    #[test]
    fn fuzzy_subsequence_empty_partial_matches_everything() {
        assert!(fuzzy_subsequence("", "anything"));
        assert!(fuzzy_subsequence("", ""));
    }

    #[test]
    fn dir_suggestions_returns_only_matching_subdirs_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        std::fs::create_dir(base.join("kamaji")).unwrap();
        std::fs::create_dir(base.join("kafka")).unwrap();
        std::fs::create_dir(base.join("zzz")).unwrap();
        std::fs::write(base.join("kamfile.txt"), b"x").unwrap(); // a file, must be excluded

        // partial "ka" matches the two k-dirs (prefix matches first, alphabetical)
        let got = dir_suggestions(base, "ka");
        assert_eq!(got, vec!["kafka".to_string(), "kamaji".to_string()]);

        // empty partial lists all subdirs, prefix group is empty so plain alphabetical
        let all = dir_suggestions(base, "");
        assert_eq!(
            all,
            vec!["kafka".to_string(), "kamaji".to_string(), "zzz".to_string()]
        );
    }

    #[test]
    fn dir_suggestions_orders_prefix_matches_first() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        std::fs::create_dir(base.join("alpha")).unwrap();
        std::fs::create_dir(base.join("banana")).unwrap();
        std::fs::create_dir(base.join("ant")).unwrap();
        // partial "an": "ant" is a prefix match, "banana" only a subsequence match.
        let got = dir_suggestions(base, "an");
        assert_eq!(got, vec!["ant".to_string(), "banana".to_string()]);
    }

    #[test]
    fn dir_suggestions_nonexistent_parent_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(dir_suggestions(&missing, "x").is_empty());
    }

    #[test]
    fn refresh_lists_subdirs_of_parent() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("kamaji")).unwrap();
        std::fs::create_dir(tmp.path().join("other")).unwrap();

        let mut f = DirField::new();
        f.value = format!("{}/kam", tmp.path().display());
        f.refresh();

        assert_eq!(f.suggestions, vec!["kamaji".to_string()]);
        assert_eq!(f.suggestion_idx, 0);
    }

    #[test]
    fn move_suggestion_clamps_at_both_ends() {
        let mut f = DirField::new();
        f.suggestions = vec!["a".into(), "b".into(), "c".into()];
        f.suggestion_idx = 0;

        f.move_suggestion(-1); // already at top
        assert_eq!(f.suggestion_idx, 0);

        f.move_suggestion(1);
        f.move_suggestion(1);
        assert_eq!(f.suggestion_idx, 2);

        f.move_suggestion(1); // already at bottom
        assert_eq!(f.suggestion_idx, 2);
    }

    #[test]
    fn accept_suggestion_replaces_partial_and_appends_slash() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("kamaji")).unwrap();

        let mut f = DirField::new();
        f.value = format!("{}/kam", tmp.path().display());
        f.refresh();
        assert_eq!(f.suggestions, vec!["kamaji".to_string()]);

        f.accept_suggestion();
        assert_eq!(f.value, format!("{}/kamaji/", tmp.path().display()));
    }

    #[test]
    fn accept_suggestion_preserves_tilde_parent() {
        let mut f = DirField::new();
        f.value = "~/dev/kam".into();
        f.suggestions = vec!["kamaji".into()];
        f.suggestion_idx = 0;

        f.accept_suggestion();
        assert!(f.value.starts_with("~/dev/kamaji/"));
    }

    #[test]
    fn accept_suggestion_with_empty_list_is_noop() {
        let mut f = DirField::new();
        f.value = "~/dev/".into();
        f.suggestions.clear();
        f.accept_suggestion();
        assert_eq!(f.value, "~/dev/");
    }

    #[test]
    fn check_root_is_ready_for_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            check_root(dir.path().to_path_buf()),
            RootCheck::Ready(_)
        ));
    }

    #[test]
    fn check_root_needs_confirm_for_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does/not/exist");
        assert!(matches!(check_root(missing), RootCheck::NeedsConfirm(_)));
    }

    #[test]
    fn check_root_is_invalid_for_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(check_root(file), RootCheck::Invalid(_)));
    }

    #[test]
    fn confirm_create_makes_missing_directory_with_parents() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("new/deeply/nested");

        let mut f = DirField::new();
        f.pending_create = Some(nested.clone());

        let created = f.confirm_create().unwrap();
        assert_eq!(created.as_deref(), Some(nested.as_path()));
        assert!(nested.is_dir(), "directory and parents should be created");
        assert!(f.pending_create.is_none(), "pending is consumed");
    }

    #[test]
    fn confirm_create_is_noop_without_pending() {
        let mut f = DirField::new();
        assert!(f.confirm_create().unwrap().is_none());
    }

    #[test]
    fn escape_dismisses_pending_before_closing() {
        let mut f = DirField::new();
        f.pending_create = Some(PathBuf::from("/tmp/whatever"));

        // First Esc only cancels the pending create.
        assert!(!f.escape());
        assert!(f.pending_create.is_none());
        // Next Esc signals close.
        assert!(f.escape());
    }

    #[test]
    fn editing_clears_a_pending_create() {
        let mut f = DirField::new();

        f.pending_create = Some(PathBuf::from("/tmp/a"));
        f.input_char('x');
        assert!(f.pending_create.is_none(), "typing clears pending");

        f.pending_create = Some(PathBuf::from("/tmp/a"));
        f.backspace();
        assert!(f.pending_create.is_none(), "backspace clears pending");
    }

    #[test]
    fn contract_home_abbreviates_home_prefix() {
        let home = directories::BaseDirs::new()
            .map(|b| b.home_dir().to_path_buf())
            .expect("home dir");

        // A path under home is shown with a leading `~`.
        assert_eq!(contract_home(&home.join("dev/kamaji")), "~/dev/kamaji");
        // The home directory itself contracts to a bare `~`.
        assert_eq!(contract_home(&home), "~");
        // Round-trips with shellexpand, the inverse operation.
        assert_eq!(
            shellexpand(&contract_home(&home.join("dev/kamaji"))),
            home.join("dev/kamaji").to_string_lossy()
        );
        // A path outside home is left untouched.
        assert_eq!(contract_home(&PathBuf::from("/opt/foo")), "/opt/foo");
    }
}
