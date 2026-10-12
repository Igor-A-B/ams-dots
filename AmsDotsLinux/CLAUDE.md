# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Desktop (Wayland/Hyprland) half of a "vehicle motion cues" tool: a particle-style dot generator drawn as a click-through overlay. The phone sender is a separate project in `../AmsDotsMobileController`. `README.md` documents the dot behaviour, the config keys and the Wayland concepts mapped to code.

## Commands
- `cargo run` / `cargo run -- tests/data/drive.csv` — overlay on every monitor (needs a live Wayland session with wlr-layer-shell); Ctrl+C to stop. `--config FILE` overrides `~/.config/ams-dots/config.toml`.
- `cargo test` — all unit tests; `cargo test spring` or `cargo test dots_flow_with_the_felt_inertia` runs a subset by name
- `hyprctl layers` — verify the `ams-dots` surface is on the overlay level

## Architecture
Per frame: replay sample (or zeros) -> `Spring` smooths (lateral, forward) acceleration -> intensity + drift -> `particles::Field` -> `Vec<Dot>` -> `dots::draw` into a `Pixmap` -> `wl_shm` buffer.
- `particles.rs`, `spring.rs`, `dots.rs`, `config.rs`, `replay.rs` are Wayland-free and unit-tested; keep Wayland code in `overlay.rs`.
- `overlay.rs` holds all Wayland state in one `App`, driven by sctk handler traits; each monitor has its own `Overlay` with its own `Sim` (field + spring). Draw only after the compositor's `configure`; request the next frame callback *before* the commit it belongs to, or the animation stalls. `frame_pending` prevents duplicate callback chains.
- Click-through relies on the empty input region set in `App::add_overlay`; do not remove it.
- Dots are painted directly into the `wl_shm` buffer (`PixmapMut::from_bytes`) with no R/B swap, which is only valid while dots are white/black (see `App::draw`).
- Performance depends on partial clearing: `App::slots` records what each reused buffer holds and `draw` clears only those rectangles. Changing how or where dots are painted (e.g. adding glow or new bands) must keep `dots::bounds_by_side` covering every painted pixel, or ghost dots remain on screen.
- Sign conventions: `forward > 0` = speeding up (dots flow down), `lateral > 0` = passengers pushed right (dots flow right); the same conventions apply to the future phone data after fusion.
- sctk 0.21 uses `delegate_dispatch2!` (not the older per-protocol `delegate_*` macros).
- New config keys: add to the struct, `Default`, `validate`, `config.example.toml` (a test checks the example matches the defaults) and the README table.
