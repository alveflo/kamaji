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
