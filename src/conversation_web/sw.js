'use strict';
const cacheName = 'conversations-shell-v1';
const assets = ['/', '/app.js', '/app.css', '/components.css', '/brand.png', '/icon.png', '/manifest.webmanifest', '/mic.js'];
self.addEventListener('install', event => {
  event.waitUntil(caches.open(cacheName).then(cache => cache.addAll(assets)));
});
self.addEventListener('activate', event => {
  event.waitUntil(self.clients.claim());
});
// Only cache the app shell. Pairing, transcripts and voice always go to the Mac.
self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET' || url.origin !== location.origin || !assets.includes(url.pathname) || url.search) return;
  event.respondWith(fetch(event.request, {signal:AbortSignal.timeout(3000)}).then(async response => {
    if (!response.ok) throw Error('Mac unavailable');
    const cache = await caches.open(cacheName);
    await cache.put(event.request, response.clone());
    return response;
  }).catch(async error => {
    const saved = await caches.match(event.request, {cacheName});
    if (saved) return saved;
    throw error;
  }));
});
