//! Everything that talks to the Wayland compositor.
//!
//! The flow, which the README walks through in detail:
//!
//! 1. `run` connects to the compositor and asks which features ("globals") it
//!    offers: surfaces, layer-shell, shared memory, outputs.
//! 2. For every monitor ("output") we create a *layer surface* on the
//!    `overlay` layer, covering the whole monitor, that ignores all input.
//! 3. The compositor answers each surface with a `configure` event carrying
//!    its real size. Only then do we draw: paint the dots (see `dots.rs`),
//!    copy the pixels into shared memory, attach that buffer, and `commit`.
//! 4. Every draw also asks for a *frame callback*; the compositor answers when
//!    it is ready for the next picture. The answer triggers the next draw, so
//!    the animation runs without timers. Callbacks arrive at the display's
//!    refresh rate (60 or 100 Hz); the `display.fps` setting skips the extra
//!    ones.

use crate::config::{Config, Settings};
use crate::dots::{self, Rect};
use crate::manual::Manual;
use crate::particles::Field;
use crate::motion::MotionSource;
use crate::replay::Sample;
use crate::spring::Spring;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::client::{
        Connection, QueueHandle,
        globals::registry_queue_init,
        protocol::{wl_output, wl_shm, wl_surface},
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tiny_skia::PixmapMut;

/// Name the compositor shows for our surfaces (`hyprctl layers`) and the name
/// to use in Hyprland `layerrule`s.
const NAMESPACE: &str = "ams-dots";
/// Frame callbacks come in at the display rate; drawing slightly early avoids
/// skipping a whole callback when it lands just before the target interval.
const FRAME_SLACK: f32 = 0.004;

/// Connects to the compositor and shows the dots until the process is killed
/// or the compositor closes our surfaces. The dots react to `motion` (the phone
/// or a looping recording) unless `manual` is switched on, and follow `settings`
/// when they are reloaded. `ready` runs once the compositor connection works,
/// before the first frame: the place to open the control socket, so that a start
/// that cannot work (no layer-shell) never looks like a running overlay.
pub fn run(settings: Arc<Settings>, manual: Arc<Manual>, motion: MotionSource, ready: impl FnOnce()) -> Result<(), String> {
    let conn: Connection = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
    let qh: QueueHandle<App> = event_queue.handle();

    let compositor: CompositorState = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let layer_shell: LayerShell = LayerShell::bind(&globals, &qh)
        .map_err(|_| "the compositor does not support wlr-layer-shell (needed for an overlay)".to_string())?;
    let shm: Shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let pool: SlotPool = SlotPool::new(1, &shm).map_err(|e| e.to_string())?;
    let (config, config_generation) = settings.snapshot();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        slots: HashMap::new(),
        slots_pool_len: 0,
        overlays: Vec::new(),
        config,
        config_generation,
        settings,
        manual,
        motion,
        started: Instant::now(),
        exit: false,
    };
    ready();

    // Outputs arrive as events; `new_output` creates one overlay per monitor.
    while !app.exit {
        event_queue.blocking_dispatch(&mut app).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One overlay covering one monitor.
struct Overlay {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    /// Size in logical pixels, known only after the first `configure`.
    size: Option<(u32, u32)>,
    /// Animation state; created together with `size`.
    sim: Option<Sim>,
    /// True while a frame callback is outstanding. Prevents a second `configure`
    /// from starting a second callback chain (which would double the frame rate).
    frame_pending: bool,
}

/// Everything that changes from frame to frame on one monitor.
struct Sim {
    field: Field,
    spring: Spring,
    last_draw: Instant,
    /// Where the previous frame painted (left and right group of dots). The
    /// compositor still shows it, so it must be reported as changed too.
    last_bounds: [Option<Rect>; 2],
}

/// What a shared-memory slot currently contains, so that only that part has
/// to be erased before the slot is painted again.
struct SlotContent {
    width: u32,
    height: u32,
    /// Size of the slot in bytes.
    len: usize,
    painted: [Option<Rect>; 2],
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    /// Shared-memory arena the pixel buffers are carved from; grows on demand.
    pool: SlotPool,
    /// Per slot (identified by its memory address): what it was last painted
    /// with. Valid only while the pool keeps its size (`slots_pool_len`).
    slots: HashMap<usize, SlotContent>,
    slots_pool_len: usize,
    overlays: Vec<Overlay>,
    /// The settings in use, and the [`Settings::generation`] they came from.
    config: Config,
    config_generation: u64,
    settings: Arc<Settings>,
    /// Direction and speed set by hand; replaces `motion` while on.
    manual: Arc<Manual>,
    motion: MotionSource,
    /// Replay clock origin.
    started: Instant,
    exit: bool,
}

impl App {
    /// Creates the click-through overlay surface for one monitor. It is not
    /// visible yet: the compositor must reply with `configure` first.
    fn add_overlay(&mut self, qh: &QueueHandle<App>, output: wl_output::WlOutput) {
        let surface: wl_surface::WlSurface = self.compositor.create_surface(qh);
        let layer: LayerSurface =
            self.layer_shell.create_layer_surface(qh, surface, Layer::Overlay, Some(NAMESPACE), Some(&output));

        // Anchoring to all four edges with size 0x0 means "fill the output".
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        // -1: draw over panels/bars instead of being laid out around them.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        // An empty input region makes every pointer/touch event fall through
        // to the window underneath: the overlay can never be clicked.
        match Region::new(&self.compositor) {
            Ok(empty) => layer.set_input_region(Some(empty.wl_region())),
            Err(err) => eprintln!("ams-dots: cannot make the overlay click-through: {err}"),
        }

        // First commit has no buffer; it asks the compositor to configure us.
        layer.commit();
        self.overlays.push(Overlay { layer, output, size: None, sim: None, frame_pending: false });
    }

    /// Index of the overlay that owns `surface`.
    fn find_overlay(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.overlays.iter().position(|o| o.layer.wl_surface() == surface)
    }

    /// The vehicle motion right now: what was set by hand, or else whatever source is configured.
    fn current_sample(&self) -> Sample {
        let elapsed = self.started.elapsed().as_secs_f32();
        self.manual.sample(elapsed, &self.config.motion).unwrap_or_else(|| self.motion.at(elapsed))
    }

    /// Picks up a reloaded config file: every monitor starts its dots again with the new settings.
    fn refresh_config(&mut self) {
        if self.settings.generation() == self.config_generation {
            return;
        }
        (self.config, self.config_generation) = self.settings.snapshot();
        for index in 0..self.overlays.len() {
            if let Some((width, height)) = self.overlays[index].size {
                self.start_sim(index, width, height);
            }
        }
    }

    /// (Re)creates the animation state for overlay `index` at the given size.
    fn start_sim(&mut self, index: usize, width: u32, height: u32) {
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) + index as u64;
        self.overlays[index].sim = Some(Sim {
            field: Field::new(width, height, &self.config, seed),
            spring: Spring::new(self.config.spring.stiffness, self.config.spring.damping_ratio),
            last_draw: Instant::now(),
            // Nothing was ever shown at this size: the first frame damages everything.
            last_bounds: [Some((0, 0, width as i32, height as i32)), None],
        });
    }

    /// Asks the compositor to tell us when it wants the next picture. Must be
    /// called before the `commit` it should belong to.
    fn request_frame(&mut self, qh: &QueueHandle<App>, index: usize) {
        let overlay: &mut Overlay = &mut self.overlays[index];
        if !overlay.frame_pending {
            let surface: &wl_surface::WlSurface = overlay.layer.wl_surface();
            surface.frame(qh, FrameCallbackData(surface.clone()));
            overlay.frame_pending = true;
        }
    }

    /// Advances the animation of overlay `index` by the time since its last
    /// draw, paints it and presents the result.
    fn draw(&mut self, qh: &QueueHandle<App>, index: usize) {
        self.refresh_config();
        let sample: Sample = self.current_sample();
        let motion_cfg = self.config.motion.clone();

        let overlay: &mut Overlay = &mut self.overlays[index];
        let (width, height) = match overlay.size {
            Some(size) => size,
            None => return,
        };
        let sim: &mut Sim = match overlay.sim.as_mut() {
            Some(sim) => sim,
            None => return,
        };

        let now = Instant::now();
        let dt = now.duration_since(sim.last_draw).as_secs_f32();
        sim.last_draw = now;

        // Smooth the raw acceleration, then derive how intense the motion is.
        sim.spring.step((sample.lateral, sample.forward), dt);
        let smoothed = sim.spring.pos();
        let intensity = smoothed.0.hypot(smoothed.1) / motion_cfg.acceleration_full_scale
            + sample.yaw.abs() / motion_cfg.rotation_full_scale;
        sim.field.step(dt, smoothed, intensity);
        let dots_now: Vec<dots::Dot> = sim.field.dots();

        // If the pool grew since the last frame its memory may have moved, so
        // every remembered slot is forgotten. (Growth happens inside
        // `create_buffer`, hence the check one frame later; an unknown slot is
        // simply cleared completely.)
        if self.pool.len() != self.slots_pool_len {
            self.slots.clear();
            self.slots_pool_len = self.pool.len();
        }

        let stride = width as i32 * 4;
        let (buffer, canvas) =
            match self.pool.create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888) {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("ams-dots: cannot allocate a buffer: {err}");
                    return;
                }
            };

        // Erase what this slot showed last time. A slot is reused over and over,
        // so only the parts that had dots need clearing, not the whole 8 MB.
        // A slot we know nothing about (new, resized pool, other monitor size)
        // is cleared completely.
        let key = canvas.as_ptr() as usize;
        let len = stride as usize * height as usize;
        let known_content: Option<[Option<Rect>; 2]> = match self.slots.get(&key) {
            Some(old) if old.width == width && old.height == height => Some(old.painted),
            _ => None,
        };
        // Slots of different sizes can overlap in memory over time (a monitor
        // with a bigger buffer takes the place of a smaller one). Painting here
        // dirties every overlapping slot in ways we do not track, so forget them.
        self.slots.retain(|other, c| *other + c.len <= key || *other >= key + len);
        match known_content {
            Some(painted) => {
                for rect in painted.iter().flatten() {
                    dots::clear(canvas, width, *rect);
                }
            }
            None => canvas.fill(0),
        }

        // Paint straight into the shared-memory buffer; no intermediate image.
        // tiny-skia writes premultiplied R,G,B,A while wl_shm Argb8888 is
        // B,G,R,A. The dots are only white and black, where swapping red and
        // blue changes nothing. Any coloured dot would need the channels swapped.
        match PixmapMut::from_bytes(canvas, width, height) {
            Some(mut pixmap) => dots::draw(&mut pixmap, &dots_now),
            None => return,
        }

        // Remember what the slot holds now, and tell the compositor what changed
        // on screen: where dots were and where they are.
        let painted = dots::bounds_by_side(&dots_now, width, height);
        self.slots.insert(key, SlotContent { width, height, len, painted });
        let changed: [Option<Rect>; 2] = [
            dots::union(sim.last_bounds[0], painted[0]),
            dots::union(sim.last_bounds[1], painted[1]),
        ];
        sim.last_bounds = painted;

        let surface: &wl_surface::WlSurface = overlay.layer.wl_surface();
        for (x0, y0, x1, y1) in changed.into_iter().flatten() {
            surface.damage_buffer(x0, y0, x1 - x0, y1 - y0);
        }
        // The callback request must come before the commit it should belong to.
        if !overlay.frame_pending {
            surface.frame(qh, FrameCallbackData(surface.clone()));
            overlay.frame_pending = true;
        }
        if buffer.attach_to(surface).is_ok() {
            overlay.layer.commit();
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<App>, layer: &LayerSurface) {
        self.overlays.retain(|o| o.layer.wl_surface() != layer.wl_surface());
        if self.overlays.is_empty() {
            self.exit = true;
        }
    }

    /// The compositor tells us the real size; this is also our cue to draw.
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<App>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if let Some(index) = self.find_overlay(layer.wl_surface()) {
            // Keep the running animation if only a repeated configure arrived.
            if self.overlays[index].size != Some(configure.new_size) || self.overlays[index].sim.is_none() {
                self.overlays[index].size = Some(configure.new_size);
                self.start_sim(index, configure.new_size.0, configure.new_size.1);
            }
            self.draw(qh, index);
        }
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<App>, output: wl_output::WlOutput) {
        self.add_overlay(qh, output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<App>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<App>, output: wl_output::WlOutput) {
        self.overlays.retain(|o| o.output != output);
    }
}

impl CompositorHandler for App {
    /// The compositor is ready for the next picture of this overlay. If the
    /// configured frame rate says it is too early, only ask for the next callback.
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<App>, surface: &wl_surface::WlSurface, _: u32) {
        let index: usize = match self.find_overlay(surface) {
            Some(index) => index,
            None => return,
        };
        self.overlays[index].frame_pending = false;

        let min_interval = 1.0 / self.config.display.fps as f32 - FRAME_SLACK;
        let due = match &self.overlays[index].sim {
            Some(sim) => sim.last_draw.elapsed().as_secs_f32() >= min_interval,
            None => false,
        };
        if due {
            self.draw(qh, index);
        } else {
            self.request_frame(qh, index);
            // A commit without a new buffer is what makes the compositor send the callback.
            self.overlays[index].layer.commit();
        }
    }

    // Not used yet: scaling and rotation are not handled.
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<App>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<App>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<App>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<App>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
