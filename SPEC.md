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
