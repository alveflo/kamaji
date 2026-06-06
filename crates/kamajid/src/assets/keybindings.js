// kamaji — global board keyboard shortcuts.
//
// Page-level keybindings that complement the per-component handlers in
// `search.js` (`/` focus, `Esc` clear) and the modal's own `Esc`-to-close:
//
//   n       open the "new ticket" form (same as the topbar "+ New" button)
//   Ctrl-Q  close the inline terminal attach panel
//
// `n` is a bare letter, so it is suppressed while typing in a field or while a
// modal is already open (the new-ticket form, an edit form, the terminal). It
// reuses the topbar button instead of re-deriving the project id: a synthetic
// click drives the button's existing Datastar `@get` handler.
//
// `Ctrl-Q` exists because the terminal panel deliberately binds neither Esc nor
// the search shortcuts — the embedded zellij iframe captures the keyboard and
// those are vital terminal keys (see views/terminal.rs). It only closes the
// panel when one is open. NOTE: a cross-origin iframe (the board is :8755, the
// proxy iframe is :8756) keeps keydown events to itself while it holds focus,
// so this fires only when focus is on the board chrome (e.g. the panel's title
// bar) rather than trapped inside the live terminal; the ✕ remains the always-
// works close. We still preventDefault so we don't trip the browser's own
// Ctrl-Q when we do handle it.

(() => {
  const modalMount = document.getElementById('modal');
  const modalOpen = () => modalMount && modalMount.childElementCount > 0;
  // The terminal attach panel roots a `.term-modal` inside the mount.
  const terminalOpen = () =>
    modalMount && modalMount.querySelector('.term-modal') !== null;

  function isTyping() {
    const ae = document.activeElement;
    return !!(
      ae &&
      (ae.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(ae.tagName))
    );
  }

  document.addEventListener('keydown', (e) => {
    // Ctrl-Q: close the terminal attach panel (if one is open).
    if (
      e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      e.key &&
      e.key.toLowerCase() === 'q'
    ) {
      if (terminalOpen()) {
        e.preventDefault();
        e.stopPropagation();
        modalMount.replaceChildren();
      }
      return;
    }

    // n: open the new-ticket form — but never while typing, while a modal is
    // already open, or as part of a modifier chord (Ctrl-N, ⌘N, …).
    if (
      e.key === 'n' &&
      !e.ctrlKey &&
      !e.metaKey &&
      !e.altKey &&
      !isTyping() &&
      !modalOpen()
    ) {
      const btn = document.querySelector('.new-ticket');
      if (btn) {
        e.preventDefault();
        btn.click();
      }
    }
  });
})();
