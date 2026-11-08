# Capwise performance baseline

First baseline, 2026-11-08. Only measured, nothing optimized. Raw data for the main run is in [`perf-baseline-20261108.json`](perf-baseline-20261108.json).

## How to run

```bash
apps/desktop-gpui/scripts/bench.sh                                   # all stages, about 5 min, JSON written to apps/desktop-gpui/bench-results/
apps/desktop-gpui/scripts/bench.sh --stages startup,record           # any subset
apps/desktop-gpui/scripts/bench.sh --fixture path/to/x.cap --stages timeline,playback,scrub,export
apps/desktop-gpui/scripts/bench.sh compare old.json new.json          # regression comparison
python3 apps/desktop-gpui/scripts/test_bench_perf.py                   # tests for the script itself
```

- It drives `/Applications/Capwise.app` by default (Developer ID signed, so it already has the Screen Recording permission; it never runs the debug `cap-gpui` directly). Use `--app` to point at another .app.
- Every launch uses a separate temporary sandbox (`CAP_GPUI_APP_DATA_DIR` / `RECORDINGS_DIR` / `LOG_DIR`), so the user's library and settings are never touched. The sandbox store pre-fills: onboarding done, countdown 0, no "recording without microphone" confirmation, **auto-captions off**.
- The record stage records the primary display (plus system audio) for 40 s + 6 s + 6 s. The 40 s take then serves as the fixture for the timeline, playback, scrub and export stages. The sandbox is deleted afterwards (`--keep` keeps it), and nothing is committed to the repo.
- The scrub stage needs `target/debug/examples/scrub-benchmark` (`--build-scrub` builds it automatically; the first build takes about 18 min).
- Every timing comes from timestamps of log lines the app already writes. The only new instrumentation is `CAP_PERF_MARKS=1` (`src/perf_marks.rs`, 3 startup markers), which only shows up after the next `install-local.sh`.
- Don't reinstall Capwise.app during a run: the script compares the binary's mtime and aborts if it changed.

## Test environment

| Item | Value |
|---|---|
| Machine | Apple M5 Pro, macOS 27.0.1 |
| App | Capwise 0.1.0, **debug build** (`install-local.sh` defaults to `PROFILE=debug`: the app crate is opt-level 0, dependencies opt-level 2, the 218 MB binary is not stripped) |
| Code | `c0be3cc43` |
| Fixture | Studio recording of the real screen, 3024×1964, about 39 s, ~30 fps (SCK only delivers frames when the screen changes), with system audio |
| Load | **load average 13–61**: several other sessions were running `cargo build` in parallel. Startup numbers are heavily affected by this |

The installed app was reinstalled by other sessions twice during testing (01:29 and 01:59), so the three valid runs below come from three different builds. The table shows the main run (C) plus run (B) for comparison.

## Baseline numbers

| Metric | Target | C 02:01 (build 01:59) | B 01:40 (build 01:29) | Met |
|---|---|---|---|---|
| Launch → main window created (median of 5) | < 300 ms | 1256 ms (range 625–1503) | 485 ms (445–679) | ❌ |
| Launch → device list ready (can start recording) | < 300 ms | 3509 ms | 1252 ms | ❌ |
| Idle memory, phys footprint | < 150 MB¹ | 264 MB (RSS 193 MB) | 281 MB (RSS 163 MB) | ❌ |
| Idle CPU | < 1 %¹ | 3.0 % | 0.4 % | ❌/✅ |
| Record pressed → first frame enters the encoder | < 200 ms | 1345 ms | 1157 ms | ❌ |
| CPU while recording (3024×1964 full screen + system audio, 100 % = 1 core) | < 30 %¹ | 4.5 % | 4.6 % | ✅ |
| Memory while recording, phys footprint peak | < 400 MB¹ | 340 MB | 376 MB | ✅ |
| Stop recording → editor first frame (median of 3) | < 500 ms | 435 ms (414–611) | 978 ms (910–1236) | ✅/❌ |
| Timeline hover/scrub draw p95 | < 16.7 ms | 2.5 ms | 1.65 ms | ✅ |
| Playback-follow draw p95 | < 16.7 ms | 1.5 ms | 1.95 ms | ✅ |
| Frame interval during playback p50 / p99 / max | 16.7 / < 33 ms | 16.67 / 17.64 / 21.2 ms | 11.1 / 19.9 / 28.9 ms | ✅ |
| Paused scrub, request → frame, p50 / p95 | p95 < 16.7 ms | 2.9 / 8.3 ms | 3.6 / **99.9 ms**² | ⚠️ |
| Paused seek to a new position (max) | — | 565 ms | 270 ms | — |
| In-app export, 720p30 default, VideoToolbox hardware | > 5× realtime | **15.9×** (31 s of video in 1.94 s) | 13.5× | ✅ |
| cap-cli export 720p30 | > 5× | 13.2× | 9.7× | ✅ |
| cap-cli export 1080p60 | > 5× | 4.7× | 4.4× | ❌³ |

¹ The task didn't set targets for these four; the values are placeholders, change them in `TARGETS` in `bench_perf.py`.
² Run B's p95 came entirely from the first two regions (drag p95 232 ms / 100 ms right after a cold seek); the other three regions were all < 7 ms.
³ The installed `cap-cli` is a debug build of the root workspace, where the workspace's own crates (`cap-export` / `cap-rendering` / `cap-editor`) are **opt-level 0**, so it isn't representative. The app itself compiles those crates at opt-level 2.

Playback-engine side: `effective_fps=60.0`, `total_skipped=0`, `starved=0`, every frame from prefetch. The timeline draw times above are UI-thread CPU time; playback frame intervals are the real on-screen cadence.

## Which log lines each metric comes from

| Metric | Start | End |
|---|---|---|
| Launch → window | `Popen` (includes dyld loading the 218 MB binary) | `cap_gpui: main window opened` |
| Launch → device list | same | `enumerated capture devices` |
| Idle | measured 8 s after the device list is ready: `footprint -p`, `ps rss`, 5 s CPU-time delta | |
| Record start | `auto-record start requested` (the same `start_recording` the button calls) | `Start gate admitted first video frame` |
| Recording CPU/memory | sampled every 0.5 s from 2 s after `recording started` until stop | |
| Stop → editor | `auto-record stop requested` | `editor frame number=0` |
| Timeline | existing `CAP_GPUI_AUTO_TIMELINE_BENCH` (`editor_bench.rs`, 300 synchronous `Window::draw` calls per scenario) | |
| Playback | `CAP_GPUI_AUTO_FRAME_STATS=6` + `CAP_GPUI_AUTO_PLAYBACK=8` | |
| Scrub | existing `examples/scrub-benchmark` (headless `EditorInstance`, 5 regions × 30 steps) | |
| Export | `auto editor export requested` → `editor export completed`, divided by the output's ffprobe duration | |

## What can't be measured automatically yet, and why

- **True cold start** (after a reboot or with the page cache cleared): needs `sudo purge` or a reboot. Right now only the first launch after the binary changes is somewhat cold (C's first run took 1256 ms); the rest are warm.
- **A real click on the Record button**: synthetic clicks are dropped without Accessibility permission, so measurement starts at the `start_recording` call and doesn't include the click-to-event delay (normally < 16 ms). The countdown is also set to 0.
- **A real mouse drag on the timeline**: same reason. Replaced by "UI draw time + headless scrub latency"; the overall latency of the two pipelines together (input → frame on screen) isn't measured.
- **GPU time / composited present**: draw times are CPU-side `Window::draw`. Frame intervals only cover the playback scenario.
- **Camera, microphone, 4K / 1080p60 in-app export, export with burned-in captions**: the sandbox uses `AUTO_NO_MIC` and no camera. In-app export uses the defaults (720p30); measuring 1080p60 needs the export preferences pre-filled in the store (follow-up item).
- **Release build**: the installed app is debug. Getting release numbers needs `PROFILE=release scripts/install-local.sh` (it replaces /Applications/Capwise.app, so ask the user first).
- **Startup phase breakdown**: `CAP_PERF_MARKS` only takes effect after the next install.
- **Quiet-machine numbers**: while sessions are compiling in parallel, startup varies 2–3×. Re-run once the machine is idle before comparing.

## Initial analysis of the misses (not optimized this time)

### 1. Startup 0.45–1.5 s to window, 1.2–3.5 s to device list
- The first log line appears 40–140 ms after spawn (dyld plus static initialization of a 218 MB debug binary). With the app crate at opt-level 0, every bit of startup code runs slower.
- In one run (01:09 build) there were 3.4 s between `install_appkit_exception_guards` and font loading, i.e. inside `gpui_platform::application()` / `app.run` launching. That's AppKit/gpui launch or contention, and it needs `CAP_PERF_MARKS` to confirm.
- The device list is ready 0.7–2.3 s after the window: it enumerates 2 displays + 48 windows + cameras + microphones in one go, and a 50-window thumbnail sweep (`window thumbnail sweep captured=50`) runs at the same time. Recording only needs the primary display.
- Direction: test with a release build first; make the Record button depend only on the display list, and load windows/cameras/thumbnails lazily; read `CAP_PERF_MARKS` to see whether gpui launch or our own setup is slow.

### 2. Idle footprint 260–360 MB (RSS 130–190 MB), CPU 0.4–3 %
- The editor's wgpu device and YUV converter textures are warmed up at launch (`launch render warmup`), and the Metal instance buffer is enlarged to 4 MB. Those count toward the footprint even when no editor ever opens.
- The thumbnail sweep (50 window thumbnails) leaves memory and periodic CPU behind; the main window is hidden at idle and should stop.
- Direction: warm up when the editor is about to open; pause thumbnail sweeps while the window is hidden; cap the thumbnail cache.

### 3. Record → first frame 1.06–2.27 s (biggest gap, target 200 ms)
Breakdown of one typical take (run C):
| Phase | Time |
|---|---|
| `start requested` → `starting recording` (window hide, excluded-window lookup) | ~160 ms |
| → SCK configured (`excluding window from capture` arrives 0.3–0.9 s later; SCShareableContent enumeration) | 300–900 ms |
| → `pipeline playing` | 460–1130 ms cumulative |
| Start cue playback; the start gate only opens when the cue ends (`played_back_after_ms≈680`) | **+680 ms** |
| Gate open → first frame admitted (SCK only delivers on screen change; nearly 0 ms when frames are waiting, up to 460 ms on a static screen) | 10–460 ms |
- So about 680 ms is the deliberate design of "don't record the start sound", and another 0.5–1 s is SCK/shareable-content preparation.
- Direction: prepare `SCShareableContent` and the capture session ahead of time when the main window is shown or the target is armed; start the cue at the same moment as pipeline setup (in parallel instead of serially), or shorten the cue to ≤150 ms; use the gate only to drop frames that contain the sound, and don't hold back the "recording started" state shown in the UI.

### 4. Stop → editor first frame 0.41–1.24 s (one 4.0 s outlier)
- The editor shell appears 160–530 ms after stop (`editor window opened`). Whether the first frame has to wait for finalization depends on the build: in the 01:59 build the editor frame comes before `recording finished` (414–611 ms); the 01:29 build waited for finalization (910–1236 ms); the first run of the 01:09 build hit **4.0 s** on a 40 s recording: concatenating 17 m4s segments took 1.5 s, validating the finished video 0.85 s, writing the config 0.3 s, all on the critical path, plus `Preparing playback declined error=Missing or reused preparing input`.
- Direction: make sure the "preview directly from the fragmented segments" path is always taken (keep an eye on the declined case above); move finalization validation off the critical path; finalization time grows linearly with recording length, so add a 5-minute recording as a regression case.

### 5. Paused scrub p95 is unstable (8 ms vs 100 ms)
- Drag latency is 2–4 ms once warm, but the first steps after seeking to a new region cost 100–565 ms. `Initialized multi-position decoder pool decoder_count=1 optimal_pool_size=5`: only 1 of the 5 decoders in the pool is initialized.
- Direction: initialize the decoder pool in full when the editor opens; check the recording's keyframe interval (affects seek cost); show the nearest cached frame first when the target frame isn't decoded yet.

### 6. CLI 1080p60 export 4.4–4.7×
- See footnote 3: a build-config problem, not the export pipeline itself. The in-app hardware path reaches 16× at 720p30. Next step: measure in-app 1080p60 and 4K.

## Other findings while measuring (worth separate follow-up, not fixed)

1. **With auto-captions on (default `autoCaptionsAfterRecording=true`), in-app export hangs**: the first run (B) stalled after `Encoder thread finished encoded_frames=128` (expected 935) for more than 15 min, while the local Qwen3-ASR (llama-server) was transcribing. With auto-captions off, the same fixture exported in 2 s. Reproduced once; root cause not yet located.
2. **llama-server is orphaned when the app exits**: every recording-and-editor run starts a llama-server (it loads the model from the real `~/Library/Application Support/so.cap.desktop`, ignoring `CAP_GPUI_APP_DATA_DIR`). When the main process exits via SIGTERM the helper is left behind (PPID=1), and 4 orphans holding the ASR model accumulated during testing. The script now cleans up child processes and turns auto-captions off in the sandbox, but the app itself should make sure helpers exit with it.
3. **`device_query` panic while recording**: `device_query-4.0.1/src/device_state/macos/mod.rs:122` "This app does not have Accessibility permission". It's caught by `crash_capture`; the thread that collects keyboard/mouse events probably exits as a result, so keyboard and click data may be lost silently.
4. **The installed app is a debug build**: every number above is from debug. Users get a debug build from `install-local.sh` by default; consider switching the default to release or adding a profile in between.
