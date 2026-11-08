# Launch verification

Verified on 2026-11-08, macOS / Apple Silicon.

## Demo project

- `cap-cli project validate`: valid, all required media present, duration 24 seconds.
- Native editor opened the project and displayed editable titles, six caption entries and a multitrack timeline. The 4-second preview showed the intended layout and bilingual captions.
- `cap-cli export`: 720 / 720 frames completed; FFprobe confirmed H.264 + AAC, 1920 × 1080, 30 fps, 24.000 seconds. Frames at 4, 10, 17 and 23 seconds were visually checked.
- Source artwork and NOVA branding are original fictional demonstration content. Captions are authored sample text, with no word-level speech alignment. Music is a 24-second excerpt of the maintainer-provided `friendly-welcome` track, mixed at -18 dB with fades. Audio was checked technically, without automatic playback or microphone recording.
- Public ZIP was generated from a separate clean project, not from the live project after manual editing. It contains no keys, account configuration or private recordings.

## Website

- HTTPS custom domain: `capwise.qiaomu.ai`, Cloudflare Worker `capwise-website`.
- Desktop 1512 px and mobile 390 × 844 layouts checked; mobile document width was exactly 390 px with no horizontal overflow.
- Gallery selection, ArrowRight keyboard selection, screenshot enlargement and Escape dismissal passed. FAQ expansion passed.
- Follow and reward dialogs displayed their respective canonical QR assets. Copy-install action reported successful clipboard copy.
- Build checks local linked resources, public Umami ID, valid JSON-LD, generated CSP hash and a cache version derived from HTML and all public assets.
- Scoped Biome, Prettier, JavaScript syntax and Python compile checks passed. No native source files changed in this launch work, so Rust gates were not run.
- Structured data, Open Graph card, sitemap, robots, manifest, offline fallback and service worker included. Offline fallback is a website notice, not an offline native-app installer.
- Cloudflare's existing beacon injection is allowed by the site's CSP using the origins in [Cloudflare's documented CSP requirements](https://developers.cloudflare.com/fundamentals/reference/policies-compliances/content-security-policies/).

## Analytics

- Unique Umami property created through its authenticated dashboard: `b16b78e3-7c93-49df-8495-078eba543e57`.
- Production browser visited `/?qa=launch-20261108-capwise` and sent event `launch-qa-20261108-capwise` at **2026-11-08 15:54:40 UTC**.
- Read back the exact event name, `capwise.qiaomu.ai` domain and matching time in that property's Realtime dashboard. This is QA traffic, not an adoption metric.
- VPS registry appended after a timestamped backup; no analytics database access or changes.

## Installation handoff

- Downloaded the public **v0.3.0** `Capwise.dmg` from GitHub.
- SHA-256 matched release checksums: `f844ff63d2a29d3b920d4f452e220146805a3762619f353001a3c497778e2447`.
- Read-only mount; `codesign --verify --deep --strict`, `spctl` app assessment, `stapler validate` and DMG Gatekeeper assessment passed. Source: Notarized Developer ID.
- Image unmounted afterward. Existing installation, projects, preferences and Keychain were not replaced or cleared. Download links intentionally follow the latest release; future binaries need their own verification.
