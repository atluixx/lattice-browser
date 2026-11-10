# AGENTS.md — instructions for AI agents working in this repo

Read `SPEC.md` first. It holds all product decisions and M0 findings.
Do not silently change locked decisions (engine, process model, MVP scope).
Efficiency-first: no UI cost without idle RAM/CPU neutrality.

## Git policy (mandatory)

- Changes must ALWAYS be committed to GitHub. Never end a task with
  uncommitted work. Commit early and often, one logical change per commit.
- After every commit, `git push origin <branch>`. Work happens on `main`
  unless the person asks for a branch.
- Keep commit messages short and concise (a hook enforces this).
- Never use `--no-verify`, `REVIEWERS_BYPASS=1`, or `REVIEWERS_FRESH=1`
  unless the person explicitly asks.
- If a commit is blocked by Reviewers, read the hook output, fix the code,
  and retry. Retry once on runner crashes (`codex exited 1`); if it still
  fails, report to the person instead of bypassing.

## Build (NixOS 26.05, GCC 15, glibc 2.42)

- Fast checks (no Servo): `nix-shell -p gcc pkg-config --run "cargo check -p lattice-core"`
- Full checks need the Servo C toolchain or `tikv-jemalloc-sys` configure
  fails (`strerror_r`, Nix `_FORTIFY_SOURCE` + `-Werror` + `-O0`):
  `nix-shell -p rustPlatform.bindgenHook llvmPackages.libstdcxxClang cmake llvm python311 pkg-config openssl wayland libxkbcommon libx11 fontconfig freetype m4 perl git --run "export NIX_HARDENING_ENABLE=\"\"; ..."`
- The same flags live in `flake.nix` (`hardeningDisable = [ "fortify" ]`).
  `nix develop` may fail on GitHub fetches (truncated tar); fall back to
  `nix-shell -p ...` with channel packages.
- Servo crates: `cargo check -p lattice-embed --features servo-embed`.
  Shell with Servo: `cargo check -p lattice-shell --features servo`.
- Never run two `cargo` commands on the same target dir concurrently
  (lock contention). Use `CARGO_TARGET_DIR=/tmp/<name>` to isolate.

## Tests and benchmarks

- `CARGO_TARGET_DIR=/tmp/lattice-m0-test cargo test -p lattice-core`
- Spike (windowless Servo render + PNG): binary at
  `/tmp/lattice-servo-check2/debug/examples/spike_load`
  (rebuilt via the full-toolchain shell above).
- `LATTICE_UA` must be a Chrome UA string or YouTube serves an error page
  (bot mitigation resets the H2 stream for the `Servo/` token).
- `bash bench/bench.sh '<url>' <timeout>` prints load report + peak RSS.
  The spike segfaults at teardown (no orderly Servo shutdown in 0.1.4);
  judge by stdout, never by exit code. Run under `stdbuf -o0 -e0`.

## Layout

- `crates/lattice-core`: tabs, policy engine, permissions, session (tested).
- `crates/lattice-embed`: Servo boundary; `headless` module (feature
  `servo-embed`) shared by spikes; `examples/spike_load.rs`,
  `examples/spike_dual.rs` (2-tab bench).
- `crates/lattice-shell`: GPUI window; `servo` feature enables the
  Servo driver thread (`src/page.rs`) + texture upload.
- `bench/`: M0 benchmark script. `patches/xattr`: vendored libc fix.
