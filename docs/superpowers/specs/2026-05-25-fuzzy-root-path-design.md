# Fuzzy path completion for the new-project root directory

## Problem

Creating a project (the `n` modal in `src/picker.rs`) requires typing the root
directory path blind into a plain text field. There is no completion or
feedback until you press Enter, at which point the path is validated as a
directory. Typing long paths is error-prone and slow.

## Goal

Add live, shell-style **path-segment completion** to the Root directory field:
as the user types, show the directory entries inside the path's parent that
fuzzy-match the segment being typed, and let the user navigate and accept them
to descend the tree one level at a time.

## Behavior

When the **Root directory** field is active, a suggestion list appears beneath
it. It lists the *directory* entries inside the typed path's parent that
fuzzy-match the segment currently being typed.

- The raw field string is split at the **last `/`** into `parent` + `partial`.
  Examples:
  - `~/dev/kam` → parent `~/dev/`, partial `kam`
  - `~/dev/` → parent `~/dev/`, partial `` (empty → list everything in parent)
  - `kam` (no slash) → parent `` (treated as current dir `.`), partial `kam`
- `parent` is expanded with the existing `shellexpand` (leading `~`) **only to
  read the filesystem**. The field text keeps the literal characters the user
  typed (so `~/...` stays `~/...` on screen).
- Matching is **case-insensitive subsequence** ("fuzzy"): `km` matches
  `kamaji`. Only directories are listed.
- Sort order: entries whose name starts with `partial` (case-insensitive)
  come first, then the rest; alphabetical within each group.
- If `parent` does not exist or cannot be read, the suggestion list is empty.

### Keys

Root field only (Name field keeps today's behavior):

| Key         | Action                                                            |
|-------------|-------------------------------------------------------------------|
| `↑` / `↓`   | Move the highlight within the suggestion list.                    |
| `Tab`       | Accept the highlighted entry: replace `partial` with the entry name + trailing `/`, then refresh suggestions for the new level. |
| `Shift-Tab` | Switch back to the Name field (existing field navigation).        |
| `Enter`     | Create the project (unchanged).                                   |
| `Esc`       | Cancel the modal (unchanged).                                     |
| char / Backspace | Edit the field as today; resets the highlight to the top and refreshes suggestions. |

On the **Name** field, `Tab` still moves to the Root field as today. If the
suggestion list is empty, `Tab` on the Root field does nothing (it does not
move fields — `Shift-Tab` is the way back to Name).

## State

`ProjectForm` (in `src/picker.rs`) gains:

- `suggestions: Vec<String>` — directory names for the current parent/partial.
- `suggestion_idx: usize` — highlighted entry, clamped to `suggestions`.

Suggestions are recomputed:

- after every character input / backspace on the Root field,
- after accepting a completion,
- when the Root field becomes active.

Editing the field resets `suggestion_idx` to 0.

## Components

### Pure helpers (unit-tested)

Implemented in `src/picker.rs` alongside `shellexpand`, kept free of UI/event
state so they can be tested directly (with `tempfile` for the filesystem ones):

- `fn split_root(raw: &str) -> (&str, &str)` — splits at the last `/` into
  `(parent_including_trailing_slash_or_empty, partial)`.
- `fn fuzzy_subsequence(partial: &str, candidate: &str) -> bool` —
  case-insensitive subsequence test. Empty `partial` matches everything.
- `fn dir_suggestions(parent_expanded: &Path, partial: &str) -> Vec<String>` —
  reads `parent_expanded`, keeps subdirectories whose name matches
  `fuzzy_subsequence`, sorts (prefix matches first, then alphabetical), returns
  the names.

### `ProjectForm` methods

- `refresh_suggestions(&mut self)` — splits `self.root`, expands the parent,
  calls `dir_suggestions`, stores the result, clamps `suggestion_idx`.
- `accept_suggestion(&mut self)` — if a suggestion is highlighted, replace
  `partial` in `self.root` with the entry name + `/`, then `refresh_suggestions`.
- `move_suggestion(&mut self, delta: isize)` — move and clamp `suggestion_idx`.

### Rendering

Extend `render_field_modal` in `src/ui/modals.rs` (the picker is its only
caller) to accept an optional suggestion list and a selected index, drawn as a
highlighted list below the fields using the existing accent highlight style
(`fg = base/Black, bg = accent, BOLD`). When the list is empty or the Root
field is not active, nothing extra is drawn.

The picker passes the suggestions only when the Root field is active. The hint
line shown while on the Root field becomes:

```
↑/↓ choose · Tab complete · ↵ create · Esc cancel
```

(The Name field keeps the current hint.)

## Testing

Following TDD. Unit tests (in the existing `#[cfg(test)] mod tests` in
`picker.rs`) cover:

- `split_root`: with/without slash, trailing slash, `~/` prefix, nested path.
- `fuzzy_subsequence`: subsequence hit, miss, case-insensitivity, empty partial.
- `dir_suggestions`: build a temp dir tree with `tempfile`; assert only
  subdirectories are returned, files excluded, matching + sort order correct,
  non-existent parent → empty.
- `accept_suggestion`: replaces the partial segment and appends `/`, preserves
  the literal parent text including a `~/` prefix.
- `move_suggestion`: clamps at both ends.

A render smoke test asserts the suggestion list draws when the Root field is
active and suggestions exist.

## Out of scope (YAGNI)

- Recursive / fzf-style matching across the tree (only one level at a time).
- Matching or completing non-directory files.
- Frequency/recency ranking (zoxide-style).
- Cursor positioning inside the middle of the field (input stays append-only,
  matching the current form behavior).
