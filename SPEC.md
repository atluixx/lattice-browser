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

## Corrections from live testing (Nov 10)
- Drivers are currently ONE-SHOT: spike_load renders once, saves, and
  exits (exit 0/2). Background tabs therefore show static snapshots, not
  live pages; SIGSTOP/SIGCONT only engages if the user switches tabs
  mid-render (~15-60s window). The v0.4 architecture doc implied
  long-lived frozen drivers — correct statement: per-navigation renderer
  processes with SIGSTOP available during their render window. A
  persistent-driver design (live background tabs) is future work and
  changes the memory story, since N live Servo instances cost N× engine.
- Proven visually: session restore with 2 titled tabs + active YouTube
  page rendered in-window (v28); typed-URL navigation with title/status
  update + in-window pixels (v25); full YouTube watch page pixels via the
  exact texture files the window composites (frame-1-3.png); MP4 decodes
  to real frames via the GStreamer backend (flower.png).
- NOT yet proven visually: clicking an inactive tab pill to switch
  (Tab2->Tab1). + button and navigate-verified; switch-back screenshot
  pending an idle moment. Video has no audio verification and no HW
  decode; YouTube needs MSE (absent in Servo 0.1.4) regardless of backend.

## Video: DECODES CLEANLY with full plugin set (Nov 10)
- MP4 (flower.mp4, H.264): 0 player errors, 0 missing-plugin errors,
  decoded flower frame saved (51k non-white samples).
  `GST_PLUGIN_PATH` = union of the 5 nixpkgs 1.26.11 plugin dirs
  (good, bad, libav, base + base deps). Do NOT use
  GST_PLUGIN_SYSTEM_PATH alone: something in this environment makes
  GStreamer ignore it and scan only /usr/lib (foreign install), which
  lacks demuxers/decoders. aacparse (bad) + avdec_aac (libav) verified
  present via gst-inspect.
- Remaining video work: audibility (pipeline prerolls; no listening
  test), hardware decode (untouched), MSE (absent in Servo 0.1.4, so
  YouTube proper stays on the fallback), in-window playback screenshot.

## Audio: attempted, fails at track setup (Nov 10/11)
- flower.mp4 has H.264 video + AAC audio (ffprobe-verified).
- With the union plugin set, video decodes and composites; on the audio
  path Servo logs `Could not set audio track SetTrackFailed` from
  `servo-media-gstreamer-0.1.4/player.rs::set_audio_track` (inner
  `player.set_audio_track(stream_index)` errors). Video keeps playing.
- Likely a stream-registration race or sink negotiation gap in the 0.1.4
  media glue, not a missing codec (aacparse + avdec_aac verified
  present via gst-inspect). Root-causing needs media-internals debugging
  with rebuild cycles — queued behind idle time, not blocking visual
  browsing or the MSE situation.

## Tab switching: PROVEN in-window (Nov 11, j10/j11)
- Two tabs open (example.com + Google); clicking the inactive
  "Example Domain" pill swaps the entire view: active marker moves,
  address bar + status + page pixels all switch to example.com content
  while the Google tab keeps its title. Screenshots j10 (Google active)
  and j11 (example active) form the before/after pair.
- Coordinates learned the hard way: tab strip y~185, toolbar y~242,
  address text y~290 at window (320,155); verify with a fresh screenshot
  before clicking, windows move between runs.

## Tab switching: PROVEN in-window (Nov 11, k7/k8)
- Clicking the inactive "Example Domain" pill swaps the whole view:
  active marker moves Tab2->Tab1, address bar + status + page pixels all
  follow (example.com text render). Reverse direction (new tab via +
  becoming active with its own render) shown in the same session.
- This also validates the unique-frame-path fix: before it, switched
  tabs showed stale all-white frames from GPUI's path-keyed image cache;
  after it, both tabs render their own content.

## Upstream blockers, verified Nov 11 (no build needed)
- MSE: ABSENT even on Servo main (components/script/dom/media has no
  mediasource/sourcebuffer files; 0.1.4 identical). YouTube playback is
  blocked upstream, not by our wiring. Options: (a) accept fallback-only
  YouTube, (b) track Servo MSE development, (c) never implement MSE
  ourselves (enormous). Recommendation: (a) + (b).
- Single-binary link: monthly Servo (0.8.x) uses stylo 0.23 vs LTS 0.15,
  so the rustc-1.99 ToCss-derive failure likely disappears on upgrade.
  Trade-off: monthly API churn + rebuild everything vs LTS stability.
  Decision needed from you before any attempt; two-process remains the
  shipping shape either way.
- Audio SetTrackFailed appears even in fully-working runs (video renders
  fine), so it is a benign script-side warning for now; audibility
  itself is still unverified (needs ears, idle session).

## Memory: RELEASE dual-bench datum (Nov 11, do not cherry-pick)
- Release `spike_dual` (Servo 0.1.4 + media, single process, YouTube
  Complete + WhatsApp HeadParsed/still booting, 1280x800 x2, settle):
  **peak 1,277,500 KB = 1.22 GiB**, 18.8s wall.
- Passes the letter of the MVP target (<1.4GB), and is ~48% under the
  original 2.37GB Firefox session that motivated the project.
- BUT same-machine Firefox with just YouTube+WhatsApp measured ~903MB
  in a later session. On a strict 2-site A/B today, Firefox wins;
  Lattice does not demonstrate the 40-50% saving on the light workload.
  The saving thesis now rests on scaling (many-tab sessions where
  Firefox's per-process overhead dominates), which is UNMEASURED.
- Honest status: rough parity on 2 heavy pages, possible win at scale,
  no evidence yet. Next: measure 6-10 tab sessions on both, and try
  cache caps + no-media builds as levers. No efficiency claim in any
  user-facing copy until then.
