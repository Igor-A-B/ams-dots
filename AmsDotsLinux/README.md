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
| Receiving real phone data over Bluetooth, converted to vehicle axes | done, tried on a real phone (13 packets/s, no loss) |
| Pick the phone from a list the first time; later runs connect to the last phone by themselves | done |
| `phones`, `select`, `forget`: list and switch phones while the overlay runs | done |
| Automatic "forward" direction (learned from driving, no calibration step) | done (simulated drives; not yet tried in a car) |
| `status`, `connect`, `disconnect` commands, link messages on stderr | done |

## Quick start

`ams-dots-linux` is not installed anywhere yet, so it is not a command until you
do step 3. All commands below run from this directory (`AmsDotsLinux`).

**1. On the phone** (once; details in `../AmsDotsMobileController/README.md`):
install the app, open it and press **Start** ("Waiting for the laptop").

**2. On the laptop**, with Bluetooth on (`systemctl status bluetooth`):

```sh
cargo run --release
```

The first build takes a minute. **The first time** the app scans for 4 seconds
and lists the phones that run the AmsDots app:

```
  [1] Redmi Note 12 Pro 5G          -60 dBm  6D:1E:7C:C1:40:1C <- Enter
number or Enter to connect, r = rescan, a = first phone that appears, q = quit:
```

Type the number (or Enter for the one marked), and the dots start following that
phone. **From then on it connects to that phone by itself, with no question**:
the last phone chosen or connected is remembered in
`~/.local/state/ams-dots/phone`. The terminal says "connected, receiving
motion". Stop with Ctrl+C; that also disconnects the phone cleanly.

**To use another phone**, while the overlay runs (second terminal):

```sh
ams-dots-linux phones          # phones heard lately, marking the connected and the last one
ams-dots-linux select          # shows the list and asks which one
ams-dots-linux select 2        # or by number from `phones`, by name, or `any`
ams-dots-linux forget          # forget the last phone: the next start asks again
```

or start with `--pick` to see the list at start-up even though a phone is
remembered. `select` also remembers the new phone.

**3. To watch the link** (second terminal, overlay still running):

```sh
cargo run --release -- status --watch
```

One live line: link state, packets per second, loss, age of the newest packet and
the forward / lateral / yaw values the dots get. Shake the phone to see them move.

**Optional: make it a command.** This puts `ams-dots-linux` in `~/.local/bin`
(already on your PATH), after which you can drop `cargo run --release --`:

```sh
cargo install --path . --root ~/.local
ams-dots-linux                      # overlay with phone picker
ams-dots-linux status --watch
ams-dots-linux --help
```

Without installing, the same binary is `./target/release/ams-dots-linux`.

## Build and run, all variants

Requires a Wayland compositor that supports `wlr-layer-shell` (Hyprland, sway,
river, ...; GNOME does not).

```sh
cargo run --release                      # follow the last phone over Bluetooth (first time: pick from a list)
cargo run --release -- --pick            # always show the list of phones first
cargo run --release -- --phone "Redmi Note 12 Pro 5G"   # skip the list (also `--phone any`)
cargo run --release -- --no-phone        # vehicle at rest: dots shimmer in place
cargo run --release -- tests/data/drive.csv   # replay a mock drive (loops every 30 s)
cargo run --release -- --config my.toml  # use another config file
cargo run --release -- --rate 30         # ask the phone for 30 Hz (10-100; its default is 50)
cargo run --release -- status [--watch]  # what the running overlay sees
cargo run --release -- phones | select [N|NAME|any] | forget   # list / switch / forget the phone
cargo run --release -- connect | disconnect | relearn   # control the running overlay
cargo test                               # unit tests (no Wayland, no Bluetooth needed)
hyprctl layers                           # the overlay shows as namespace "ams-dots", level overlay
```

Without a terminal (autostart) there is never a list: the last phone is used, or
the first one found if none was ever chosen (and that one is then remembered).
If the last phone is not in range, the app keeps waiting for *that* phone and
never grabs another one; `status` says `scanning for "NAME"`.

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

The overlay follows the phone app (`../AmsDotsMobileController`): the app
advertises the AmsDots service, the laptop lists the phones that do (picker, see
Quick start; the first time only), connects, subscribes and follows the one you
chose, or the last one it followed. No pairing is
needed. Phones are told apart by their Bluetooth name, because the phone's
random Bluetooth address changes every time its link is started. The receiver never
gives up: if the phone leaves or the link drops, the dots settle (no motion
means a vehicle at rest) and it scans again. Messages go to stderr. BlueZ must
be running with a powered adapter (`systemctl status bluetooth`).

**Interactive.** The overlay tells you what the link is doing: every change is
printed on stderr, e.g.
"phone connected, receiving motion". `ams-dots-linux status --watch` is a live
view in one line (link state, packets per second, loss, age of the newest
packet, and the forward / lateral / yaw values the dots get), so shaking the phone
shows immediately whether the link is responsive. `disconnect` drops the phone
and stops scanning, `connect` starts again. `phones` lists what the receiver
heard in the last 30 s (a connected phone does not advertise, so while one is
connected the list holds that phone plus anything heard just before), and
`select` points the receiver at another phone: the current session ends and a
new one starts for the new phone, which must be advertising (app started). The phone advertises four times a
second, so the laptop notices it within a moment. Measured on a real link, the
newest packet is typically 10-40 ms old when the dots read it, plus up to 60 ms
of packing on the phone and 40 ms of sensor batching; the spring then smooths it.

**Axes.** The phone sends acceleration and rotation in its own axes plus the
gravity direction; `fusion.rs` converts them: *up* from gravity, *forward* from
a fixed direction in the phone's frame flattened onto the horizontal plane (so
tilting the phone changes nothing), *right* = forward x up.

**Forward is learned, not calibrated.** Cars go forward, so the app works out
the direction of travel from the motion itself; there is nothing to press. A
phone has no velocity sensor, so it uses two facts:

1. While going straight (small yaw rate), the horizontal acceleration is mostly
   speeding up and braking, i.e. along the car. After **more than 5 s** of such
   movement, if one direction clearly dominates (80 % of the energy), that is
   the forward/backward axis.
2. In a turn, the centripetal acceleration points to the inside, so
   acceleration times yaw rate points to the car's left. Over 3 s of turning this
   tells the front of the axis from the back (and gives the whole direction if the
   car has only turned so far).

Until the evidence is there, the direction is a guess from the phone's pose (top
edge when it lies flat, back when it stands upright in a holder); `status` says
"guessing ... learning (n of 5 s)". Pothole-sized readings (over 8 m/s²) and
sensor noise (under 0.25 m/s²) are ignored. The evidence fades over about 90 s,
so the estimate follows the phone if it is moved in its mount. Only if the front
end is wrong for a while (e.g. the first drive was in reverse) run `relearn`.
Limits: a car that has never accelerated, braked or turned cannot be learned
from, and a long reverse teaches the opposite direction until it is outweighed.

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
| `src/fusion.rs` | Phone axes to vehicle axes, and learning which way is forward. |
| `src/picker.rs` | The list of phones, the remembered last phone, the start-up decision (ask or connect), matching by name. |
| `src/ble.rs` | BlueZ receiver thread: scan, connect, subscribe, reconnect, clean disconnect on exit. |
| `src/control.rs` | Unix socket for `status`, `phones`, `select`, `forget`, `connect`, `disconnect`, `relearn`. |
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

## Troubleshooting

- **`scanning for "NAME"` never ends.** The last phone is not advertising: start
  the app on it, or run `ams-dots-linux phones` to see what is in range and
  `ams-dots-linux select` to follow another phone. After 10 s the log names the
  other phones in range.
- **The list is empty / "scanning for the phone" never ends.** Is the phone app
  at "Waiting for the laptop"? Is the phone's screen on and Bluetooth on? If
  `bluetoothctl devices Connected` still lists the phone from an earlier run
  that was killed with `kill -9`, run `bluetoothctl disconnect <address>`.
- **BlueZ 5.87 crashes (`bluetoothd` segfault).** It dies when a client asks for
  discovery filtered by service UUID and a matching device appears. This app
  therefore filters in its own code. If `journalctl -u bluetooth` still shows
  `SEGV`, update `bluez` (upstream fixed several crashes after 5.87).
- **Connected but no motion.** The log says so after 8 s: the Bluetooth link
  needs an MTU of at least 64 (the phone negotiates 517 normally).
- Everything the app does is printed on stderr; run it from a terminal.

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
- `bluer` (BlueZ over D-Bus, needs `libdbus` headers to build), `tokio`, `futures`: the Bluetooth receiver. `signal-hook`: disconnect cleanly on Ctrl+C.
