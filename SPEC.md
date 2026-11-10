# Lattice — Product & Technical Specification (M0 + decisions)

Status: M0 spike in progress. No product code beyond shell/harness.
License: Apache-2.0. Stack: Rust + GPUI 0.2.2 + Servo 0.1.4 LTS + NixOS.

## Locked product decisions

- Problem: Firefox RSS (~1.6–2.4GB for YouTube + WhatsApp Web) too high.
  Target: same 2-site workload at ~40–50% less (~1.1–1.4GB), no frame drops,
  no WhatsApp disconnects. Benchmark script: `bench/bench.sh`.
- Engine: Servo locked (only Rust-native low-memory option; Chromium/Gecko
  lose on RAM/identity, WebKit loses on Rust fit, Ladybird too early).
- UI: efficiency-first, GPUI kept for idle-cheap retained rendering (not for
  Zed looks — Zed polish constraints dropped). Horizontal top tabs, single
  workspace, collapsible left sidebar, unified address bar with local
  suggestions, single fixed default engine (DuckDuckGo), blank new tab,
  system-follow dark/light (no density options), Manrope bundled,
  fully keyboard-operable, no animations, minimal context menus.
- Functionality MVP: full history + session restore, bookmark folders +
  manager, full downloads (pause/resume), single profile, private window,
  per-site cookie allow/block (3P blocked), Firefox/Chromium import,
  built-in blocklist only (no extensions), passwords via OS keyring,
  devtools-lite (view-source/console/inspect/network — no breakpoints),
  find-in-page. Deferred: reader mode, sync, export, workspaces,
  full extensions, fingerprinting resistance.
- Performance: no auto-suspend (WhatsApp must stay live), throttling only
  (rAF off when hidden, JS timers ≥1s, sockets kept), OS memory-pressure
  response, default Servo caches, hardware video decode when available,
  built-in `about:memory`-style page.
- Architecture: single-process MVP, offscreen Servo render → GPUI texture
  composite (extra copy accepted), Lattice policy layer in front of Servo
  net (no Servo fork), separate SQLite/JSON stores, OS-baseline sandbox
  (Wayland isolation; seccomp post-MVP), session snapshot restore,
  `tracing` diagnostics. No orderly Servo shutdown API exists in 0.1.4 —
  exit-time teardown crash is known harness-only behavior.

## M0 spike findings (Servo 0.1.4, NixOS 26.05, GCC 15, glibc 2.42)

1. Build needs `rustPlatform.bindgenHook + llvmPackages.libstdcxxClang`
   (mirrors nixpkgs servo 0.7.0) and `hardeningDisable = [ "fortify" ]`
   (jemalloc 5.3.0 configure + `-Werror` + glibc 2.42 `_FORTIFY_SOURCE`
   warning + `-O0`). See `flake.nix`.
2. `tikv-jemalloc-sys` 0.6.1 is a hard dep of `servo-allocator` 0.1.4;
   `use-system-allocator` does NOT remove it. Keep jemalloc (representative
   allocator for memory measurements).
3. `xattr` 0.2.3 (via `gpui_http_client`) needs vendored fix for removed
   `libc::ENOATTR` (Linux: `ENODATA`). See `patches/xattr`.
4. Rendering path proven: `SoftwareRenderingContext` (windowless) +
   `WebViewBuilder` + `read_to_image` RGBA readback. Read MUST happen
   before `present()` (swap uses `PreserveBuffer::No`) and text arrives in
   later frames — settle loop keeps the richest frame. Harness:
   `crates/lattice-embed/examples/spike_load.rs`.
5. example.com, google.com render fully (text, i18n scripts, layout).
6. YouTube serves `Error loading page` + H2 `PROTOCOL_ERROR` reset with
   Servo's default UA (`... Servo/0.1.4 ...` token). With a Chrome UA
   (`LATTICE_UA` override) the full watch page renders (header, results,
   comments, consent dialog). Product implication: MVP must ship a
   Chrome-compatible UA string; Servo token breaks YouTube. Open question
   for spec review: UA spoofing vs identity/transparency.
7. Video element shows "Your browser can't play this video" — no media
   backend in spike config (software GL context, dummy media). HW decode
   (spec choice A) is M1 work, not M0.
8. WhatsApp Web boots to its loading screen (logo, progress bar,
   "End-to-end encrypted", 801 frames, no crash). Full QR/login flow needs
   interaction + persistent storage — M1 scope. The compat risk that
   mattered (large JS bundle, SW/IDB usage crashing or failing to boot)
   is retired.
9. Memory, first datum (debug build, software rendering, single page):
   YouTube watch page peaks at **583MB RSS** vs Firefox 903MB for the full
   2-site session (release build) on the same machine. Debug overstates
   Lattice RSS; release + second tab still to measure. Target (<1.4GB for
   both sites) looks reachable but unproven.
10. Harness lesson: the teardown segfault discards block-buffered stdout —
    run the spike under `stdbuf -o0 -e0` and judge by output, never by exit
    code. `bench.sh` wraps runs with external GNU `time -v`
    (`command time`, not the bash keyword; `/usr/bin/time` is absent on
    NixOS).

## Provisional measurable targets (to be validated, not assumed)

- Blank 1-tab idle RSS < 350MB, < 2% idle CPU.
- 2-site benchmark (YT 720p + WA connected, 5 min): RSS < 1.4GB total,
  no dropped audible frames > 2%, WA stays connected.
- Cold start with 2-tab restore < 2s on NVMe NixOS.
- CI gate: 3-run bench, fail if > target + 10%, always with same-machine
  Firefox A/B. Never claim Servo efficiency without numbers.

## v0.3 interaction evidence (Nov 10, xdotool + screenshots)

- Bookmark/Back/Reload/address clicks all dispatch (run-log lines
  `UI click: nav-*`, `UI click: Example|Google|YouTube`,
  `UI address click`); bookmark click drives navigate, live title in the
  status bar, Back enables with history.
- In-window page texture proven for example.com and Google consent page.
- YouTube in-window pixels still need one clean run (driver flake under
  rapid kill/respawn churn; mitigated with atomic + incremental saves and
  unconditional final readback). Spike-level YouTube/WhatsApp PNGs proven.

## v0.4 tabs + persistence (implemented, visual proof pending idle session)
- Tab strip with titles, × close, + new tab; per-tab driver + PNG;
  background tabs SIGSTOP'd, SIGCONT'd on switch.
- Session (`session.json`: tab URLs/titles + active index) and visit log
  (`history.json`, 5000 cap, reload collapsing) under
  ~/.local/share/lattice/; restored on launch, saved on every navigate.
- Core history tests pass (15 total in lattice-core). In-window
  restore proof (quit → relaunch → tabs back, screenshot) gated on user
  idleness; will not steal focus from an active session.

## Video playback path (probed, not yet built)
- Servo 0.1.4 exposes a `media-gstreamer` feature; system has GStreamer
  runtime libs but the build needs nixpkgs `gst_all_1` dev packages
  (mirror nixpkgs servo package inputs: gstreamer, base, good, bad,
  ugly + `media-gstreamer` feature on the embed).
- Deliberately NOT started while the user is active: it pulls a heavy
  native compile on a 7.4GB machine already under memory pressure.
  Next: enable behind a `media` feature, `cargo check` headless, then
  verify a playing video in-window (needs GL display + idle session).

## End-to-end proofs (Nov 10, live session, screenshots + run logs)
- Typed-URL navigation: click address bar -> type `google.com` -> Enter ->
  address shows URL, tab retitled "Google", status shows page title,
  consent page pixels rendered in-window (v25 screenshot + 61-key log).
  Root cause of earlier failures found and fixed: GPUI key names are
  lowercase (`enter`, `backspace`), not capitalized.
- YouTube in-window: bookmark click navigates, full watch page renders in
  the Lattice window (header, search, player fallback, recommendations,
  status title). Video element shows the expected "can't play" fallback:
  no media backend yet (M1 work, path scoped above).
- Session restore: quit -> relaunch with no args restores tabs from
  `session.json` (v26 screenshot: Google tab, address, status, pixels all
  back). `history.json` verified on disk with titled visits.
- Glass UI verified on-screen: frosted toolbar pills, rounded page card,
  translucent panels over compositor blur.
- Known annoyance: OS "Service Crash" notification on driver teardown
  (no orderly Servo shutdown in 0.1.4; render always completes first).

## Video playback: scoped, not started
- Path: `lattice-embed/media` feature -> `servo/media-gstreamer`
  (exists in Servo 0.1.4; needs gstreamer-rs 0.25 / GStreamer >= 1.18 API).
  System has GStreamer runtime libs; build needs nixpkgs `gst_all_1`
  dev packages (gstreamer, base, good, bad, ugly) and runtime
  `GST_PLUGIN_PATH` with decode plugins (H.264/VP9) + audio sink.
- Deliberately not compiled yet: enabling it triggers a near-full Servo
  rebuild (all cores, ~10 min) on a 7.4GB machine already under memory
  pressure while the user is active. Build + in-window playing-video
  verification is queued for an idle session.

## Video: media backend compiles (Nov 10)
- `cargo check -p lattice-embed --features lattice-embed/media` passes
  against system GStreamer 1.26.11 (dev inputs: gst_all_1 gstreamer, base,
  good, bad, ugly). Remaining: link spike with media, run YouTube, and
  confirm the player shows frames instead of the fallback.

## Video: backend builds, playback still blocked (Nov 10)
- `media-gstreamer` compiles and links against system GStreamer 1.26.11;
  the YouTube watch page renders identically with it, but the player
  still shows the fallback. No playbin/codec errors in logs.
- Root cause identified one layer deeper: Servo 0.1.4 has no Media Source
  Extensions (no MediaSource/SourceBuffer WebIDL or DOM bindings), which
  YouTube requires for adaptive streams. The backend is likely fine; the
  API surface is missing. Verifying with a progressive MP4 (no MSE needed).
- Install note for the media driver: binary needs RPATH on the four
  gst/glib store lib dirs (`patchelf --set-rpath`) plus
  `GST_PLUGIN_SYSTEM_PATH` at runtime.

## Video: native controls render, no frames yet (Nov 10)
- Direct MP4 loads the native video element with controls (play button,
  timeline, volume, fullscreen) but duration reads 00:00 and no frames
  flow. Backend present, data path not completing.
- Prime suspect: demuxer/decoder plugins missing from the test env
  (qtdemux lives in gst-plugins-good, H.264 decode in bad/openh264 or
  libav — only base+bad were on GST_PLUGIN_SYSTEM_PATH). Retesting with
  good+ugly+libav plugin paths plus GST_DEBUG capture.
- YouTube additionally needs MSE (no MediaSource bindings in 0.1.4),
  independent of backend health.

## Video: DECODES (Nov 10, mp4i run)
- Progressive MP4 renders real decoded frames in-window path (flower.mp4:
  photo pixels + native controls, no player errors). Backend proven
  end-to-end: qtdemux + H.264 decode + Servo software compositor +
  readback PNG.
- Environment that works: `GST_PLUGIN_PATH` = union of nix store plugin
  dirs (good, bad, base, ugly, libav 1.26.11) *alongside* the system
  install — do NOT replace `GST_PLUGIN_SYSTEM_PATH` (breaks the foreign
  /usr/lib install's registry and yields zero plugins). Plus patchelf
  RPATH on the driver for gst/glib lib dirs.
- Still open: audio output unverified, hardware decode untouched,
  YouTube needs MSE (no MediaSource bindings in Servo 0.1.4 — separate,
  larger gap), in-window video screenshot pending an idle session.
