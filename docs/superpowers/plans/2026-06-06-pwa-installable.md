# PWA Installability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the kamaji web board (`http://127.0.0.1:8755/`) installable as a standalone-window Progressive Web App.

**Architecture:** Add a hand-authored monogram icon set (SVG sources + committed PNGs), a web app manifest, and a minimal service worker. All ship as embedded assets through the existing `rust-embed` `Assets` struct. Two new dedicated routes serve `/sw.js` (root scope) and `/manifest.webmanifest` (correct MIME). The HTML `<head>` gains the manifest link, icon/theme meta, and a feature-detected service-worker registration. No offline mode, no asset caching — the service worker exists only to satisfy the browser's installability criteria.

**Tech Stack:** Rust, axum 0.7, maud 0.26, rust-embed 8, serde_json (all already in `crates/kamajid/Cargo.toml`). PNG generation uses the `resvg` CLI ad hoc (not a workspace dependency).

**Design spec:** `docs/superpowers/specs/2026-06-06-pwa-installable-design.md`

---

## File Structure

| File | Responsibility |
|------|----------------|
| `crates/kamajid/src/assets/icon.svg` | Master monogram (rounded tile), source of truth for `icon-192.png` / `icon-512.png` |
| `crates/kamajid/src/assets/icon-maskable.svg` | Full-bleed monogram (safe-zone padded), source for the maskable + apple-touch PNGs |
| `crates/kamajid/src/assets/favicon.svg` | Browser tab icon |
| `crates/kamajid/src/assets/icon-192.png` | Generated 192×192 raster (`purpose: any`) |
| `crates/kamajid/src/assets/icon-512.png` | Generated 512×512 raster (`purpose: any`) |
| `crates/kamajid/src/assets/icon-maskable-512.png` | Generated 512×512 raster (`purpose: maskable`) |
| `crates/kamajid/src/assets/apple-touch-icon.png` | Generated 180×180 raster (iOS) |
| `crates/kamajid/src/assets/manifest.webmanifest` | Web app manifest (JSON) |
| `crates/kamajid/src/assets/sw.js` | Minimal service worker (passthrough fetch handler) |
| `crates/kamajid/src/routes/pwa.rs` | `manifest()` + `service_worker()` handlers, reusing embedded `Assets`; unit tests |
| `crates/kamajid/src/routes/assets.rs` | Make `Assets` `pub(crate)` so `pwa` can reuse it; icon-embed presence test |
| `crates/kamajid/src/routes/mod.rs` | `pub mod pwa;` |
| `crates/kamajid/src/lib.rs` | Wire `/sw.js` and `/manifest.webmanifest` routes |
| `crates/kamajid/src/views/page.rs` | `<head>` manifest/icon/theme links + SW registration script; tests |
| `ARCHITECTURE.md` | One-line note on the PWA asset set |

---

## Task 1: Icon assets (SVG sources + generated PNGs)

**Files:**
- Create: `crates/kamajid/src/assets/icon.svg`
- Create: `crates/kamajid/src/assets/icon-maskable.svg`
- Create: `crates/kamajid/src/assets/favicon.svg`
- Create (generated): `crates/kamajid/src/assets/icon-192.png`, `icon-512.png`, `icon-maskable-512.png`, `apple-touch-icon.png`
- Test: `crates/kamajid/src/routes/assets.rs` (add to existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write `icon.svg` (rounded master)**

Create `crates/kamajid/src/assets/icon.svg`. A lowercase "k" monogram: white strokes (round caps/joins) over a rounded-rect gradient from accent `#89b4fa` to background `#16161f`. The leading comment records the exact PNG generation commands so the rasters are reproducible.

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <!-- kamaji app icon (master). Source of truth for the "any" PNGs.
       Regenerate rasters with the resvg CLI (cargo install resvg):
         resvg --width 192 icon.svg          icon-192.png
         resvg --width 512 icon.svg          icon-512.png
         resvg --width 512 icon-maskable.svg icon-maskable-512.png
         resvg --width 180 icon-maskable.svg apple-touch-icon.png
       (Flag names can vary by resvg version; run `resvg --help` if needed.) -->
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="512" y2="512" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#89b4fa"/>
      <stop offset="1" stop-color="#16161f"/>
    </linearGradient>
  </defs>
  <rect width="512" height="512" rx="112" ry="112" fill="url(#g)"/>
  <g fill="none" stroke="#ffffff" stroke-width="48" stroke-linecap="round" stroke-linejoin="round">
    <path d="M188 120 V 392"/>
    <path d="M340 168 L 208 282 L 350 392"/>
  </g>
</svg>
```

- [ ] **Step 2: Write `icon-maskable.svg` (full-bleed, safe-zone padded)**

Create `crates/kamajid/src/assets/icon-maskable.svg`. Same gradient and monogram, but the background fills the whole canvas (no corner radius) and the monogram is scaled down to ~62% and centred, so platform masks never clip it. The `<g transform>` scales the same artwork about the centre.

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <!-- kamaji maskable / apple-touch icon. Full-bleed background, monogram inside
       the maskable safe zone. Generated via the commands in icon.svg. -->
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="512" y2="512" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#89b4fa"/>
      <stop offset="1" stop-color="#16161f"/>
    </linearGradient>
  </defs>
  <rect width="512" height="512" fill="url(#g)"/>
  <g transform="translate(256 256) scale(0.62) translate(-256 -256)"
     fill="none" stroke="#ffffff" stroke-width="48" stroke-linecap="round" stroke-linejoin="round">
    <path d="M188 120 V 392"/>
    <path d="M340 168 L 208 282 L 350 392"/>
  </g>
</svg>
```

- [ ] **Step 3: Write `favicon.svg`**

Create `crates/kamajid/src/assets/favicon.svg` — identical artwork to the master (a standalone file so the head can reference it directly).

```xml
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="512" height="512">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="512" y2="512" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#89b4fa"/>
      <stop offset="1" stop-color="#16161f"/>
    </linearGradient>
  </defs>
  <rect width="512" height="512" rx="112" ry="112" fill="url(#g)"/>
  <g fill="none" stroke="#ffffff" stroke-width="48" stroke-linecap="round" stroke-linejoin="round">
    <path d="M188 120 V 392"/>
    <path d="M340 168 L 208 282 L 350 392"/>
  </g>
</svg>
```

- [ ] **Step 4: Generate the PNGs**

Install the rasterizer if absent, then generate from the SVG sources. Run from `crates/kamajid/src/assets/`:

```bash
cargo install resvg            # one-time, only if `resvg` is not on PATH
cd crates/kamajid/src/assets
resvg --width 192 icon.svg          icon-192.png
resvg --width 512 icon.svg          icon-512.png
resvg --width 512 icon-maskable.svg icon-maskable-512.png
resvg --width 180 icon-maskable.svg apple-touch-icon.png
```

Expected: four PNG files created. Verify dimensions:

```bash
file icon-192.png icon-512.png icon-maskable-512.png apple-touch-icon.png
```

Expected output mentions `192 x 192`, `512 x 512`, `512 x 512`, `180 x 180` respectively.

> If `resvg` is unavailable and cannot be installed, any SVG→PNG rasterizer at the same dimensions works (e.g. `rsvg-convert -w 192 icon.svg -o icon-192.png`). The PNGs must be committed binaries; do not skip them.

- [ ] **Step 5: Write the failing icon-embed test**

In `crates/kamajid/src/routes/assets.rs`, add to the existing `#[cfg(test)] mod tests` module:

```rust
#[test]
fn pwa_icon_assets_are_embedded() {
    // PNGs embed and start with the PNG magic number.
    for png in [
        "icon-192.png",
        "icon-512.png",
        "icon-maskable-512.png",
        "apple-touch-icon.png",
    ] {
        let file = Assets::get(png).unwrap_or_else(|| panic!("{png} not embedded"));
        assert!(
            file.data.starts_with(b"\x89PNG\r\n\x1a\n"),
            "{png} is not a valid PNG"
        );
    }
    // SVG sources embed and look like SVG.
    for svg in ["icon.svg", "icon-maskable.svg", "favicon.svg"] {
        let file = Assets::get(svg).unwrap_or_else(|| panic!("{svg} not embedded"));
        assert!(
            std::str::from_utf8(&file.data).unwrap().contains("<svg"),
            "{svg} is not SVG"
        );
    }
}
```

- [ ] **Step 6: Run the test to verify it passes**

`Assets` already exists in `assets.rs`, and the files exist on disk, so this passes once the assets are present.

Run: `cargo test -p kamajid --lib routes::assets::tests::pwa_icon_assets_are_embedded`
Expected: PASS. (If it fails with "not embedded", the PNG generation in Step 4 was skipped or wrote to the wrong directory.)

- [ ] **Step 7: Commit**

```bash
git add crates/kamajid/src/assets/icon.svg crates/kamajid/src/assets/icon-maskable.svg \
        crates/kamajid/src/assets/favicon.svg crates/kamajid/src/assets/icon-192.png \
        crates/kamajid/src/assets/icon-512.png crates/kamajid/src/assets/icon-maskable-512.png \
        crates/kamajid/src/assets/apple-touch-icon.png crates/kamajid/src/routes/assets.rs
git commit -m "feat(web): add PWA monogram icon set"
```

---

## Task 2: Web manifest + manifest route

**Files:**
- Create: `crates/kamajid/src/assets/manifest.webmanifest`
- Create: `crates/kamajid/src/routes/pwa.rs`
- Modify: `crates/kamajid/src/routes/mod.rs` (add `pub mod pwa;`)
- Modify: `crates/kamajid/src/routes/assets.rs` (make `Assets` `pub(crate)`)
- Modify: `crates/kamajid/src/lib.rs` (wire `/manifest.webmanifest`)
- Test: `crates/kamajid/src/routes/pwa.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write `manifest.webmanifest`**

Create `crates/kamajid/src/assets/manifest.webmanifest`. Absolute paths so the manifest's own URL is irrelevant.

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

- [ ] **Step 2: Make `Assets` reusable**

In `crates/kamajid/src/routes/assets.rs`, change the struct visibility so the `pwa` module can reuse the same embedded folder (no second `rust-embed` copy):

```rust
#[derive(RustEmbed)]
#[folder = "src/assets/"]
pub(crate) struct Assets;
```

(Was `struct Assets;`.)

- [ ] **Step 3: Write the `pwa` module with the manifest handler + failing test**

Create `crates/kamajid/src/routes/pwa.rs`:

```rust
//! PWA plumbing: serve the web app manifest and service worker from dedicated
//! routes. Both reuse the embedded `Assets` folder (see `routes::assets`).
//!
//! The manifest needs an explicit `application/manifest+json` content type
//! (`mime_guess` does not map `.webmanifest`). The service worker MUST be served
//! at the site root (`/sw.js`) so its default scope (`/`) covers the board page.

use axum::http::{header, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::routes::assets::Assets;

/// `GET /manifest.webmanifest` → the embedded manifest, correctly typed.
pub async fn manifest() -> Response {
    match Assets::get("manifest.webmanifest") {
        Some(file) => (
            [(header::CONTENT_TYPE, "application/manifest+json")],
            file.data,
        )
            .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "manifest missing").into_response(),
    }
}

/// `GET /sw.js` → the embedded service worker. Served at root for `/` scope.
pub async fn service_worker() -> Response {
    match Assets::get("sw.js") {
        Some(file) => (
            [
                (header::CONTENT_TYPE, HeaderName::from_static("content-type")),
            ],
            // placeholder replaced in Task 3
            file.data,
        )
            .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "sw missing").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_bytes(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()
    }

    fn content_type(resp: &Response) -> String {
        resp.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn manifest_route_serves_manifest_json() {
        let resp = manifest().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp), "application/manifest+json");

        let body = body_bytes(resp).await;
        let v: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(v["name"], "kamaji");
        assert_eq!(v["start_url"], "/");
        assert!(
            ["standalone", "fullscreen", "minimal-ui"]
                .contains(&v["display"].as_str().unwrap()),
            "display must be an installable mode"
        );
        let sizes: Vec<&str> = v["icons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["sizes"].as_str().unwrap())
            .collect();
        assert!(sizes.contains(&"192x192"), "192 icon present");
        assert!(sizes.contains(&"512x512"), "512 icon present");
    }
}
```

> Note: the `service_worker` body above is a temporary stub so the module compiles for this task's test; Task 3 replaces it with the real headers. The `manifest` handler is final.

- [ ] **Step 4: Register the module and route**

In `crates/kamajid/src/routes/mod.rs`, add (keep alphabetical):

```rust
pub mod pwa;
```

In `crates/kamajid/src/lib.rs`, add a route inside the `Router::new()` chain (next to the `/assets/*path` line, before `.layer(...)`):

```rust
        .route("/manifest.webmanifest", get(routes::pwa::manifest))
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p kamajid --lib routes::pwa::tests::manifest_route_serves_manifest_json`
Expected: PASS.

- [ ] **Step 6: Confirm the crate still builds**

Run: `cargo build -p kamajid`
Expected: builds clean (the `service_worker` stub compiles even though it is not yet wired to a route).

- [ ] **Step 7: Commit**

```bash
git add crates/kamajid/src/assets/manifest.webmanifest crates/kamajid/src/routes/pwa.rs \
        crates/kamajid/src/routes/mod.rs crates/kamajid/src/routes/assets.rs crates/kamajid/src/lib.rs
git commit -m "feat(web): serve PWA web app manifest"
```

---

## Task 3: Service worker + `/sw.js` route

**Files:**
- Create: `crates/kamajid/src/assets/sw.js`
- Modify: `crates/kamajid/src/routes/pwa.rs` (finalize `service_worker`, add tests)
- Modify: `crates/kamajid/src/lib.rs` (wire `/sw.js`)
- Test: `crates/kamajid/src/routes/pwa.rs`

- [ ] **Step 1: Write `sw.js`**

Create `crates/kamajid/src/assets/sw.js`. Minimal but with a real `fetch` handler (required for installability); network passthrough, no caching. `skipWaiting` + `clients.claim` so a new worker rolls over cleanly.

```javascript
// kamaji service worker — installability only. No caching: the board needs the
// live daemon (SSE + terminal proxy), so offline use is intentionally unsupported.
// Bump VERSION to force an update when this file changes.
const VERSION = "1";

self.addEventListener("install", () => self.skipWaiting());

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

// A fetch handler is required for the browser to treat the app as installable.
// Pure passthrough to the network.
self.addEventListener("fetch", (event) => {
  event.respondWith(fetch(event.request));
});
```

- [ ] **Step 2: Finalize the `service_worker` handler**

In `crates/kamajid/src/routes/pwa.rs`, replace the stub `service_worker` body with the real headers (`text/javascript` + `Service-Worker-Allowed: /`):

```rust
/// `GET /sw.js` → the embedded service worker. Served at root for `/` scope.
pub async fn service_worker() -> Response {
    match Assets::get("sw.js") {
        Some(file) => (
            [
                (header::CONTENT_TYPE, "text/javascript"),
                (HeaderName::from_static("service-worker-allowed"), "/"),
            ],
            file.data,
        )
            .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "sw missing").into_response(),
    }
}
```

- [ ] **Step 3: Add the failing service-worker route test**

In the `#[cfg(test)] mod tests` of `pwa.rs`, add:

```rust
    #[tokio::test]
    async fn service_worker_route_is_typed_and_root_scoped() {
        let resp = service_worker().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(content_type(&resp), "text/javascript");
        assert_eq!(
            resp.headers()
                .get("service-worker-allowed")
                .and_then(|v| v.to_str().ok()),
            Some("/"),
            "root scope so the SW controls the board at /"
        );
        let body = String::from_utf8(body_bytes(resp).await).unwrap();
        assert!(body.contains("addEventListener(\"fetch\""), "has fetch handler");
    }
```

- [ ] **Step 4: Wire the `/sw.js` route**

In `crates/kamajid/src/lib.rs`, next to the `/manifest.webmanifest` route added in Task 2:

```rust
        .route("/sw.js", get(routes::pwa::service_worker))
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kamajid --lib routes::pwa`
Expected: PASS (both `manifest_route_serves_manifest_json` and `service_worker_route_is_typed_and_root_scoped`).

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/src/assets/sw.js crates/kamajid/src/routes/pwa.rs crates/kamajid/src/lib.rs
git commit -m "feat(web): serve minimal PWA service worker at /sw.js"
```

---

## Task 4: HTML head — manifest link, icons, theme, SW registration

**Files:**
- Modify: `crates/kamajid/src/views/page.rs` (head markup + tests)

- [ ] **Step 1: Add the failing head test**

In `crates/kamajid/src/views/page.rs`, add to `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn page_head_has_pwa_wiring() {
        let p = project(1, "acme");
        let html = page(&p, std::slice::from_ref(&p), &empty_board()).into_string();
        assert!(
            html.contains(r#"rel="manifest" href="/manifest.webmanifest""#),
            "manifest link:\n{html}"
        );
        assert!(
            html.contains(r#"name="theme-color" content="#16161f""#),
            "theme-color meta:\n{html}"
        );
        assert!(
            html.contains(r#"rel="icon" href="/assets/favicon.svg""#),
            "favicon link:\n{html}"
        );
        assert!(
            html.contains(r#"rel="apple-touch-icon" href="/assets/apple-touch-icon.png""#),
            "apple-touch-icon link:\n{html}"
        );
        assert!(
            html.contains("serviceWorker.register('/sw.js')"),
            "sw registration:\n{html}"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kamajid --lib views::page::tests::page_head_has_pwa_wiring`
Expected: FAIL (the head markup does not exist yet).

- [ ] **Step 3: Add the head markup**

In `crates/kamajid/src/views/page.rs`, inside the `head { ... }` block, add the PWA tags after the existing `title { ... }` line and before the stylesheet links:

```rust
                title { "kamaji — " (project.name) }
                link rel="manifest" href="/manifest.webmanifest";
                meta name="theme-color" content="#16161f";
                link rel="icon" href="/assets/favicon.svg";
                link rel="apple-touch-icon" href="/assets/apple-touch-icon.png";
                meta name="apple-mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-status-bar-style" content="black-translucent";
                meta name="apple-mobile-web-app-title" content="kamaji";
```

Then add the registration script at the end of the `head { ... }` block, after the existing `script type="module" src="/assets/keybindings.js" {}` line:

```rust
                script type="module" src="/assets/keybindings.js" {}
                script {
                    (PreEscaped("if ('serviceWorker' in navigator) { window.addEventListener('load', function () { navigator.serviceWorker.register('/sw.js').catch(function () {}); }); }"))
                }
```

`PreEscaped` is already imported in this file (`use maud::{html, Markup, PreEscaped, DOCTYPE};`).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kamajid --lib views::page::tests::page_head_has_pwa_wiring`
Expected: PASS.

- [ ] **Step 5: Run the full page test module (no regressions)**

Run: `cargo test -p kamajid --lib views::page`
Expected: PASS (all existing head tests still green).

- [ ] **Step 6: Commit**

```bash
git add crates/kamajid/src/views/page.rs
git commit -m "feat(web): wire PWA manifest, icons, and service worker into the page head"
```

---

## Task 5: Docs note + full verification

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add a brief note to ARCHITECTURE.md**

Find the section describing the embedded assets / `kamajid` browser board (search for `rust-embed` or `/assets`). Add one sentence noting the PWA asset set, for example:

```markdown
The board is an installable PWA: `manifest.webmanifest` (served at
`/manifest.webmanifest`) and a minimal service worker (`/sw.js`, root scope, no
caching) make the browser offer "Install"; the monogram icons live in
`crates/kamajid/src/assets/` (regenerate the PNGs from the SVGs with the `resvg`
commands noted in `icon.svg`).
```

Place it adjacent to the existing assets description; match surrounding prose style.

- [ ] **Step 2: Run the whole crate test suite**

Run: `cargo test -p kamajid`
Expected: all tests pass.

- [ ] **Step 3: Format and lint**

Run:
```bash
cargo fmt --all
cargo clippy -p kamajid --all-targets -- -D warnings
```
Expected: no diff from fmt, no clippy warnings. (Per repo memory: this is a binary crate — do not pass `--lib` to clippy expecting a lib-only run; `--all-targets` is fine.)

- [ ] **Step 4: Manual smoke test (installability)**

```bash
make restart
make status   # expect healthy on :8755
```

Then in Chromium, open `http://127.0.0.1:8755/` and confirm:
- DevTools → Application → Manifest shows name "kamaji", the three icons, `display: standalone`, no errors.
- DevTools → Application → Service Workers shows `/sw.js` activated.
- The address bar shows the Install affordance (or Lighthouse → "Installable" passes).
- Installing opens the board in its own standalone window; the board loads, the SSE stream connects, and opening a ticket terminal still works inside that window.

- [ ] **Step 5: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: note PWA installability in ARCHITECTURE"
```

---

## Self-Review notes

- **Spec coverage:** icons (Task 1); manifest + route (Task 2); service worker + route (Task 3); head links/meta/registration (Task 4); `routes::pwa` reuse of `Assets` via `pub(crate)` (Task 2); tests for routes/manifest/head (Tasks 1–4); docs + manual installability check (Task 5). All spec sections map to a task.
- **Type/name consistency:** handlers `routes::pwa::manifest` and `routes::pwa::service_worker` are referenced identically in `lib.rs` and tests; `Assets::get(...)` keys match the on-disk filenames (`manifest.webmanifest`, `sw.js`, the icon files); the SVG `<path>` data is identical across `icon.svg`, `icon-maskable.svg`, and `favicon.svg`.
- **Sequencing:** Task 2 introduces a deliberately stubbed `service_worker` so the module compiles before Task 3 finalizes it and wires `/sw.js`; this is called out inline to avoid confusion if tasks are read out of order.
```
