//! The wlr-layer-shell client: one background surface per output.
//!
//! This talks the protocol directly instead of going through iced, and the
//! reason is placement. `iced_layershell` asks for an output *by name*, which
//! it resolves through a cache that silently falls back to "no output" on a
//! miss — the compositor then picks the focused monitor and reports nothing
//! back. That is the bug the bar's whole namespace-verification reconciler
//! exists to catch. `create_layer_surface` here takes a `wl_output` **object**,
//! so a wallpaper cannot land on the wrong screen and none of that machinery is
//! needed. Drawing into `wl_shm` also means no iced, no wgpu and no VRAM for a
//! process that shows a static picture.

use std::collections::HashMap;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::delegate_noop;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};

use crate::decode::{self, Wallpaper};

/// Layer-shell namespace for every surface this process creates.
///
/// One shared namespace is fine: nothing needs to tell the surfaces apart,
/// because placement is guaranteed rather than verified. It shares the
/// `obayebar` prefix so a Hyprland `layerrule = …, ^obayebar` keeps matching,
/// and it can never be confused with the bar's `obayebar-bar-N`.
pub const NAMESPACE: &str = "obayebar-wallpaper";

/// One output and the surface covering it.
struct Output {
    /// Port name, e.g. `DP-9`. `None` until the compositor sends it — a
    /// surface can be created and mapped before the name arrives.
    name: Option<String>,
    layer: LayerSurface,
    /// Size the compositor told us to be, from the configure event. This is in
    /// *surface-local* coordinates, which on a scaled output is smaller than
    /// the panel's real pixel grid.
    size: (u32, u32),
    /// The output's integer buffer scale, never below 1. Only used when there
    /// is no viewport to size the buffer exactly.
    scale: i32,
    /// The panel's real pixel dimensions, from its current mode.
    ///
    /// This is what the buffer should be, and it is usually neither the
    /// surface-local configure size nor that size times the integer scale. On
    /// this machine the panel is 2256x1504 while the configure says 1920x1280
    /// and the advertised scale is 2 — so sizing by `configure * scale` meant
    /// rendering 3840x2560, nearly three times the pixels the display can
    /// show, only for the compositor to scale them back down.
    mode: Option<(u32, u32)>,
    /// Set when the surface has a viewport, which is what allows a buffer of
    /// arbitrary size to be mapped onto the surface's logical size.
    viewport: Option<WpViewport>,
    /// What is currently drawn, so a redraw for an unchanged picture at an
    /// unchanged size can be skipped.
    drawn: Option<Wallpaper>,
    /// What should be drawn, set by the assignment side.
    wanted: Option<std::path::PathBuf>,
    configured: bool,
}

pub struct Renderer {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    /// `None` when the compositor does not offer `wp_viewporter`, in which case
    /// buffers fall back to the integer-scale sizing.
    viewporter: Option<WpViewporter>,
    pool: Option<SlotPool>,
    outputs: HashMap<u32, Output>,
    /// Set when the compositor closes a surface under us, so `run` can stop.
    pub exit: bool,
    /// Set whenever an output appears, goes away, or first reports its name.
    ///
    /// Lets the run loop notice hotplug when it is woken by the wayland event
    /// that caused it, instead of polling `output_names()` on a timer.
    outputs_changed: bool,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer")
            .field("outputs", &self.outputs.len())
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

/// Why the client could not be brought up.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("connecting to the wayland compositor")]
    Connect(#[source] smithay_client_toolkit::reexports::client::ConnectError),
    #[error("initialising the wayland registry")]
    Registry(#[source] smithay_client_toolkit::reexports::client::globals::GlobalError),
    #[error("the compositor does not offer {0}")]
    MissingGlobal(&'static str),
}

impl Renderer {
    /// Connect to the compositor and bind the globals we need.
    ///
    /// # Errors
    ///
    /// Returns [`SetupError`] when there is no compositor to talk to or it does
    /// not implement wlr-layer-shell or `wl_shm`.
    pub fn new() -> Result<
        (
            Self,
            Connection,
            smithay_client_toolkit::reexports::client::EventQueue<Self>,
        ),
        SetupError,
    > {
        let conn = Connection::connect_to_env().map_err(SetupError::Connect)?;
        let (globals, event_queue) = registry_queue_init(&conn).map_err(SetupError::Registry)?;
        let qh = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh)
            .map_err(|_| SetupError::MissingGlobal("wl_compositor"))?;
        let layer_shell = LayerShell::bind(&globals, &qh)
            .map_err(|_| SetupError::MissingGlobal("zwlr_layer_shell_v1"))?;
        let shm = Shm::bind(&globals, &qh).map_err(|_| SetupError::MissingGlobal("wl_shm"))?;
        // Optional. Without it buffers fall back to integer-scale sizing,
        // which costs pixels but still renders correctly.
        let viewporter = globals.bind::<WpViewporter, _, _>(&qh, 1..=1, ()).ok();
        if viewporter.is_none() {
            log::info!("wallpaper: no wp_viewporter, sizing buffers by integer scale");
        }

        Ok((
            Self {
                registry_state: RegistryState::new(&globals),
                output_state: OutputState::new(&globals, &qh),
                compositor,
                layer_shell,
                shm,
                viewporter,
                pool: None,
                outputs: HashMap::new(),
                exit: false,
                outputs_changed: false,
            },
            conn,
            event_queue,
        ))
    }

    /// Whether the output set has changed since this was last asked.
    pub const fn take_outputs_changed(&mut self) -> bool {
        let changed = self.outputs_changed;
        self.outputs_changed = false;
        changed
    }

    /// Port names of every output we currently hold a surface for.
    pub fn output_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .outputs
            .values()
            .filter_map(|o| o.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Point `monitor` at `path`. The picture is decoded and drawn on the next
    /// dispatch, once the output's size is known.
    pub fn assign(&mut self, monitor: &str, path: std::path::PathBuf) {
        for output in self.outputs.values_mut() {
            if output.name.as_deref() == Some(monitor) {
                output.wanted = Some(path);
                return;
            }
        }
        log::debug!("wallpaper: no surface for {monitor} yet, assignment dropped");
    }

    /// Draw every surface whose wanted picture differs from what is on it.
    pub fn refresh(&mut self) {
        let ids: Vec<u32> = self.outputs.keys().copied().collect();
        for id in ids {
            self.draw(id);
        }
    }

    fn create_surface(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput, id: u32) {
        let surface = self.compositor.create_surface(qh);

        // An empty input region, applied before the first commit because the
        // region is double-buffered. Without it a full-screen surface's input
        // region defaults to infinite and the wallpaper swallows every click,
        // drag and scroll on the bare desktop.
        match Region::new(&self.compositor) {
            Ok(region) => surface.set_input_region(Some(region.wl_region())),
            Err(e) => log::warn!("wallpaper: cannot create an empty input region ({e})"),
        }

        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some(NAMESPACE),
            Some(output),
        );

        // All four anchors plus a zero size is how you ask for "exactly this
        // output": the compositor fills in the real dimensions and reports them
        // in the configure. Hardcoding the size we read from hyprctl would race
        // mode changes and hotplug. A zero dimension *without* the opposing
        // anchors is a protocol error, so the two go together.
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        // -1 means "ignore other surfaces' exclusive zones", which is what puts
        // the wallpaper under the bar rather than beside it. The default of 0
        // would have the compositor shift it clear of the bar's 54px.
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();

        let viewport = self
            .viewporter
            .as_ref()
            .map(|v| v.get_viewport(layer.wl_surface(), qh, ()));

        self.outputs.insert(
            id,
            Output {
                name: None,
                layer,
                size: (0, 0),
                scale: 1,
                mode: None,
                viewport,
                drawn: None,
                wanted: None,
                configured: false,
            },
        );
    }

    /// Draw one output, if it is configured and its picture has changed.
    fn draw(&mut self, id: u32) {
        let Some(output) = self.outputs.get(&id) else {
            return;
        };
        if !output.configured {
            return;
        }
        let (logical_w, logical_h) = output.size;
        if logical_w == 0 || logical_h == 0 {
            return;
        }
        // Prefer the panel's real mode: it is the only size that is neither
        // starved of detail nor wasteful. Sizing by `configure * integer scale`
        // instead meant rendering 3840x2560 for a 2256x1504 panel — nearly
        // three times the pixels, and the resize is around 92% of the time it
        // takes to put a wallpaper up. A viewport is what lets the buffer be
        // that size without the surface changing shape.
        let sized_by_mode = output.viewport.as_ref().and(output.mode);
        let (width, height) = if let Some(mode) = sized_by_mode {
            mode
        } else {
            let scale = u32::try_from(output.scale.max(1)).unwrap_or(1);
            let (Some(w), Some(h)) = (logical_w.checked_mul(scale), logical_h.checked_mul(scale))
            else {
                log::warn!("wallpaper: {logical_w}x{logical_h} at scale {scale} overflows");
                return;
            };
            (w, h)
        };
        let Some(wanted) = output.wanted.clone() else {
            return;
        };
        // Already showing this picture at this size.
        if output
            .drawn
            .as_ref()
            .is_some_and(|d| d.path == wanted && d.width == width && d.height == height)
        {
            return;
        }

        let wallpaper = match decode::prepare(&wanted, width, height) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("wallpaper: {e}");
                // Clear the request so a broken file is not retried on every
                // dispatch; the next rotation will pick something else.
                if let Some(output) = self.outputs.get_mut(&id) {
                    output.wanted = None;
                }
                return;
            }
        };

        if let Err(e) = self.present(id, &wallpaper) {
            log::warn!("wallpaper: {e}");
            return;
        }
        if let Some(output) = self.outputs.get_mut(&id) {
            output.drawn = Some(wallpaper);
        }
    }

    /// Copy the pixels into a shm buffer and put it on screen.
    fn present(&mut self, id: u32, wallpaper: &Wallpaper) -> Result<(), PresentError> {
        let stride = i32::try_from(wallpaper.width)
            .ok()
            .and_then(|w| w.checked_mul(4))
            .ok_or(PresentError::Oversized)?;
        let width = i32::try_from(wallpaper.width).map_err(|_| PresentError::Oversized)?;
        let height = i32::try_from(wallpaper.height).map_err(|_| PresentError::Oversized)?;

        if self.pool.is_none() {
            let initial = wallpaper.bgra.len().max(1);
            self.pool =
                Some(SlotPool::new(initial, &self.shm).map_err(|_| PresentError::PoolCreate)?);
        }
        let pool = self.pool.as_mut().ok_or(PresentError::PoolCreate)?;

        let (buffer, canvas) = pool
            .create_buffer(width, height, stride, wl_shm::Format::Argb8888)
            .map_err(|_| PresentError::BufferCreate)?;
        let Some(slot) = canvas.get_mut(..wallpaper.bgra.len()) else {
            return Err(PresentError::BufferTooSmall);
        };
        slot.copy_from_slice(&wallpaper.bgra);

        let Some(output) = self.outputs.get(&id) else {
            return Err(PresentError::OutputGone);
        };
        let surface = output.layer.wl_surface();
        match output.viewport.as_ref() {
            // The viewport maps whatever size the buffer is onto the surface's
            // logical size, so the two no longer have to be related by an
            // integer. Buffer scale must stay 1 — the two mechanisms would
            // otherwise multiply.
            Some(viewport) => {
                surface.set_buffer_scale(1);
                let (logical_w, logical_h) = output.size;
                if let (Ok(w), Ok(h)) = (i32::try_from(logical_w), i32::try_from(logical_h)) {
                    viewport.set_destination(w, h);
                }
            }
            None => surface.set_buffer_scale(output.scale.max(1)),
        }

        // Declaring the whole surface opaque lets the compositor skip whatever
        // is behind it. A wallpaper always covers its output completely.
        // The region is in surface-local coordinates, not buffer pixels.
        match Region::new(&self.compositor) {
            Ok(region) => {
                let (logical_w, logical_h) = output.size;
                region.add(
                    0,
                    0,
                    i32::try_from(logical_w).unwrap_or(i32::MAX),
                    i32::try_from(logical_h).unwrap_or(i32::MAX),
                );
                surface.set_opaque_region(Some(region.wl_region()));
            }
            Err(e) => log::debug!("wallpaper: no opaque region ({e})"),
        }

        surface.damage_buffer(0, 0, width, height);
        buffer
            .attach_to(surface)
            .map_err(|_| PresentError::Attach)?;
        output.layer.commit();
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum PresentError {
    #[error("the image is too large to describe to the compositor")]
    Oversized,
    #[error("creating the shm pool")]
    PoolCreate,
    #[error("creating an shm buffer")]
    BufferCreate,
    #[error("the shm buffer came back smaller than the image")]
    BufferTooSmall,
    #[error("the output went away mid-draw")]
    OutputGone,
    #[error("attaching the buffer")]
    Attach,
}

impl CompositorHandler for Renderer {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // The buffer is already sized to the output's own pixels, so a scale
        // change does not require re-rendering at a different resolution.
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // Nothing animates; a wallpaper is drawn once per assignment.
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Renderer {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let Some(info) = self.output_state.info(&output) else {
            log::warn!("wallpaper: a new output arrived with no info, skipping");
            return;
        };
        log::info!(
            "wallpaper: output {} ({}) appeared",
            info.name.as_deref().unwrap_or("unnamed"),
            info.id
        );
        self.create_surface(qh, &output, info.id);
        if let Some(entry) = self.outputs.get_mut(&info.id) {
            entry.mode = current_mode(&info);
            entry.scale = info.scale_factor.max(1);
            entry.name = info.name;
        }
        self.outputs_changed = true;
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // The name can arrive after the surface was created, which is exactly
        // why the surface is bound to the output *object* and never to a name.
        if let Some(info) = self.output_state.info(&output) {
            if let Some(entry) = self.outputs.get_mut(&info.id) {
                // The name can arrive after the surface exists, and an
                // assignment addressed to a nameless output was dropped, so
                // this counts as a change worth re-running selection for.
                let named = entry.name.is_none() && info.name.is_some();
                entry.mode = current_mode(&info);
                entry.scale = info.scale_factor.max(1);
                entry.name = info.name;
                if named {
                    self.outputs_changed = true;
                }
            }
        }
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(info) = self.output_state.info(&output) {
            if self.outputs.remove(&info.id).is_some() {
                log::info!(
                    "wallpaper: output {} went away",
                    info.name.as_deref().unwrap_or("unnamed")
                );
                self.outputs_changed = true;
            }
        }
    }
}

impl LayerShellHandler for Renderer {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // The compositor closed one of ours. Drop it rather than exiting: the
        // other screens should keep their wallpaper.
        self.outputs.retain(|_, o| &o.layer != layer);
        if self.outputs.is_empty() {
            log::warn!("wallpaper: every surface was closed by the compositor");
            self.exit = true;
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let mut target = None;
        for (id, output) in &mut self.outputs {
            if &output.layer == layer {
                output.size = configure.new_size;
                output.configured = true;
                target = Some(*id);
                break;
            }
        }
        if let Some(id) = target {
            self.draw(id);
        }
    }
}

impl ShmHandler for Renderer {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Renderer {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Renderer);
delegate_output!(Renderer);
delegate_shm!(Renderer);
delegate_layer!(Renderer);
delegate_registry!(Renderer);

// wp_viewporter and wp_viewport have no events, so there is nothing to
// dispatch and no handler worth writing.
delegate_noop!(Renderer: ignore WpViewporter);
delegate_noop!(Renderer: ignore WpViewport);

/// The output's current mode in real pixels, if it reported one.
fn current_mode(info: &smithay_client_toolkit::output::OutputInfo) -> Option<(u32, u32)> {
    let (w, h) = info
        .modes
        .iter()
        .find(|m| m.current)
        .map(|m| m.dimensions)?;
    Some((u32::try_from(w).ok()?, u32::try_from(h).ok()?))
}
