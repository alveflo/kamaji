# PWA installability for the kamaji web board — design

**Date:** 2026-06-06
**Status:** approved, ready for implementation plan
**Scope:** make the browser board (`http://127.0.0.1:8755/`) installable as a
standalone-window Progressive Web App.

---

## Goal

Let a user "Install" the kamaji board from their browser so it runs in its own
standalone window (own taskbar/dock entry, no browser chrome). This is
**installable-window only**: the app still requires the running daemon. There is
**no offline mode and no asset-cache layer** — a service worker exists solely to
satisfy the browser's installability criteria.

Non-goals (explicitly out of scope):

- Offline functionality. The board is driven by live SSE from the daemon
  (`/ui/events`) and an embedded terminal proxied from `:8756`; neither works
  offline, so caching board state is pointless and misleading.
- Precaching / runtime caching of the static shell. Could be added later as a
  follow-up; not part of this work.
- Push notifications, background sync, share targets, or any other PWA API
  beyond installability.

## Context

The board is server-rendered HTML built with `maud` (`views/page.rs`). Static
assets (CSS, the vendored Datastar runtime, JS modules, the Nerd Font woff2) are
compiled into the `kamajid` binary via `rust-embed` (`routes/assets.rs`,
`#[folder = "src/assets/"]`) and served from `GET /assets/*path`. The board
itself is served at `GET /` (`routes::ui::board`, wired in `lib.rs`). There is
**no favicon or logo** anywhere in the repo today.

Browser installability (Chromium) requires all of:

- a linked web app manifest with `name`/`short_name`, a `start_url`, a
  `display` of `standalone`/`fullscreen`/`minimal-ui`, and icons including at
  least 192×192 and 512×512;
- a registered service worker **with a `fetch` event handler** (a no-op
  passthrough handler counts);
- served over a secure context (`localhost`/`127.0.0.1` qualifies).

## Components

### 1. Icon set — a monogram

A master `icon.svg` is the source of truth: a lowercase **"k"** monogram on a
rounded-rect gradient drawn from the existing design tokens (`tokens.css`):
accent `#89b4fa` → background `#16161f` (the Catppuccin palette already in use).

Committed outputs under `crates/kamajid/src/assets/`:

| File | Size | `purpose` | Use |
|------|------|-----------|-----|
| `icon-192.png` | 192×192 | `any` | manifest, min install icon |
| `icon-512.png` | 512×512 | `any` | manifest, splash / hi-dpi |
| `icon-maskable-512.png` | 512×512 | `maskable` | adaptive icon — same art with ~20% safe-zone padding so platform masks don't clip it |
| `apple-touch-icon.png` | 180×180 | — | iOS home-screen icon |
| `favicon.svg` | vector | — | browser tab icon (none exists today) |

**Rasterization mechanism:** the SVG is authored by hand; the PNGs are generated
**once** from it with the `resvg` CLI and committed. `resvg` is installed
ad hoc for generation (`cargo install resvg`) and is **not** added as a runtime
or build dependency of the workspace. The exact generation commands are recorded
in a short comment block at the top of `icon.svg` (and/or a one-line note in
`ARCHITECTURE.md`'s asset section) so the PNGs are reproducible. Result: the
binary stays self-contained with zero new dependencies; the PNGs ride along
through the existing `rust-embed` `Assets` struct and the `/assets/*path` route
unchanged.

### 2. Web manifest — `src/assets/manifest.webmanifest`

```json
{
  "id": "/",
  "name": "kamaji",
  "short_name": "kamaji",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "background_color": "#16161f",
  "theme_color": "#16161f",
  "icons": [
    { "src": "/assets/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
    { "src": "/assets/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
    { "src": "/assets/icon-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
  ]
}
```

All icon `src` values and `start_url`/`scope` are **absolute** so the manifest's
own URL is irrelevant to resolution. `theme_color`/`background_color` match the
app chrome (`--bg` = `#16161f`).

### 3. Service worker — `src/assets/sw.js`

Minimal, but with a real `fetch` handler so the install criteria are met. The
handler is **network passthrough** (`event.respondWith(fetch(event.request))`)
— no caching, consistent with the installable-window-only scope. Includes:

- a `VERSION` constant (bump to force an SW update),
- `self.addEventListener('install', () => self.skipWaiting())`,
- `self.addEventListener('activate', (e) => e.waitUntil(self.clients.claim()))`,

so a daemon restart shipping a new `sw.js` rolls the worker over cleanly without
a manual unregister.

### 4. Serving — new `routes::pwa` module

Two dedicated routes wired into the router in `lib.rs`, both reusing the existing
embedded `Assets` (no second `rust-embed` folder):

- `GET /sw.js` → body of embedded `sw.js`, `Content-Type: text/javascript`,
  plus `Service-Worker-Allowed: /`. **Served at the site root** so its default
  scope (`/`) covers the board page at `/`. (A service worker can only control
  pages at or below its own path.)
- `GET /manifest.webmanifest` → body of embedded `manifest.webmanifest`,
  `Content-Type: application/manifest+json`. Served via a dedicated route
  because `mime_guess` does not reliably map the `.webmanifest` extension to the
  correct type.

The icon PNGs and `favicon.svg` are served by the **existing** `/assets/*path`
handler with no change (their MIME types are handled by `mime_guess`).

These two handlers return bytes from `Assets::get(...)`; on the (impossible
unless the build is broken) miss they return `500`/`404`. They do not need the
ETag/304 machinery of the general asset route — small files, infrequent fetches.

### 5. `<head>` additions — `views/page.rs`

Added to the document head, alongside the existing stylesheet/script links:

```html
<link rel="manifest" href="/manifest.webmanifest">
<meta name="theme-color" content="#16161f">
<link rel="icon" href="/assets/favicon.svg">
<link rel="apple-touch-icon" href="/assets/apple-touch-icon.png">
<meta name="apple-mobile-web-app-capable" content="yes">
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">
<meta name="apple-mobile-web-app-title" content="kamaji">
```

Plus a small inline service-worker registration script, feature-detected:

```html
<script>
  if ('serviceWorker' in navigator) {
    window.addEventListener('load', () =>
      navigator.serviceWorker.register('/sw.js').catch(() => {}));
  }
</script>
```

(Inline keeps it dependency-free and avoids a module fetch on every load; the
`.catch` swallows registration failure so a blocked SW never breaks the board.)

## Data / control flow

1. Browser loads `GET /` → HTML head now references `/manifest.webmanifest` and
   registers `/sw.js`.
2. Browser fetches `/manifest.webmanifest` (typed `application/manifest+json`)
   and the icons from `/assets/*`.
3. The inline script registers `/sw.js` (root scope); the SW installs, claims
   clients, and serves a passthrough `fetch` handler.
4. With manifest + icons + a fetch-handling SW in place over a secure context,
   the browser surfaces its native Install affordance. Installed, the app opens
   at `start_url` `/` in a `standalone` window.

The embedded terminal iframe (proxy origin `:8756`) is a **different origin**
from the PWA scope; it is unaffected and continues to work inside the standalone
window.

## Error handling

- SW registration failure is caught and ignored — the board works regardless.
- Missing embedded asset in a `routes::pwa` handler → `500` (indicates a broken
  build, not a runtime condition).
- No new failure modes are introduced on the daemon side; the routes are static.

## Testing

- `routes::pwa` unit tests:
  - `GET /sw.js` → `200`, `Content-Type: text/javascript`,
    `Service-Worker-Allowed: /`, non-empty body.
  - `GET /manifest.webmanifest` → `200`,
    `Content-Type: application/manifest+json`, non-empty body.
- Manifest content test: parse the embedded `manifest.webmanifest` as JSON;
  assert `name`, `start_url`, a valid `display`, and that the `icons` array
  contains both a 192×192 and a 512×512 entry.
- `views/page.rs` tests (extend existing head tests): head contains
  `rel="manifest"` → `/manifest.webmanifest`, a `theme-color` meta,
  `rel="apple-touch-icon"`, and a `serviceWorker.register('/sw.js')` call.
- Manual verification: open the board in Chromium, confirm the Install icon
  appears and Lighthouse's "Installable" check passes.

## Files touched

| File | Change |
|------|--------|
| `crates/kamajid/src/assets/icon.svg` | new — master monogram (with generation-command comment) |
| `crates/kamajid/src/assets/icon-192.png` | new — generated raster |
| `crates/kamajid/src/assets/icon-512.png` | new — generated raster |
| `crates/kamajid/src/assets/icon-maskable-512.png` | new — generated raster |
| `crates/kamajid/src/assets/apple-touch-icon.png` | new — generated raster |
| `crates/kamajid/src/assets/favicon.svg` | new — tab icon |
| `crates/kamajid/src/assets/manifest.webmanifest` | new — web app manifest |
| `crates/kamajid/src/assets/sw.js` | new — minimal service worker |
| `crates/kamajid/src/routes/pwa.rs` | new — `/sw.js` + `/manifest.webmanifest` handlers + tests |
| `crates/kamajid/src/routes/mod.rs` | add `pub mod pwa;` |
| `crates/kamajid/src/lib.rs` | wire the two routes |
| `crates/kamajid/src/views/page.rs` | head links/meta + SW registration + tests |
| `ARCHITECTURE.md` | brief note on the PWA assets (optional, if it fits the assets section) |
