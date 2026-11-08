const cacheName = "capwise-__ASSET_VERSION__";
const shell = ["/offline.html", "/assets/app-icon.png"];

self.addEventListener("install", (event) => {
	event.waitUntil(caches.open(cacheName).then((cache) => cache.addAll(shell)));
	self.skipWaiting();
});

self.addEventListener("activate", (event) => {
	event.waitUntil(
		caches.keys().then(async (names) => {
			await Promise.all(
				names
					.filter((name) => name.startsWith("capwise-") && name !== cacheName)
					.map((name) => caches.delete(name)),
			);
			await self.clients.claim();
		}),
	);
});

self.addEventListener("fetch", (event) => {
	const url = new URL(event.request.url);
	if (event.request.method !== "GET" || url.origin !== self.location.origin)
		return;
	if (event.request.mode === "navigate") {
		event.respondWith(
			fetch(event.request).catch(() => caches.match("/offline.html")),
		);
		return;
	}
	if (
		!url.pathname.startsWith("/assets/") ||
		!/\.(webp|png|jpg)$/.test(url.pathname)
	)
		return;
	event.respondWith(
		caches.open(cacheName).then(async (cache) => {
			const cached = await cache.match(event.request);
			if (cached) return cached;
			const response = await fetch(event.request);
			if (response.ok) await cache.put(event.request, response.clone());
			return response;
		}),
	);
});
