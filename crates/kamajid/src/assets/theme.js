// kamaji — light/dark theme toggle.
//
// The initial theme is applied by a tiny inline script in the document head
// (before first paint, to avoid a flash) from `localStorage['kamaji.theme']`,
// defaulting to light. This module just exposes the toggle the rail's theme
// button calls (`data-on:click="window.__kamajiToggleTheme()"`) and persists
// the choice. The label/icon swap is pure CSS keyed off `html[data-theme]`.
(() => {
  const KEY = 'kamaji.theme';
  window.__kamajiToggleTheme = function () {
    const cur =
      document.documentElement.getAttribute('data-theme') === 'dark'
        ? 'dark'
        : 'light';
    const next = cur === 'dark' ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', next);
    try {
      localStorage.setItem(KEY, next);
    } catch (_) {}
  };
})();
