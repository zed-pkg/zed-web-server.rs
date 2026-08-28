// Offline shell for the installed registry app.
//
// Deliberately narrow. The cache holds the stylesheet, scripts, icons, and one
// offline page — assets that are identical for every viewer. HTML responses are
// never cached: a package page can be private, and a shared on-device cache is
// the wrong place for anything whose visibility depends on who asked. The only
// thing the worker does for navigations is serve the offline page when the
// network is gone.
//
// Bump CACHE_VERSION whenever a precached asset changes; the activate handler
// deletes every older cache, so a stale stylesheet cannot outlive a deploy.

const CACHE_VERSION = "zed-shell-v1";

const PRECACHE = [
  "/static/styles.css",
  "/static/app.js",
  "/static/htmx.min.js",
  "/static/favicon.svg",
  "/static/icon.svg",
  "/static/offline.html",
  "/static/manifest.webmanifest",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE_VERSION)
      // One missing asset must not abandon the whole precache, so each is
      // added independently and failures are tolerated.
      .then((cache) =>
        Promise.all(
          PRECACHE.map((url) => cache.add(url).catch(() => undefined))
        )
      )
      .then(() => self.skipWaiting())
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key !== CACHE_VERSION)
            .map((key) => caches.delete(key))
        )
      )
      .then(() => self.clients.claim())
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;

  // Only ever touch same-origin GETs. Anything else — a publish, a sign-out
  // POST, a cross-origin asset — goes straight to the network untouched.
  if (request.method !== "GET") {
    return;
  }
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) {
    return;
  }

  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request).catch(() =>
        caches
          .match("/static/offline.html")
          .then(
            (cached) =>
              cached ||
              new Response("Offline", {
                status: 503,
                headers: { "content-type": "text/plain; charset=utf-8" },
              })
          )
      )
    );
    return;
  }

  // Static assets: serve the cached copy immediately, refresh it in the
  // background. Auth and API paths are excluded outright.
  if (!url.pathname.startsWith("/static/") && url.pathname !== "/sw.js") {
    return;
  }

  event.respondWith(
    caches.match(request).then((cached) => {
      const network = fetch(request)
        .then((response) => {
          if (response && response.ok && response.type === "basic") {
            const copy = response.clone();
            caches.open(CACHE_VERSION).then((cache) => cache.put(request, copy));
          }
          return response;
        })
        .catch(() => cached);
      return cached || network;
    })
  );
});
