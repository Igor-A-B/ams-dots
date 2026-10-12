# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Desktop (Wayland/Hyprland) half of a "vehicle motion cues" tool: a particle-style dot generator drawn as a click-through overlay. The phone sender is a separate project in `../AmsDotsMobileController`. `README.md` documents the dot behaviour, the config keys and the Wayland concepts mapped to code.

## Commands
- `cargo run` (follows the phone over Bluetooth), `cargo run -- --no-phone` (at rest) or `cargo run -- tests/data/drive.csv` (replay) — overlay on every monitor (needs a live Wayland session with wlr-layer-shell); Ctrl+C to stop. `--config FILE` overrides `~/.config/ams-dots/config.toml`.
- `cargo test` — all unit tests; `cargo test spring` or `cargo test dots_flow_with_the_felt_inertia` runs a subset by name
- `ams-dots-linux status [--watch]` / `connect` / `disconnect` / `relearn` — talk to the running overlay via `$XDG_RUNTIME_DIR/ams-dots.sock`
- `hyprctl layers` — verify the `ams-dots` surface is on the overlay level

## Architecture
Per frame: `MotionSource::at` (phone, replay sample or zeros) -> `Spring` smooths (lateral, forward) acceleration -> intensity + drift -> `particles::Field` -> `Vec<Dot>` -> `dots::draw` into a `Pixmap` -> `wl_shm` buffer.
- `particles.rs`, `spring.rs`, `dots.rs`, `config.rs`, `replay.rs`, `protocol.rs`, `fusion.rs`, `motion.rs` are Wayland- and Bluetooth-free and unit-tested; keep Wayland code in `overlay.rs`.
- `overlay.rs` holds all Wayland state in one `App`, driven by sctk handler traits; each monitor has its own `Overlay` with its own `Sim` (field + spring). Draw only after the compositor's `configure`; request the next frame callback *before* the commit it belongs to, or the animation stalls. `frame_pending` prevents duplicate callback chains.
- Click-through relies on the empty input region set in `App::add_overlay`; do not remove it.
- Dots are painted directly into the `wl_shm` buffer (`PixmapMut::from_bytes`) with no R/B swap, which is only valid while dots are white/black (see `App::draw`).
- Performance depends on partial clearing: `App::slots` records what each reused buffer holds and `draw` clears only those rectangles. Changing how or where dots are painted (e.g. adding glow or new bands) must keep `dots::bounds_by_side` covering every painted pixel, or ghost dots remain on screen.
- Sign conventions: `forward > 0` = speeding up (dots flow down), `lateral > 0` = passengers pushed right (dots flow right); `fusion.rs` produces the same conventions from phone data (up = gravity, which Android reports pointing up; right = forward x up; lateral = minus the acceleration along right; yaw = gyro . up). Forward is learned automatically (principal axis of straight-driving acceleration after >5 s, front/back from turns: acceleration x yaw points left); its tests drive the bundled `drive.csv` through random phone orientations. There is deliberately no manual calibration.
- sctk 0.21 uses `delegate_dispatch2!` (not the older per-protocol `delegate_*` macros).
- New config keys: add to the struct, `Default`, `validate`, `config.example.toml` (a test checks the example matches the defaults) and the README table.
- Phone link: `ble.rs` runs a tokio current-thread runtime on its own thread (the Wayland loop is blocking and sync). The only shared state is `motion::Live` behind a mutex; data older than `STALE_AFTER` or a disconnect reads as zero motion so the dots settle. The phone advertises no name, so it is found by service UUID. `protocol.rs` must keep passing `../AmsDotsMobileController/protocol/vectors.txt` (included at compile time by its tests).
