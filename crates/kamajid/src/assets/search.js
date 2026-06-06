// kamaji — live board search/filter (Slice 4).
//
// Pure client-side: the board is already server-rendered, so filtering just
// hides non-matching cards across all columns. Match scope is title + `#id` +
// agent (NOT action-button labels). The matched substring of the title is
// highlighted with <mark>; column counts recount to matches; a "N results"
// total shows beside the box; columns that have cards but no match show a
// per-column "No matches". `/` focuses the box, `Esc` clears it.
//
// The board is re-rendered piecemeal by SSE patches (`#col-*` / `#card-*`
// morphs), which wipe the marks / hidden state / "No matches" lines. A
// MutationObserver re-applies the active filter after such patches; it is
// disconnected while we write our own DOM mutations so it never self-triggers.

(() => {
  const input = document.getElementById('search');
  if (!input) return;
  const wrap = input.closest('.search-wrap');
  const totalEl = document.querySelector('.search-count');
  const board = document.getElementById('board');

  const esc = (s) =>
    s.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));

  // Find (or lazily create) the "No matches" line at the end of a column body.
  function noMatchLine(body) {
    let p = body.querySelector(':scope > .col-nomatch');
    if (!p) {
      p = document.createElement('p');
      p.className = 'col-nomatch';
      p.textContent = 'No matches';
      body.appendChild(p);
    }
    return p;
  }

  function apply() {
    const raw = input.value;
    const q = raw.trim().toLowerCase();
    if (wrap) wrap.classList.toggle('has-q', raw.length > 0);

    let total = 0;
    document.querySelectorAll('.column').forEach((col) => {
      const cards = col.querySelectorAll('.card');
      let n = 0;
      cards.forEach((card) => {
        const titleEl = card.querySelector('.card-title');
        // textContent flattens any existing <mark>, so this is the original title.
        const orig = titleEl ? titleEl.textContent : '';
        const idEl = card.querySelector('.card-id');
        const agentEl = card.querySelector('.agent');
        const idText = idEl ? idEl.textContent : '';
        const agent = agentEl ? agentEl.textContent : '';
        const hay = `${idText} ${orig} ${agent}`.toLowerCase();
        const match = q === '' || hay.includes(q);
        card.style.display = match ? '' : 'none';
        if (titleEl) {
          const i = match && q ? orig.toLowerCase().indexOf(q) : -1;
          if (i < 0) {
            titleEl.textContent = orig;
          } else {
            titleEl.innerHTML =
              esc(orig.slice(0, i)) +
              '<mark>' +
              esc(orig.slice(i, i + q.length)) +
              '</mark>' +
              esc(orig.slice(i + q.length));
          }
        }
        if (match) n++;
      });
      total += n;

      const countEl = col.querySelector('.col-count');
      if (countEl) countEl.textContent = q === '' ? cards.length : n;

      const body = col.querySelector('.col-body');
      // Only "No matches" when the column has cards but none survive the filter;
      // a genuinely empty column keeps its own "Nothing here" placeholder.
      if (body) {
        noMatchLine(body).style.display =
          q !== '' && cards.length > 0 && n === 0 ? '' : 'none';
      }
    });

    if (totalEl) {
      totalEl.textContent = q === '' ? '' : `${total} result${total === 1 ? '' : 's'}`;
    }
  }

  let observer = null;
  // Re-apply the filter, suspending the observer so our writes don't re-trigger it.
  function run() {
    if (observer) observer.disconnect();
    apply();
    if (observer && board) observer.observe(board, { childList: true, subtree: true });
  }

  if (board) {
    observer = new MutationObserver(() => {
      if (input.value) run();
    });
    observer.observe(board, { childList: true, subtree: true });
  }

  function clearSearch() {
    input.value = '';
    run();
    input.focus();
  }

  input.addEventListener('input', run);
  if (wrap) {
    const clearBtn = wrap.querySelector('.search-clear');
    if (clearBtn) clearBtn.addEventListener('click', clearSearch);
  }

  // A modal mounts into `#modal`; while one is open it owns Escape (it should
  // close, not clear the search behind it). The mount is empty otherwise.
  const modalMount = document.getElementById('modal');
  const modalOpen = () => modalMount && modalMount.childElementCount > 0;

  document.addEventListener('keydown', (e) => {
    const ae = document.activeElement;
    const typing =
      (ae && (ae.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(ae.tagName)));
    if (e.key === '/' && !typing) {
      e.preventDefault();
      input.focus();
    } else if (e.key === 'Escape' && input.value && !modalOpen()) {
      // preventDefault so a `type=search` field's native Escape-clear doesn't
      // double-fire; we own the clear (and yield Escape to any open modal).
      e.preventDefault();
      clearSearch();
    }
  });
})();
