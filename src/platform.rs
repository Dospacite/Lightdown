//! The Wayland side: window, input, clipboard and presenting frames.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::time::Instant;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::data_device_manager::data_device::{DataDevice, DataDeviceHandler};
use smithay_client_toolkit::data_device_manager::data_offer::{DataOfferHandler, DragOffer};
use smithay_client_toolkit::data_device_manager::data_source::{CopyPasteSource, DataSourceHandler};
use smithay_client_toolkit::data_device_manager::{DataDeviceManagerState, ReadPipe, WritePipe};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::primary_selection::PrimarySelectionManagerState;
use smithay_client_toolkit::primary_selection::device::{PrimarySelectionDevice, PrimarySelectionDeviceHandler};
use smithay_client_toolkit::primary_selection::selection::{PrimarySelectionSource, PrimarySelectionSourceHandler};
use smithay_client_toolkit::reexports::calloop::ping::make_ping;
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, PostAction};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::wl_data_device::WlDataDevice;
use smithay_client_toolkit::reexports::client::protocol::wl_data_device_manager::DndAction;
use smithay_client_toolkit::reexports::client::protocol::wl_data_source::WlDataSource;
use smithay_client_toolkit::reexports::client::protocol::wl_keyboard::WlKeyboard;
use smithay_client_toolkit::reexports::client::protocol::wl_output::{Transform, WlOutput};
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::WlPointer;
use smithay_client_toolkit::reexports::client::protocol::wl_seat::WlSeat;
use smithay_client_toolkit::reexports::client::protocol::wl_shm;
use smithay_client_toolkit::reexports::client::protocol::wl_surface::WlSurface;
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use smithay_client_toolkit::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1;
use smithay_client_toolkit::reexports::protocols::wp::primary_selection::zv1::client::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers};
use smithay_client_toolkit::seat::pointer::{
    AxisScroll, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec,
    ThemedPointer,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    DecorationMode, Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

use crate::app::{App, Edge, Key, Mods, Zone};
use crate::gfx::Canvas;

mod text_input;

const TEXT_MIMES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];

/// What file managers offer for files dragged out of them.
const URI_LIST: &str = "text/uri-list";

/// User data for protocol objects whose events need no per-object state.
struct Plain;

struct State {
    app: App,
    /// Launch time, until the first frame has been presented.
    start: Option<Instant>,
    conn: Connection,
    qh: QueueHandle<State>,
    loop_handle: LoopHandle<'static, State>,
    registry: RegistryState,
    compositor: CompositorState,
    seats: SeatState,
    outputs: OutputState,
    shm: Shm,
    data: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    copy_source: Option<CopyPasteSource>,
    clipboard: String,
    clipboard_html: Option<String>,
    /// The primary selection: what was last selected with the pointer,
    /// which the middle button pastes.
    primaries: Option<PrimarySelectionManagerState>,
    primary_device: Option<PrimarySelectionDevice>,
    primary_source: Option<PrimarySelectionSource>,
    primary: String,
    window: Window,
    viewport: Option<WpViewport>,
    pool: SlotPool,
    buffer: Option<(Buffer, u32, u32)>,
    ime: text_input::InputMethod,
    seat: Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<ThemedPointer>,
    /// Logical window size.
    size: (u32, u32),
    /// Scale in 120ths, as the fractional-scale protocol reports it.
    scale: u32,
    fractional: bool,
    configured: bool,
    frame_pending: bool,
    force_csd: bool,
    mods: Mods,
    serial: u32,
    pointer_at: (f32, f32),
    click: (u32, f32, f32, u32),
    icon: Option<CursorIcon>,
}

pub fn run(app: App, start: Instant) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init(&conn)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<State> = EventLoop::try_new()?;
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle())?;

    let compositor = CompositorState::bind(&globals, &qh)?;
    let xdg = XdgShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;
    let surface = compositor.create_surface(&qh);

    // With both protocols the compositor tells us its exact scale and we
    // render at it; otherwise we fall back to integer buffer scales.
    let viewporter = globals.bind::<WpViewporter, State, Plain>(&qh, 1..=1, Plain).ok();
    let scaler = globals.bind::<WpFractionalScaleManagerV1, State, Plain>(&qh, 1..=1, Plain).ok();
    let viewport = match (&viewporter, &scaler) {
        (Some(v), Some(s)) => {
            s.get_fractional_scale(&surface, &qh, Plain);
            Some(v.get_viewport(&surface, &qh, Plain))
        }
        _ => None,
    };

    let window = xdg.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title(app.title());
    window.set_app_id("lightdown");
    window.set_min_size(Some((320, 200)));
    window.commit();

    let mut state = State {
        app,
        start: Some(start),
        conn: conn.clone(),
        qh: qh.clone(),
        loop_handle: event_loop.handle(),
        registry: RegistryState::new(&globals),
        compositor,
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        pool: SlotPool::new(1 << 20, &shm)?,
        shm,
        data: DataDeviceManagerState::bind(&globals, &qh).ok(),
        data_device: None,
        copy_source: None,
        clipboard: String::new(),
        clipboard_html: None,
        primaries: PrimarySelectionManagerState::bind(&globals, &qh).ok(),
        primary_device: None,
        primary_source: None,
        primary: String::new(),
        window,
        fractional: viewport.is_some(),
        viewport,
        buffer: None,
        ime: text_input::bind(&globals,&qh),
        seat: None,
        keyboard: None,
        pointer: None,
        size: (880, 1000),
        scale: 120,
        configured: false,
        frame_pending: false,
        force_csd: std::env::var_os("LIGHTDOWN_CSD").is_some(),
        mods: Mods::default(),
        serial: 0,
        pointer_at: (0.0, 0.0),
        click: (0, 0.0, 0.0, 0),
        icon: None,
    };

    // Images, the Python interpreter and the agent work on other threads,
    // which ping the loop when they have something to show.
    let (ping, pinged) = make_ping()?;
    event_loop.handle().insert_source(pinged, |_, _, state| state.app.poll())?;
    state.app.set_waker(move || ping.ping());

    while !state.app.exit {
        // An animation on show wakes the loop when its next frame is due,
        // unless the compositor has yet to take the last one.
        let animating = state.app.next_frame().filter(|_| state.configured && !state.frame_pending);
        event_loop.dispatch(
            Some(animating.map_or(std::time::Duration::from_secs(1), |at| {
                at.saturating_duration_since(Instant::now()).min(std::time::Duration::from_secs(1))
            })),
            &mut state,
        )?;
        if animating.is_some_and(|at| at <= Instant::now()) {
            state.app.redraw = true;
        }
        state.app.tick();
        state.serve();
    }
    if state.app.wait_cancelled {
        return Err("editor command aborted with unsaved edits".into());
    }
    Ok(())
}

impl State {
    fn factor(&self) -> f32 {
        self.scale as f32 / 120.0
    }

    /// Carries out whatever the editor asked for while handling events.
    fn serve(&mut self) {
        self.input_sync();
        if let Some(text) = self.app.copy.take() {
            let html = self.app.copy_html.take();
            self.copy(text, html);
        }
        if std::mem::take(&mut self.app.paste) {
            self.paste();
        }
        if std::mem::take(&mut self.app.clipboard_read) {
            self.read_clipboard();
        }
        if let Some(text) = self.app.primary.take() {
            self.set_primary(text);
        }
        if std::mem::take(&mut self.app.paste_primary) {
            self.paste_primary();
        }
        if std::mem::take(&mut self.app.retitle) {
            self.window.set_title(self.app.title());
        }
        if self.app.redraw && self.configured && !self.frame_pending {
            self.draw();
        }
    }

    fn draw(&mut self) {
        let drawing = Instant::now();
        let (lw, lh) = self.size;
        let w = (lw * self.scale + 60) / 120;
        let h = (lh * self.scale + 60) / 120;
        self.app.resize(w, h, self.factor());
        self.app.redraw = false;

        if self.buffer.as_ref().is_some_and(|b| (b.1, b.2) != (w, h)) {
            self.buffer = None;
        }
        let format = wl_shm::Format::Xrgb8888;
        let (iw, ih) = (w as i32, h as i32);
        let fresh = |pool: &mut SlotPool| pool.create_buffer(iw, ih, iw * 4, format).expect("shm buffer").0;
        let (buffer, _, _) = self.buffer.get_or_insert_with(|| (fresh(&mut self.pool), w, h));
        if self.pool.canvas(buffer).is_none() {
            // The compositor still holds the last frame; draw into another.
            *buffer = fresh(&mut self.pool);
        }
        let bytes = self.pool.canvas(buffer).expect("fresh buffer is free");
        // SAFETY: u32 has no invalid bit patterns; alignment is checked.
        let (head, px, _) = unsafe { bytes.align_to_mut::<u32>() };
        assert!(head.is_empty(), "shm buffer is not aligned to pixels");
        self.app.draw(&mut Canvas::new(px, iw, ih));

        let surface = self.window.wl_surface();
        match &self.viewport {
            Some(v) => v.set_destination(lw as i32, lh as i32),
            None => surface.set_buffer_scale((self.scale / 120) as i32),
        }
        surface.damage_buffer(0, 0, iw, ih);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        buffer.attach_to(surface).expect("attach buffer");
        self.window.commit();
        self.frame_pending = true;

        if let Some(t) = self.start.take() {
            if std::env::var_os("LIGHTDOWN_TIMING").is_some() {
                eprintln!("first frame after {:.1?} (drawn in {:.1?})", t.elapsed(), drawing.elapsed());
            }
        }
    }

    fn set_scale(&mut self, scale: u32) {
        if scale != self.scale && scale >= 60 {
            self.scale = scale;
            self.app.redraw = true;
        }
    }

    fn copy(&mut self, text: String, html: Option<String>) {
        let (Some(manager), Some(device)) = (&self.data, &self.data_device) else { return };
        let mut mimes = TEXT_MIMES.to_vec();
        if html.is_some() { mimes.push("text/html"); }
        let source = manager.create_copy_paste_source(&self.qh, mimes);
        source.set_selection(device, self.serial);
        self.copy_source = Some(source);
        self.clipboard = text;
        self.clipboard_html = html;
    }

    /// Reads a pipe to its end without blocking the window, then hands
    /// what came through it to `done`.
    fn read_all(&mut self, pipe: ReadPipe, done: impl FnOnce(&mut State, Vec<u8>) + 'static) {
        let (mut got, mut done) = (Vec::new(), Some(done));
        let _ = self.loop_handle.insert_source(pipe, move |_, file, state| {
            // SAFETY: the file is only read, never closed or replaced.
            let file: &mut File = unsafe { file.get_mut() };
            let mut chunk = [0u8; 1 << 16];
            match file.read(&mut chunk) {
                Ok(0) => {
                    if let Some(done) = done.take() {
                        done(state, std::mem::take(&mut got));
                    }
                    PostAction::Remove
                }
                Ok(n) => {
                    if got.len() + n > 32 * 1024 * 1024 {
                        return PostAction::Remove;
                    }
                    got.extend_from_slice(&chunk[..n]);
                    PostAction::Continue
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) => {
                    PostAction::Continue
                }
                Err(_) => PostAction::Remove,
            }
        });
    }

    fn paste(&mut self) {
        let csv = std::mem::take(&mut self.app.paste_table);
        let plain = std::mem::take(&mut self.app.paste_plain) || csv;
        if self.copy_source.is_some() {
            // The selection is ours; no need to round-trip through a pipe.
            if !plain && self.app.accepts_markdown_paste() && let Some(html) = self.clipboard_html.clone() { return self.app.paste_html(&html); }
            let text = self.clipboard.clone();
            return if csv { self.app.paste_csv_text(&text) } else if plain { self.app.paste_plain_text(&text) } else { self.app.paste_text(&text) };
        }
        let Some(offer) = self.data_device.as_ref().and_then(|d| d.data().selection_offer()) else { return };
        if !plain && (self.app.accepts_screenshot() || self.app.accepts_markdown_paste()) && offer.with_mime_types(|have| have.iter().any(|m| m == "image/png")) {
            if let Ok(pipe) = offer.receive("image/png".into()) {
                self.read_all(pipe, |state, bytes| state.app.paste_image(&bytes));
            }
            return;
        }
        if !plain && self.app.accepts_markdown_paste() && offer.with_mime_types(|have| have.iter().any(|m| m == "text/html")) {
            if let Ok(pipe) = offer.receive("text/html".into()) {
                self.read_all(pipe, |state, bytes| state.app.paste_html(&String::from_utf8_lossy(&bytes)));
            }
            return;
        }
        let mime = offer.with_mime_types(|have| TEXT_MIMES.iter().find(|m| have.iter().any(|h| h == *m)).copied());
        let Some(pipe) = mime.and_then(|m| offer.receive(m.to_string()).ok()) else { return };
        self.read_all(pipe, move |state, bytes| {
            let text = String::from_utf8_lossy(&bytes);
            if csv { state.app.paste_csv_text(&text); } else if plain { state.app.paste_plain_text(&text); } else { state.app.paste_text(&text); }
        });
    }

    /// Hands the clipboard's text to the editor, without pasting it; empty
    /// if there is none.
    fn read_clipboard(&mut self) {
        if self.copy_source.is_some() {
            let text = self.clipboard.clone();
            return self.app.clipboard_arrived(text);
        }
        let offer = self.data_device.as_ref().and_then(|d| d.data().selection_offer());
        let mime = offer.as_ref().and_then(|o| o.with_mime_types(|have| TEXT_MIMES.iter().find(|m| have.iter().any(|h| h == *m)).copied()));
        match offer.zip(mime).and_then(|(o, m)| o.receive(m.to_string()).ok()) {
            Some(pipe) => self.read_all(pipe, |state, bytes| state.app.clipboard_arrived(String::from_utf8_lossy(&bytes).into_owned())),
            None => self.app.clipboard_arrived(String::new()),
        }
    }

    fn set_primary(&mut self, text: String) {
        let (Some(manager), Some(device)) = (&self.primaries, &self.primary_device) else { return };
        let source = manager.create_selection_source(&self.qh, TEXT_MIMES);
        source.set_selection(device, self.serial);
        self.primary_source = Some(source);
        self.primary = text;
    }

    fn paste_primary(&mut self) {
        if self.primary_source.is_some() {
            let text = self.primary.clone();
            return self.app.paste_text(&text);
        }
        let Some(offer) = self.primary_device.as_ref().and_then(|d| d.data().selection_offer()) else { return };
        let mime = offer.with_mime_types(|have| TEXT_MIMES.iter().find(|m| have.iter().any(|h| h == *m)).copied());
        let Some(pipe) = mime.and_then(|m| offer.receive(m.to_string()).ok()) else { return };
        self.read_all(pipe, |state, bytes| state.app.paste_text(&String::from_utf8_lossy(&bytes)));
    }

    fn key(&mut self, event: KeyEvent, serial: u32) {
        self.serial = serial;
        let key = match event.keysym {
            Keysym::Left | Keysym::KP_Left => Key::Left,
            Keysym::Right | Keysym::KP_Right => Key::Right,
            Keysym::Up | Keysym::KP_Up => Key::Up,
            Keysym::Down | Keysym::KP_Down => Key::Down,
            Keysym::Home | Keysym::KP_Home => Key::Home,
            Keysym::End | Keysym::KP_End => Key::End,
            Keysym::Page_Up | Keysym::KP_Page_Up => Key::PageUp,
            Keysym::Page_Down | Keysym::KP_Page_Down => Key::PageDown,
            Keysym::BackSpace => Key::Backspace,
            Keysym::Delete | Keysym::KP_Delete => Key::Delete,
            Keysym::Return | Keysym::KP_Enter => Key::Enter,
            Keysym::Tab | Keysym::ISO_Left_Tab => Key::Tab,
            Keysym::Escape => Key::Escape,
            sym if (Keysym::F1.raw()..=Keysym::F12.raw()).contains(&sym.raw()) => {
                Key::F((sym.raw() - Keysym::F1.raw() + 1) as u8)
            }
            sym => sym.key_char().map_or(Key::Other, |c| Key::Char(c.to_ascii_lowercase())),
        };
        self.app.key(key, self.mods, event.utf8.as_deref());
    }

    fn set_icon(&mut self, zone: Zone) {
        let icon = match zone {
            Zone::Text => CursorIcon::Text,
            Zone::Link | Zone::Close | Zone::Minimize | Zone::Maximize | Zone::Button => CursorIcon::Pointer,
            Zone::Plain | Zone::Title => CursorIcon::Default,
            Zone::Divider => CursorIcon::ColResize,
            Zone::RowDivider => CursorIcon::RowResize,
            Zone::Resize(Edge::Top) => CursorIcon::NResize,
            Zone::Resize(Edge::Bottom) => CursorIcon::SResize,
            Zone::Resize(Edge::Left) => CursorIcon::WResize,
            Zone::Resize(Edge::Right) => CursorIcon::EResize,
            Zone::Resize(Edge::TopLeft) => CursorIcon::NwResize,
            Zone::Resize(Edge::TopRight) => CursorIcon::NeResize,
            Zone::Resize(Edge::BottomLeft) => CursorIcon::SwResize,
            Zone::Resize(Edge::BottomRight) => CursorIcon::SeResize,
        };
        if self.icon != Some(icon) {
            if let Some(p) = &self.pointer {
                let _ = p.set_cursor(&self.conn, icon);
            }
            self.icon = Some(icon);
        }
    }

    fn press(&mut self, time: u32, serial: u32) {
        self.serial = serial;
        let (x, y) = self.pointer_at;
        // Presses close together in time and place count as a multi-click.
        let (t0, x0, y0, n) = self.click;
        let near = (x - x0).abs() < 5.0 && (y - y0).abs() < 5.0;
        let clicks = if near && time.wrapping_sub(t0) < 400 { n % 3 + 1 } else { 1 };
        self.click = (time, x, y, clicks);

        let zone = self.app.zone(x, y, self.mods.ctrl);
        let Some(seat) = &self.seat else { return };
        match zone {
            Zone::Text | Zone::Link | Zone::Button | Zone::Divider | Zone::RowDivider | Zone::Plain => {
                self.app.press(x, y, self.mods, clicks)
            }
            Zone::Close => self.app.close(),
            Zone::Minimize => self.window.set_minimized(),
            Zone::Maximize => {
                if self.app.maximized {
                    self.window.unset_maximized();
                } else {
                    self.window.set_maximized();
                }
            }
            Zone::Title if clicks == 2 => {
                if self.app.maximized {
                    self.window.unset_maximized();
                } else {
                    self.window.set_maximized();
                }
            }
            Zone::Title => self.window.move_(seat, serial),
            Zone::Resize(edge) => {
                let edge = match edge {
                    Edge::Top => ResizeEdge::Top,
                    Edge::Bottom => ResizeEdge::Bottom,
                    Edge::Left => ResizeEdge::Left,
                    Edge::Right => ResizeEdge::Right,
                    Edge::TopLeft => ResizeEdge::TopLeft,
                    Edge::TopRight => ResizeEdge::TopRight,
                    Edge::BottomLeft => ResizeEdge::BottomLeft,
                    Edge::BottomRight => ResizeEdge::BottomRight,
                };
                self.window.resize(seat, serial, edge);
            }
        }
    }

    /// A press of the right or middle button.
    fn press_other(&mut self, button: u8, serial: u32) {
        self.serial = serial;
        let (x, y) = self.pointer_at;
        // On a title bar of our own, the right button brings up the
        // compositor's menu for the window, as on any other.
        if button == 2 && self.app.zone(x, y, false) == Zone::Title {
            if let Some(seat) = &self.seat {
                let scale = self.factor();
                self.window.show_window_menu(seat, serial, ((x / scale) as i32, (y / scale) as i32));
            }
            return;
        }
        self.app.press_button(x, y, button);
    }
}

impl Dispatch2<WpFractionalScaleV1, State> for Plain {
    fn event(
        &self,
        state: &mut State,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.set_scale(scale);
        }
    }
}

macro_rules! no_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch2<$proxy, State> for Plain {
            fn event(&self, _: &mut State, _: &$proxy, _: <$proxy as Proxy>::Event, _: &Connection, _: &QueueHandle<State>) {}
        }
    )*};
}
no_events!(WpViewporter, WpViewport, WpFractionalScaleManagerV1);

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, factor: i32) {
        if !self.fractional {
            self.set_scale(factor.max(1) as u32 * 120);
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {
        self.frame_pending = false;
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: &WlOutput) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl WindowHandler for State {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.app.close();
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window, configure: WindowConfigure, _: u32) {
        // Without a size from the compositor, keep ours, within its bounds.
        let (bw, bh) = configure.suggested_bounds.filter(|b| b.0 > 0 && b.1 > 0).unwrap_or((u32::MAX, u32::MAX));
        self.size = (
            configure.new_size.0.map_or(self.size.0.min(bw), |v| v.get()),
            configure.new_size.1.map_or(self.size.1.min(bh), |v| v.get()),
        );
        self.app.csd = self.force_csd || configure.decoration_mode == DecorationMode::Client;
        self.app.maximized = configure.is_maximized() || configure.is_fullscreen() || configure.is_tiled();
        self.app.tiled = configure.is_tiled();
        self.app.redraw = true;
        self.configured = true;
        // A configure must be answered with a commit even mid-frame.
        self.frame_pending = false;
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: WlSeat, capability: Capability) {
        if self.seat.is_none() {
            self.input_seat(&seat);
            self.data_device = self.data.as_ref().map(|d| d.get_data_device(qh, &seat));
            self.primary_device = self.primaries.as_ref().map(|p| p.get_selection_device(qh, &seat));
            self.seat = Some(seat.clone());
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            let repeat = Box::new(|state: &mut State, _: &WlKeyboard, event: KeyEvent| {
                let serial = state.serial;
                state.key(event, serial);
            });
            self.keyboard = self.seats.get_keyboard_with_repeat(qh, &seat, None, self.loop_handle.clone(), repeat).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            self.pointer = self
                .seats
                .get_pointer_with_theme::<State, ()>(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default())
                .ok();
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat, capability: Capability) {
        match capability {
            Capability::Keyboard => {
                if let Some(k) = self.keyboard.take() {
                    k.release();
                }
            }
            Capability::Pointer => self.pointer = None,
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl KeyboardHandler for State {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.app.attend();
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: &WlSurface, _: u32) { self.app.focus_lost(); }

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, serial: u32, event: KeyEvent) {
        self.key(event, serial);
    }

    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, serial: u32, event: KeyEvent) {
        self.key(event, serial);
    }

    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlKeyboard, _: u32, _: KeyEvent) {}

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        m: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.mods = Mods { ctrl: m.ctrl, shift: m.shift, alt: m.alt };
        self.app.mods = self.mods;
    }
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlPointer, events: &[PointerEvent]) {
        let scale = self.factor();
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            let (x, y) = (event.position.0 as f32 * scale, event.position.1 as f32 * scale);
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    if matches!(event.kind, PointerEventKind::Enter { .. }) {
                        self.icon = None;
                    }
                    self.pointer_at = (x, y);
                    self.app.motion(x, y);
                    self.set_icon(self.app.zone(x, y, self.mods.ctrl));
                }
                PointerEventKind::Press { button: BTN_LEFT, time, serial } => {
                    self.pointer_at = (x, y);
                    self.press(time, serial);
                }
                PointerEventKind::Release { button: BTN_LEFT, .. } => self.app.release(),
                PointerEventKind::Press { button: b @ (BTN_RIGHT | BTN_MIDDLE), serial, .. } => {
                    self.pointer_at = (x, y);
                    self.press_other(if b == BTN_RIGHT { 2 } else { 1 }, serial);
                }
                PointerEventKind::Release { button: b @ (BTN_RIGHT | BTN_MIDDLE), .. } => {
                    self.app.release_button(if b == BTN_RIGHT { 2 } else { 1 });
                }
                PointerEventKind::Leave { .. } => {
                    self.app.leave();
                    self.icon = None;
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => {
                    // Wheels report steps; touchpads report distance.
                    let steps = |axis: &AxisScroll| match axis.value120 {
                        0 => axis.discrete as f32,
                        v => v as f32 / 120.0,
                    };
                    let (down, across) = (steps(&vertical), steps(&horizontal));
                    if self.mods.ctrl {
                        if down != 0.0 {
                            self.app.zoom_by(if down < 0.0 { 1 } else { -1 });
                        }
                    } else if self.mods.shift && down != 0.0 {
                        // Shift turns the wheel sideways.
                        self.app.scroll_across_lines(down * 3.0);
                    } else if down != 0.0 {
                        self.app.scroll_lines(down * 3.0);
                    } else if vertical.absolute != 0.0 || horizontal.absolute == 0.0 {
                        self.app.scroll(vertical.absolute as f32 * scale);
                    }
                    if !self.mods.ctrl && across != 0.0 {
                        self.app.scroll_across_lines(across * 3.0);
                    } else if !self.mods.ctrl && horizontal.absolute != 0.0 {
                        self.app.scroll_across(horizontal.absolute as f32 * scale);
                    }
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl DataDeviceHandler for State {
    /// Something is dragged over the window: files are taken, anything
    /// else is not.
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64, _: &WlSurface) {
        let Some(offer) = self.data_device.as_ref().and_then(|d| d.data().drag_offer()) else { return };
        let files = offer.with_mime_types(|have| have.iter().any(|m| m == URI_LIST));
        offer.accept_mime_type(offer.serial, files.then(|| URI_LIST.to_string()));
        if files {
            offer.set_actions(DndAction::Copy, DndAction::Copy);
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, x: f64, y: f64) {
        let scale = self.factor();
        self.pointer_at = (x as f32 * scale, y as f32 * scale);
    }

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        let Some(offer) = self.data_device.as_ref().and_then(|d| d.data().drag_offer()) else { return };
        let scale = self.factor();
        let at = (offer.x as f32 * scale, offer.y as f32 * scale);
        let Ok(pipe) = offer.receive(URI_LIST.to_string()) else { return };
        self.read_all(pipe, move |state, bytes| {
            offer.finish();
            offer.destroy();
            let paths = String::from_utf8_lossy(&bytes)
                .lines()
                .filter_map(|l| l.trim().strip_prefix("file://"))
                .map(|rest| rest.find('/').map_or(rest, |i| &rest[i..]))
                .map(|p| std::path::PathBuf::from(crate::pane::percent_decode(p)))
                .collect();
            state.app.drop_files(paths, at.0, at.1);
            state.serve();
        });
    }
}

impl DataOfferHandler for State {
    fn source_actions(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}

    fn selected_action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &mut DragOffer, _: DndAction) {}
}

impl DataSourceHandler for State {
    fn accept_mime(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: Option<String>) {}

    fn send_request(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, mime: String, pipe: WritePipe) {
        // The reader may be slow or stuck; never block the UI on it.
        let text = if mime == "text/html" { self.clipboard_html.clone().unwrap_or_default() } else { self.clipboard.clone() };
        std::thread::spawn(move || {
            let _ = File::from(OwnedFd::from(pipe)).write_all(text.as_bytes());
        });
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self.copy_source.as_ref().is_some_and(|s| s.inner() == source) {
            self.copy_source = None;
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl PrimarySelectionDeviceHandler for State {
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &ZwpPrimarySelectionDeviceV1) {}
}

impl PrimarySelectionSourceHandler for State {
    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &ZwpPrimarySelectionSourceV1,
        _: String,
        pipe: WritePipe,
    ) {
        let text = self.primary.clone();
        std::thread::spawn(move || {
            let _ = File::from(OwnedFd::from(pipe)).write_all(text.as_bytes());
        });
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &ZwpPrimarySelectionSourceV1) {
        if self.primary_source.as_ref().is_some_and(|s| s.inner() == source) {
            self.primary_source = None;
        }
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(State);
delegate_dispatch2!(State);
