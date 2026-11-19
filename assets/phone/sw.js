// The phone view's service worker: it shows the notes gerdr pushes (see
// `web_push`), and a tap opens the page the note names.

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

self.addEventListener("push", (e) => {
  let n = {};
  try { n = e.data ? e.data.json() : {}; } catch { n = {}; }
  e.waitUntil(self.registration.showNotification(n.title || "gerdr", {
    body: n.body || "",
    tag: n.tag || undefined,
    icon: "/icon-192.png",
    badge: "/icon-192.png",
    data: { url: n.url || "/" },
  }));
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const url = new URL(e.notification.data?.url || "/", self.location.origin);
  e.waitUntil((async () => {
    // A note of elsewhere (a pull request's checks) opens there.
    if (url.origin !== self.location.origin) return self.clients.openWindow(url.href);
    const open = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const page = open[0];
    if (!page) return self.clients.openWindow(url.href);
    await page.focus();
    page.postMessage({ go: url.hash || "#/" });
  })());
});
