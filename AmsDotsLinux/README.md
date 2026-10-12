# AmsDotsLinux

A Linux/Wayland take on the iPhone's *Vehicle Motion Cues*: translucent dots
along the screen edges that react to the vehicle's motion, so your eyes see the
motion your inner ear feels. This crate is the **desktop half**. The phone half
(an Android/Kotlin Multiplatform app that reads the sensors and sends them over
Bluetooth) lives in `../AmsDotsMobileController`.

## Status

| Piece | State |
|---|---|
| Click-through overlay on every monitor, above fullscreen windows | done |
| Dot generator: spawn, drift, shrink away, respawn | done |
| Spring smoothing of the motion signal | done |
| TOML configuration | done |
| Mock motion from a CSV recording | done |
| Receiving real phone data over Bluetooth, converted to vehicle axes | done (tested against golden vectors; not yet tried with the phone) |
| Calibrate command (which way is forward) | done |

## Build and run

Requires a Wayland compositor that supports `wlr-layer-shell` (Hyprland, sway,
river, ...; GNOME does not).

```sh
cargo run                                # follow the phone over Bluetooth (dots rest until it connects)
cargo run -- --no-phone                  # vehicle at rest: dots shimmer in place
cargo run -- tests/data/drive.csv        # replay a mock drive (loops every 30 s)
cargo run -- --config my.toml            # use another config file
cargo run -- --rate 30                   # ask the phone for 30 Hz (10-100; its default is 50)
ams-dots-linux calibrate                 # tell the running overlay which way is forward (see below)
cargo test                               # unit tests (no Wayland needed)
hyprctl layers                           # the overlay shows as namespace "ams-dots", level overlay
```

Stop with Ctrl+C. Optional Hyprland rule to skip the open/close animation:

```
layerrule = noanim, ams-dots
```

## How the dots behave

The dots are a *particle effect*, not a fixed set:

- **Spawn.** New dots are born at random spots in a band along the left and
  right edges. The number alive is `default_amount` at rest, rising to
  `default_amount + motion_extra_amount` as motion gets more intense. A dot is
  born every `lifetime / target` seconds, so the population follows the target
  smoothly.
- **Life.** A dot grows for the first `fade_in` % of its `lifetime`, stays full
  size, then shrinks to nothing during the last `fade_out` % and is removed.
  At rest this alone makes the dots shimmer without going anywhere.
- **Drift.** Dots flow the way a passenger is thrown: speeding up moves them
  down the screen, braking up, a push to the right moves them right, a push to
  the left moves them left. The speed is `flow_gain` x the smoothed
  acceleration, capped at `max_speed`.
- **Intensity** (0 to 1) = smoothed acceleration / `acceleration_full_scale`
  + turning speed / `rotation_full_scale`, capped at 1.
- **Spring.** The raw acceleration is not used directly. A damped spring chases
  it, so sudden changes turn into smooth ramps that overshoot slightly and
  settle, like inertia. `stiffness` sets how tightly it follows, and
  `damping_ratio` below 1 allows overshoot.

## Configuration

Settings are read from `~/.config/ams-dots/config.toml` (or
`$XDG_CONFIG_HOME/ams-dots/config.toml`), or from the file given with
`--config`. All keys are optional, a missing default file means defaults, and a
typo in a key is reported as an error. `config.example.toml` lists every key
with its default and meaning:

| Section | Key | Meaning |
|---|---|---|
| `[dots]` | `size` | maximum dot radius, px |
| | `lifetime` | seconds from birth to disappearance |
| | `default_amount` | dots alive at rest |
| | `motion_extra_amount` | extra dots alive at full intensity |
| | `fade_in`, `fade_out` | % of the lifetime spent growing / shrinking |
| | `edge_band` | spawn band width per side, % of screen width |
| `[motion]` | `acceleration_full_scale` | m/s² counted as full intensity |
| | `rotation_full_scale` | rad/s counted as full intensity |
| | `flow_gain`, `max_speed` | drift speed per m/s², and its cap (px/s) |
| `[spring]` | `stiffness`, `damping_ratio` | smoothing behaviour |
| `[display]` | `fps` | redraws per second |

The file is read once at startup; restart to apply changes.

## Phone over Bluetooth

By default the overlay looks for the phone app (`../AmsDotsMobileController`):
start the app's link on the phone and the laptop finds it by its service UUID,
connects, subscribes and follows it. No pairing is needed. The receiver never
gives up: if the phone leaves or the link drops, the dots settle (no motion
means a vehicle at rest) and it scans again. Messages go to stderr. BlueZ must
be running with a powered adapter (`systemctl status bluetooth`).

**Axes.** The phone sends acceleration and rotation in its own axes plus the
gravity direction; `fusion.rs` converts them: *up* from gravity, *forward* from
a fixed direction in the phone's frame flattened onto the horizontal plane (so
tilting the phone changes nothing), *right* = forward x up. Without calibration
the forward direction is guessed from the pose: the top edge when the phone
lies flat, the back when it stands upright in a holder.

**Calibrate.** To set the real forward direction, with the phone fixed in the
vehicle run `ams-dots-linux calibrate` (bind it to a key, e.g. in Hyprland
`bind = $mod, F9, exec, ams-dots-linux calibrate`), then accelerate gently in a
straight line. The first clear push (over 1 m/s²) is averaged for 1.5 s and
becomes "forward"; braking first would invert it. The overlay prints the result
(`Done`, `TooWeak` or `TimedOut`) on stderr. Calibration is kept while the
overlay runs, so repeat it if the phone is moved.

**Data flow.** A background thread (`ble.rs`) receives notifications, `protocol.rs`
decodes them, `fusion.rs` converts them and `motion.rs` keeps the newest sample;
the overlay reads it once per frame. A packet older than 0.5 s counts as gone.

## Mock motion data

`tests/data/drive.csv` stands in for the phone for development (and replaces it when given on the command line):

```
t,forward,lateral,yaw
```

`t` is seconds; `forward` is m/s² (positive = speeding up); `lateral` is m/s²
(positive = passengers pushed right, as in a left turn); `yaw` is the turning
speed in rad/s (positive = turning left). The bundled drive has: speeding up
(1-4 s), braking (8-10 s), a left turn (12-15 s), a right turn (17-20 s) and a
bumpy road (22-28 s). The same quantities are what the phone data provides
after conversion to the vehicle's frame.

## Code map

| File | Responsibility |
|---|---|
| `src/main.rs` | Command line, chooses the motion source, starts the overlay. |
| `src/config.rs` | TOML settings, defaults and validation. |
| `src/motion.rs` | The motion source (rest, recording or phone) and the shared latest phone sample. |
| `src/replay.rs` | Reads and plays back the CSV recording. |
| `src/protocol.rs` | Phone packet decoder (tested against `protocol/vectors.txt`) and control command. |
| `src/fusion.rs` | Phone axes to vehicle axes, and the calibration. |
| `src/ble.rs` | BlueZ receiver thread: scan, connect, subscribe, reconnect. |
| `src/control.rs` | Unix socket for `ams-dots-linux calibrate`. |
| `src/spring.rs` | The damped spring that smooths the motion. |
| `src/particles.rs` | The dot generator: spawning, ageing, drift, size over life. No drawing. |
| `src/dots.rs` | Paints a list of dots into an image. No Wayland. |
| `src/overlay.rs` | All compositor communication: overlay surfaces, buffers, frame timing. |

Data flow per frame: motion source (phone, recording or zeros) -> spring -> intensity and drift ->
particle field -> list of dots -> image -> shared memory -> compositor.

## How the Wayland part works

Wayland is a protocol between your app (the *client*) and the compositor
(Hyprland, the *server*). Nothing is drawn directly on screen: the app hands
the compositor an image and the compositor composes all images into the final
picture. Terms used in `overlay.rs`:

- **Global**: a feature the compositor advertises at connect time (`wl_compositor`,
  `wl_shm`, `zwlr_layer_shell_v1`, one `wl_output` per monitor). `run()` binds
  the ones we need and exits with an explanation if layer-shell is missing.
- **Surface** (`wl_surface`): a rectangle of pixels the compositor knows about.
  A surface alone has no position or role.
- **Layer surface**: the *role* from the layer-shell protocol that turns a
  surface into a shell-style element (like a bar or wallpaper) on one of four
  layers: background, bottom, top, **overlay**. Overlay is drawn above
  everything, including fullscreen windows. `add_overlay` creates it and sets:
  - anchor to all four edges + size 0x0 = "fill the monitor";
  - `exclusive_zone = -1` = don't reserve space, draw over bars;
  - keyboard interactivity `None` = never take keyboard focus;
  - an **empty input region** = every pixel counts as "not part of this
    surface" for pointer and touch purposes, so clicks reach the window below.
    This is what makes the dots non-clickable.
- **Configure / commit cycle**: the first `commit()` has no image and means
  "I'm ready, tell me my size". The compositor replies with a `configure`
  event (`LayerShellHandler::configure`); only then do we draw. Every later
  change is applied atomically with `commit()`.
- **Frame callback**: before committing, `draw` asks the compositor to notify
  us when it wants the next picture. The notification (`CompositorHandler::frame`)
  triggers the next draw. This paces the animation to the display. It must be
  requested *before* the commit it belongs to, and the notification arrives at
  the monitor's rate, so `display.fps` makes `frame` skip callbacks that come
  too early (it re-requests one with an empty commit).
- **Shared-memory buffer** (`wl_shm`): software rendering. We ask a `SlotPool`
  for a chunk of memory shared with the compositor, paint the dots straight
  into it (`App::draw`), `attach` it to the surface, mark the changed area
  `damage`d, and `commit`. `Argb8888` is premultiplied B,G,R,A while
  `tiny-skia` writes R,G,B,A. The dots are white and black only, for which the
  two layouts are identical, so nothing is converted. Coloured dots would need
  the red and blue channels swapped.
- **Cheap frames**: buffers are reused, so the previous picture is still in
  them. `App::slots` remembers, per buffer, which rectangles held dots, and only
  those are erased before painting (the two dot bands are tracked separately,
  because one rectangle around both would span the whole screen). The damage
  reported to the compositor is the union of where dots were and where they
  are now. Buffers of different monitors can overlap in memory over time, so
  painting a buffer forgets the records of any buffer it overlaps; an unknown
  buffer is always erased completely.
- **Output**: a monitor. `OutputHandler::new_output` fires for each monitor at
  startup and when one is plugged in; `output_destroyed` when removed. The
  program keeps one overlay, with its own dots and spring, per output.

## Known limitations

- **CPU use**: about 6 % of one core for the app with two 1080p monitors at
  30 fps (Hyprland itself spends a few extra percent compositing). Lower
  `display.fps` to reduce it further.
- Fractional scaling (e.g. the laptop panel at 1.333) is not handled: the
  compositor stretches our 1x image, so dots are slightly soft there. Fixing it
  needs the `wp-fractional-scale` and `wp-viewporter` protocols.
- Config is read once; there is no live reload.

## Dependencies

- `smithay-client-toolkit`: safe Rust helpers over the Wayland protocol.
- `tiny-skia`: small CPU 2D renderer used to paint anti-aliased circles.
- `serde`, `toml`: reading the configuration file.
- `bluer` (BlueZ over D-Bus, needs `libdbus` headers to build), `tokio`, `futures`: the Bluetooth receiver.
