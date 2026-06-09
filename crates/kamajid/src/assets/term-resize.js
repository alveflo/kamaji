// kamaji — make the terminal attach panel a resizable, movable window.
//
// The panel (`.term-modal` inside `#modal`) embeds a CROSS-ORIGIN zellij iframe
// (the board is :8755, the proxy iframe is :8756). zellij web watches its own
// size and reflows the terminal whenever the iframe element changes size, so we
// only have to resize the `.term-modal` box — the embedded session follows. The
// proxy stays untouched.
//
// Why not CSS `resize: both`? The moment the cursor crosses the cross-origin
// iframe mid-drag, the iframe captures pointer events and the drag stalls. So we
// drive geometry ourselves from pointer events on explicit edge/corner grips
// (`.term-rh[data-dir]`) and the title bar (`.term-bar` — drag to move), and lay
// a full-viewport overlay over the iframe for the duration of a drag so
// pointermove keeps flowing to us.
//
// Geometry (left/top/width/height) is persisted to localStorage so a resized
// window comes back the same on the next attach. The title-bar maximize button
// (and double-clicking the title bar) toggles maximize (near-fullscreen) /
// restore; the pre-maximize geometry is remembered so restore returns there.
//
// The panel is morphed into `#modal` by Datastar SSE, so we watch the mount with
// a MutationObserver and wire each `.term-modal` as it appears.

(() => {
  const KEY = 'kamaji.term.geom';
  const RKEY = 'kamaji.term.restore'; // pre-maximize geometry to restore to
  const MIN_W = 360; // keep in sync with .term-modal min-width
  const MIN_H = 220; // keep in sync with .term-modal min-height
  const MARGIN = 16; // matches the CSS default top/left inset
  const MAX_GLYPH = '⤢';
  const RESTORE_GLYPH = '⧉';

  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

  // Near-fullscreen geometry — the CSS default, used by the maximize toggle.
  function maximized() {
    return {
      left: MARGIN,
      top: MARGIN,
      width: Math.max(MIN_W, window.innerWidth - MARGIN * 2),
      height: Math.max(MIN_H, window.innerHeight - MARGIN * 2),
    };
  }

  function loadGeom() {
    try {
      const g = JSON.parse(localStorage.getItem(KEY));
      if (g && typeof g.width === 'number' && typeof g.height === 'number') {
        return g;
      }
    } catch (_) {}
    return null;
  }

  function saveGeom(g) {
    try {
      localStorage.setItem(KEY, JSON.stringify(g));
    } catch (_) {}
  }

  function loadRestore() {
    try {
      const g = JSON.parse(localStorage.getItem(RKEY));
      if (g && typeof g.width === 'number' && typeof g.height === 'number') {
        return g;
      }
    } catch (_) {}
    return null;
  }

  function saveRestore(g) {
    try {
      localStorage.setItem(RKEY, JSON.stringify(g));
    } catch (_) {}
  }

  // Where restore lands when there's no remembered pre-maximize geometry (e.g.
  // the window opened near-fullscreen and the user hit restore first): a
  // centered ~70% box.
  function defaultRestore() {
    const width = Math.max(MIN_W, Math.round(window.innerWidth * 0.7));
    const height = Math.max(MIN_H, Math.round(window.innerHeight * 0.7));
    return {
      left: Math.round((window.innerWidth - width) / 2),
      top: Math.round((window.innerHeight - height) / 2),
      width,
      height,
    };
  }

  // Apply a geometry to the modal as inline styles, clamped to the current
  // viewport so a window saved on a larger screen stays on-screen and reachable.
  // Returns the clamped geometry actually applied.
  function applyGeom(modal, g) {
    const maxW = window.innerWidth;
    const maxH = window.innerHeight;
    const width = clamp(g.width, MIN_W, maxW);
    const height = clamp(g.height, MIN_H, maxH);
    const left = clamp(g.left, 0, Math.max(0, maxW - width));
    const top = clamp(g.top, 0, Math.max(0, maxH - height));
    modal.style.left = left + 'px';
    modal.style.top = top + 'px';
    modal.style.width = width + 'px';
    modal.style.height = height + 'px';
    return { left, top, width, height };
  }

  function rectOf(modal) {
    const r = modal.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height };
  }

  // --- maximize / restore ----------------------------------------------------
  // The window is "maximized" when its geometry matches maximized() within a
  // couple of pixels (the default open state already is — see the CSS).
  function isMaximized(modal) {
    const r = rectOf(modal);
    const m = maximized();
    const near = (a, b) => Math.abs(a - b) <= 2;
    return (
      near(r.left, m.left) &&
      near(r.top, m.top) &&
      near(r.width, m.width) &&
      near(r.height, m.height)
    );
  }

  // Reflect the current state on the maximize button: a maximize glyph when the
  // window can grow, a restore glyph when it's already full.
  function syncMaxButton(modal) {
    const btn = modal.querySelector('.term-max');
    if (!btn) return;
    const max = isMaximized(modal);
    btn.textContent = max ? RESTORE_GLYPH : MAX_GLYPH;
    btn.setAttribute('aria-label', max ? 'Restore terminal' : 'Maximize terminal');
    btn.title = max ? 'Restore' : 'Maximize';
  }

  // Maximize if not already full; otherwise restore the remembered pre-maximize
  // geometry (falling back to a centered box). Persists both sides.
  function toggleMaximize(modal) {
    if (isMaximized(modal)) {
      saveGeom(applyGeom(modal, loadRestore() || defaultRestore()));
    } else {
      saveRestore(rectOf(modal));
      saveGeom(applyGeom(modal, maximized()));
    }
    syncMaxButton(modal);
  }

  // --- drag shield -----------------------------------------------------------
  let overlay = null;
  function showOverlay(cursor) {
    overlay = document.createElement('div');
    overlay.className = 'term-drag-overlay';
    overlay.style.cursor = cursor;
    document.body.appendChild(overlay);
  }
  function hideOverlay() {
    if (overlay) {
      overlay.remove();
      overlay = null;
    }
  }

  // Run `onMove` for each pointermove until pointerup, then clean up and persist.
  function dragLoop(onMove, modal) {
    const onUp = () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      modal.classList.remove('term-resizing');
      hideOverlay();
      if (modal._rzGeom) saveGeom(modal._rzGeom);
      // A drag may have moved/sized the window in or out of "maximized".
      syncMaxButton(modal);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
  }

  // --- move (title-bar drag) -------------------------------------------------
  function startMove(e, modal) {
    if (e.button !== 0) return;
    // Let the title-bar buttons do their own job instead of starting a move.
    if (e.target.closest('.term-close, .term-max')) return;
    e.preventDefault();
    const start = rectOf(modal);
    const px = e.clientX;
    const py = e.clientY;
    modal.classList.add('term-resizing');
    showOverlay('move');

    const onMove = (ev) => {
      modal._rzGeom = applyGeom(modal, {
        left: start.left + (ev.clientX - px),
        top: start.top + (ev.clientY - py),
        width: start.width,
        height: start.height,
      });
    };
    dragLoop(onMove, modal);
  }

  // --- resize (edge/corner grip drag) ----------------------------------------
  function startResize(e, modal, dir) {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const start = rectOf(modal);
    const px = e.clientX;
    const py = e.clientY;
    modal.classList.add('term-resizing');
    showOverlay(getComputedStyle(e.target).cursor || 'default');

    const onMove = (ev) => {
      const dx = ev.clientX - px;
      const dy = ev.clientY - py;
      let { left, top, width, height } = start;
      if (dir.includes('e')) width = start.width + dx;
      if (dir.includes('s')) height = start.height + dy;
      if (dir.includes('w')) {
        width = start.width - dx;
        left = start.left + dx;
      }
      if (dir.includes('n')) {
        height = start.height - dy;
        top = start.top + dy;
      }
      // When shrinking past the minimum from a west/north edge, pin the moving
      // edge so the opposite (anchored) edge stays put.
      if (width < MIN_W) {
        if (dir.includes('w')) left = start.left + (start.width - MIN_W);
        width = MIN_W;
      }
      if (height < MIN_H) {
        if (dir.includes('n')) top = start.top + (start.height - MIN_H);
        height = MIN_H;
      }
      modal._rzGeom = applyGeom(modal, { left, top, width, height });
    };
    dragLoop(onMove, modal);
  }

  // --- wiring ----------------------------------------------------------------
  function init(modal) {
    if (modal.dataset.rzInit) return;
    modal.dataset.rzInit = '1';

    const saved = loadGeom();
    if (saved) saveGeom(applyGeom(modal, saved));

    modal.querySelectorAll('.term-rh').forEach((h) => {
      h.addEventListener('pointerdown', (e) => startResize(e, modal, h.dataset.dir));
    });

    const maxBtn = modal.querySelector('.term-max');
    if (maxBtn) {
      maxBtn.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        toggleMaximize(modal);
      });
    }

    const bar = modal.querySelector('.term-bar');
    if (bar) {
      bar.addEventListener('pointerdown', (e) => startMove(e, modal));
      bar.addEventListener('dblclick', (e) => {
        if (e.target.closest('.term-close, .term-max')) return;
        toggleMaximize(modal);
      });
    }

    // Match the button's glyph to the geometry just applied (a persisted
    // geometry may have opened the window maximized, or not).
    syncMaxButton(modal);
  }

  const mount = document.getElementById('modal');
  if (mount) {
    const scan = () => {
      const modal = mount.querySelector('.term-modal');
      if (modal) init(modal);
    };
    new MutationObserver(scan).observe(mount, { childList: true, subtree: true });
    scan();
  }
})();
