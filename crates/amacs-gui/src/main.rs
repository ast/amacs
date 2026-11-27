//! `amacs-gui`: a frame of the amacs server in a Wayland window.
//!
//! The window is a thin client. It sends key presses and its size in
//! character cells to the server and draws what the server sends back.

mod output;
mod present;

use std::ffi::OsString;
use std::io::BufWriter;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use amacs_client::{Connection, parse_position};
use amacs_gui::keys;
use amacs_gui::render::Fonts;
use amacs_proto::{
    ClientMsg, FileSpec, FrameRequest, InputEvent, MouseEvent, ServerMsg, SubpixelOrder, read_msg,
    socket_path, write_msg,
};
use anyhow::{Context, Result, anyhow, bail};
use output::{Output, RendererKind};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection as WaylandConnection, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_registry, registry_handlers};

const USAGE: &str = "Usage: amacs-gui [OPTION]... [+LINE[:COLUMN]] [FILE]...

Open a window on the amacs server and visit FILEs in it. The server is
started if it is not running.

Options:
  -n, --no-wait           Keep the window when the files are finished
      --font FAMILY       Font family, instead of the `font-family` option
      --font-size PIXELS  Font size, instead of the `font-size` option
      --subpixel ORDER    Draw text for subpixels in this order, `rgb` or
                          `bgr`, or `none` for shades of grey, instead of
                          the `subpixel-order` option
      --renderer KIND     Draw with the `cpu` or the `gpu`, instead of what
                          the `gpu-renderer` option says; AMACS_RENDERER
                          in the environment says the same
  -s, --socket-name PATH  Use this socket instead of the default
  -h, --help              Show this help
  -V, --version           Show the version";

/// The frame's size before the compositor has said how big the window is.
const INITIAL_GRID: (u16, u16) = (80, 24);
/// The fractional-scale protocol expresses scales in 120ths.
const SCALE_DENOMINATOR: u32 = 120;

#[derive(Default)]
struct Options {
    files: Vec<FileSpec>,
    no_wait: bool,
    font: Option<String>,
    font_size: Option<u16>,
    subpixel: Option<SubpixelOrder>,
    renderer: Option<RendererKind>,
    socket: Option<PathBuf>,
}

fn parse_args() -> Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = std::env::args_os().skip(1);
    let mut options_done = false;
    let mut position = None;
    while let Some(arg) = args.next() {
        let text = arg.to_str().filter(|_| !options_done);
        if let Some(parsed) = text
            .and_then(|s| s.strip_prefix('+'))
            .and_then(parse_position)
        {
            position = Some(parsed);
            continue;
        }
        let Some(flag) = text.filter(|s| s.starts_with('-') && *s != "-") else {
            let path = std::path::absolute(Path::new(&arg)).context("cannot resolve file name")?;
            let (line, column) = position.take().unzip();
            options.files.push(FileSpec {
                path,
                line,
                column: column.flatten(),
            });
            continue;
        };
        let mut value = |what: &str| -> Result<OsString> {
            args.next()
                .ok_or_else(|| anyhow!("{flag} needs {what}\n{USAGE}"))
        };
        match flag {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("amacs-gui {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--" => options_done = true,
            "-n" | "--no-wait" => options.no_wait = true,
            "-s" | "--socket-name" => options.socket = Some(value("a path")?.into()),
            "--font" => {
                let family = value("a font family")?;
                let family = family
                    .into_string()
                    .map_err(|_| anyhow!("--font needs valid UTF-8"))?;
                options.font = Some(family);
            }
            "--font-size" => {
                let size = value("a size in pixels")?;
                let size = size
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .filter(|s| (4..=200).contains(s));
                options.font_size =
                    Some(size.ok_or_else(|| anyhow!("--font-size needs a number from 4 to 200"))?);
            }
            "--subpixel" => {
                let order = value("none, rgb or bgr")?;
                let order = order.to_str().and_then(SubpixelOrder::parse);
                options.subpixel =
                    Some(order.ok_or_else(|| anyhow!("--subpixel needs none, rgb or bgr"))?);
            }
            "--renderer" => {
                let kind = value("cpu or gpu")?;
                let kind = kind.to_str().and_then(RendererKind::parse);
                options.renderer =
                    Some(kind.ok_or_else(|| anyhow!("--renderer needs cpu or gpu"))?);
            }
            _ => bail!("unknown option `{flag}`\n{USAGE}"),
        }
    }
    Ok(Some(options))
}

/// The family that the system's font configuration means by `monospace`.
/// The text library has its own idea of that, which ignores the user's.
fn system_monospace() -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(["--format", "%{family}", "monospace"])
        .output()
        .ok()?;
    let families = String::from_utf8(output.stdout).ok()?;
    let first = families.split(',').next()?.trim();
    (output.status.success() && !first.is_empty()).then(|| first.to_owned())
}

/// What the server sends, or the end of the connection.
enum FromServer {
    Message(ServerMsg),
    Closed,
}

/// User data for protocol objects that send no events we care about.
struct Quiet;

/// User data for the object that reports the surface's preferred scale.
struct ScaleListener;

struct App {
    /// Before the window, so that it is dropped first: a GPU's surface
    /// must be gone before the surface of the window that it is on.
    output: Output,
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    window: Window,
    /// Present if the compositor supports fractional scaling.
    viewport: Option<WpViewport>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Scrolling by a touchpad that has not yet come to a whole line, in
    /// pixels of the surface.
    scroll_rest: f64,
    /// The last press of a button: when, at which cell, which button, and
    /// how many presses in a row it made.
    press: Option<(u32, (u16, u16), u8, u8)>,
    /// The button that is held and the cell the pointer is at meanwhile.
    dragging: Option<(u8, (u16, u16))>,
    modifiers: Modifiers,
    loop_handle: LoopHandle<'static, App>,

    /// Font size at scale 1, in pixels.
    font_size: f32,
    /// Output scale in 120ths.
    scale: u32,
    /// Window size in surface units, once the compositor has given one.
    size: Option<(u32, u32)>,
    /// Columns and rows last reported to the server.
    grid: (u16, u16),
    /// The size of a cell in pixels that the server was last told.
    cell: Option<(u16, u16)>,
    /// A frame has been committed and the compositor has not yet said that
    /// it is time for the next.
    frame_pending: bool,
    /// The picture changed while a frame was pending.
    needs_draw: bool,

    /// Window size to use when the compositor leaves the choice to us.
    initial_size: (u32, u32),
    qh: QueueHandle<App>,
    server: BufWriter<UnixStream>,
    /// Set when it is time to stop: a message for the user and the status.
    exit: Option<(Option<String>, u8)>,
}

impl App {
    fn send(&mut self, event: InputEvent) {
        if write_msg(&mut self.server, &ClientMsg::Input(event)).is_err() {
            self.exit = Some((
                Some("amacs-gui: lost the connection to the server".to_owned()),
                1,
            ));
        }
    }

    /// Size of the buffer in pixels for the current window size and scale.
    fn buffer_size(&self) -> Option<(u32, u32)> {
        let (width, height) = self.size?;
        let scaled =
            |length: u32| (length * self.scale + SCALE_DENOMINATOR / 2) / SCALE_DENOMINATOR;
        Some((scaled(width).max(1), scaled(height).max(1)))
    }

    /// Adapts to a new window size or scale: resizes the picture and tells
    /// the server how many cells now fit.
    fn relayout(&mut self) {
        let Some((width, height)) = self.buffer_size() else {
            return;
        };
        self.output.resize(width, height);
        let grid = self.output.grid_size(width, height);
        // The size of a cell goes with it, for the pictures that the
        // server sends: it changes with the scale of the output.
        let cell = self.output.cell_size();
        let cell = (
            u16::try_from(cell.0).unwrap_or(u16::MAX),
            u16::try_from(cell.1).unwrap_or(u16::MAX),
        );
        if grid != self.grid || Some(cell) != self.cell {
            self.grid = grid;
            self.cell = Some(cell);
            self.send(InputEvent::Resize {
                cols: grid.0,
                rows: grid.1,
                cell: Some(cell),
            });
        }
        self.request_draw();
    }

    fn set_scale(&mut self, scale: u32) {
        if scale == self.scale || scale == 0 {
            return;
        }
        self.scale = scale;
        self.output
            .set_font_size(self.font_size * scale as f32 / SCALE_DENOMINATOR as f32);
        self.relayout();
    }

    /// Draws now, or as soon as the compositor is ready for another frame.
    fn request_draw(&mut self) {
        if self.frame_pending {
            self.needs_draw = true;
        } else {
            self.draw();
        }
    }

    fn draw(&mut self) {
        self.needs_draw = false;
        if self.buffer_size().is_none() {
            return;
        }
        let surface = self.window.wl_surface();
        match (&self.viewport, self.size) {
            // The buffer is in device pixels; the viewport says how large
            // that is in surface units.
            (Some(viewport), Some((surface_w, surface_h))) => {
                surface.set_buffer_scale(1);
                viewport.set_destination(surface_w as i32, surface_h as i32);
            }
            _ => surface.set_buffer_scale((self.scale / SCALE_DENOMINATOR).max(1) as i32),
        }
        // The compositor answers a frame callback when it wants the next
        // frame. Until then further changes are collected rather than
        // drawn. The callback is part of what a commit carries.
        let commits = self.output.commits();
        if commits {
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        }
        let shown = match self.output.show(&self.shm, surface) {
            Ok(shown) => shown,
            Err(error) => {
                eprintln!("amacs-gui: {error:#}");
                false
            }
        };
        if !shown {
            return;
        }
        if !commits {
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
            self.window.commit();
        }
        self.frame_pending = true;
    }

    fn key(&mut self, event: &KeyEvent) {
        if let Some(key) = keys::translate(event.keysym, self.modifiers) {
            self.send(InputEvent::Key(key));
        }
    }

    fn handle_server(&mut self, message: FromServer) {
        match message {
            FromServer::Message(ServerMsg::Draw(ops)) => {
                if self.output.apply(&ops) {
                    self.request_draw();
                }
            }
            FromServer::Message(ServerMsg::Exit { message, status }) => {
                self.exit = Some((message, status));
            }
            FromServer::Message(_) => {}
            FromServer::Closed => {
                if self.exit.is_none() {
                    let message = "amacs-gui: lost the connection to the server".to_owned();
                    self.exit = Some((Some(message), 1));
                }
            }
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("amacs-gui: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let Some(options) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };

    // Fail on a missing display before bothering the server.
    let wayland = WaylandConnection::connect_to_env()
        .context("cannot connect to a Wayland compositor (is WAYLAND_DISPLAY set?)")?;

    let socket = options.socket.clone().unwrap_or_else(socket_path);
    let mut connection = Connection::open(&socket)?;
    connection.send(&ClientMsg::Edit {
        files: options.files,
        wait: !options.no_wait,
        frame: FrameRequest::New {
            cols: INITIAL_GRID.0,
            rows: INITIAL_GRID.1,
        },
    })?;
    let (family, font_size, subpixel, gpu) = match connection.receive()? {
        ServerMsg::Attached {
            font_family,
            font_size,
            subpixel,
            gpu,
        } => (font_family, font_size, subpixel, gpu),
        ServerMsg::Exit { message, status } => {
            if let Some(message) = message {
                eprintln!("{message}");
            }
            return Ok(ExitCode::from(status));
        }
        other => bail!("unexpected reply from the server: {other:?}"),
    };
    let family = options.font.unwrap_or(family);
    let family = match family.as_str() {
        "monospace" => system_monospace().unwrap_or(family),
        _ => family,
    };
    let font_size = f32::from(options.font_size.unwrap_or(font_size));
    let subpixel = options.subpixel.unwrap_or(subpixel);

    let (globals, event_queue) =
        registry_queue_init(&wayland).context("cannot list Wayland globals")?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> =
        EventLoop::try_new().context("cannot create the event loop")?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(wayland.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|error| anyhow!("cannot watch the Wayland connection: {error}"))?;

    let compositor =
        CompositorState::bind(&globals, &qh).context("the compositor lacks wl_compositor")?;
    let xdg_shell = XdgShell::bind(&globals, &qh).context("the compositor lacks xdg-shell")?;
    let shm = Shm::bind(&globals, &qh).context("the compositor lacks wl_shm")?;

    let surface = compositor.create_surface(&qh);
    // Crisp text at scales such as 1.25 needs both of these protocols.
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, Quiet).ok();
    let scale_manager: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, Quiet).ok();
    let viewport = match (&viewporter, &scale_manager) {
        (Some(viewporter), Some(manager)) => {
            manager.get_fractional_scale(&surface, &qh, ScaleListener);
            Some(viewporter.get_viewport(&surface, &qh, Quiet))
        }
        _ => None,
    };

    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("amacs");
    window.set_app_id("amacs");
    window.commit();

    let kind = options
        .renderer
        .or_else(|| RendererKind::parse(&std::env::var("AMACS_RENDERER").ok()?))
        .unwrap_or(if gpu {
            RendererKind::Gpu
        } else {
            RendererKind::Cpu
        });
    let output = Output::new(
        kind,
        Fonts::new(&family, font_size).with_subpixel(subpixel),
        &wayland,
        window.wl_surface(),
    );
    let (cell_w, cell_h) = output.cell_size();
    let initial = (
        u32::from(INITIAL_GRID.0) * cell_w,
        u32::from(INITIAL_GRID.1) * cell_h,
    );

    // The server is read on its own thread and its messages are handed to
    // the event loop.
    let Connection { mut reader, writer } = connection;
    let (sender, messages): (_, Channel<FromServer>) = channel::channel();
    std::thread::spawn(move || {
        while let Ok(Some(message)) = read_msg::<_, ServerMsg>(&mut reader) {
            if sender.send(FromServer::Message(message)).is_err() {
                return;
            }
        }
        let _ = sender.send(FromServer::Closed);
    });
    loop_handle
        .insert_source(messages, |event, (), app| match event {
            channel::Event::Msg(message) => app.handle_server(message),
            channel::Event::Closed => app.handle_server(FromServer::Closed),
        })
        .map_err(|error| anyhow!("cannot watch the server connection: {error}"))?;

    let mut app = App {
        output,
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        window,
        viewport,
        keyboard: None,
        pointer: None,
        scroll_rest: 0.0,
        press: None,
        dragging: None,
        modifiers: Modifiers::default(),
        loop_handle,
        font_size,
        scale: SCALE_DENOMINATOR,
        size: None,
        grid: INITIAL_GRID,
        cell: None,
        frame_pending: false,
        needs_draw: false,
        server: writer,
        exit: None,
        initial_size: initial,
        qh: qh.clone(),
    };

    while app.exit.is_none() {
        event_loop
            .dispatch(Duration::from_secs(1), &mut app)
            .context("the Wayland connection failed")?;
    }
    let (message, status) = app.exit.take().unwrap_or((None, 0));
    if let Some(message) = message {
        eprintln!("{message}");
    }
    Ok(ExitCode::from(status))
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        // Whole-number scales are the fallback for compositors without the
        // fractional-scale protocol.
        if self.viewport.is_none() {
            self.set_scale(factor.max(1) as u32 * SCALE_DENOMINATOR);
        }
    }

    fn transform_changed(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.frame_pending = false;
        if self.needs_draw {
            self.draw();
        }
    }

    fn surface_enter(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &WaylandConnection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
    }

    fn update_output(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: wl_output::WlOutput,
    ) {
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &WaylandConnection, _: &QueueHandle<Self>, _: &Window) {
        // Closing the connection is what tells the server the frame is gone.
        self.exit = Some((None, 0));
    }

    fn configure(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let (width, height) = configure.new_size;
        // A compositor that does not care, as for a floating window, gets
        // the size of the initial grid.
        let fallback = self.size.unwrap_or_else(|| {
            let unscaled = |pixels: u32| pixels * SCALE_DENOMINATOR / self.scale;
            (unscaled(self.initial_size.0), unscaled(self.initial_size.1))
        });
        self.size = Some((
            width.map_or(fallback.0, |width| width.get()),
            height.map_or(fallback.1, |height| height.get()),
        ));
        self.relayout();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &WaylandConnection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &WaylandConnection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            // Held keys repeat at the compositor's rate through this callback.
            let repeat = Box::new(
                |app: &mut App, _: &wl_keyboard::WlKeyboard, event: KeyEvent| {
                    app.key(&event);
                },
            );
            self.keyboard = self
                .seat_state
                .get_keyboard_with_repeat(qh, &seat, None, self.loop_handle.clone(), repeat)
                .ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _: &WaylandConnection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

/// How many lines one notch of a mouse wheel scrolls.
const WHEEL_LINES: i32 = 3;
/// The number that the editor gives a button of a mouse, from the one
/// that Linux gives it: 1 for the main button, 2 for the middle one and 3
/// for the one for menus, 8 and 9 for those at the side that go back and
/// forward.
fn button_number(button: u32) -> Option<u8> {
    match button {
        0x110 => Some(1),
        0x112 => Some(2),
        0x111 => Some(3),
        // The buttons at the side, numbered as X numbers them.
        0x113 => Some(8),
        0x114 => Some(9),
        _ => None,
    }
}
/// How long after a press another one at the same cell counts with it, in
/// milliseconds.
const MULTI_CLICK_MS: u32 = 400;

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            // The pointer is in surface units; cells are in buffer pixels.
            let scale = f64::from(self.scale) / f64::from(SCALE_DENOMINATOR);
            let (cell_w, cell_h) = self.output.cell_size();
            let cell = |position: f64, size: u32, count: u16| {
                let index = (position.max(0.0) * scale / f64::from(size.max(1))) as u32;
                index.min(u32::from(count.saturating_sub(1))) as u16
            };
            let col = cell(event.position.0, cell_w, self.grid.0);
            let row = cell(event.position.1, cell_h, self.grid.1);
            match &event.kind {
                PointerEventKind::Axis { vertical, .. } => {
                    let lines = if vertical.value120 != 0 {
                        vertical.value120 * WHEEL_LINES / 120
                    } else if vertical.discrete != 0 {
                        vertical.discrete * WHEEL_LINES
                    } else {
                        // A touchpad: so many pixels, of which the part
                        // that is no whole line is kept for the next time.
                        let line = f64::from(cell_h.max(1)) / scale;
                        self.scroll_rest += vertical.absolute;
                        let lines = (self.scroll_rest / line).trunc();
                        self.scroll_rest -= lines * line;
                        lines as i32
                    };
                    if lines != 0 {
                        let lines = lines.clamp(-1000, 1000) as i16;
                        self.send(InputEvent::Mouse(MouseEvent::Scroll { col, row, lines }));
                    }
                }
                PointerEventKind::Press { button, time, .. } => {
                    let Some(button) = button_number(*button) else {
                        continue;
                    };
                    // Presses of one button soon after each other at one
                    // cell count up to three and start over.
                    let again = self.press.is_some_and(|(at, cell, pressed, clicks)| {
                        cell == (col, row)
                            && pressed == button
                            && clicks < 3
                            && time.wrapping_sub(at) <= MULTI_CLICK_MS
                    });
                    let clicks = match self.press {
                        Some((_, _, _, clicks)) if again => clicks + 1,
                        _ => 1,
                    };
                    self.press = Some((*time, (col, row), button, clicks));
                    // The button that went down first is the one that drags.
                    if self.dragging.is_none() {
                        self.dragging = Some((button, (col, row)));
                    }
                    let mods = keys::mods(self.modifiers);
                    self.send(InputEvent::Mouse(MouseEvent::Press {
                        col,
                        row,
                        button,
                        mods,
                        clicks,
                    }));
                }
                PointerEventKind::Motion { .. } => {
                    if let Some((button, cell)) = self.dragging
                        && cell != (col, row)
                    {
                        self.dragging = Some((button, (col, row)));
                        let mods = keys::mods(self.modifiers);
                        self.send(InputEvent::Mouse(MouseEvent::Drag { col, row, mods }));
                    }
                }
                PointerEventKind::Release { button, .. }
                    if self
                        .dragging
                        .is_some_and(|(held, _)| Some(held) == button_number(*button)) =>
                {
                    self.dragging = None;
                    let mods = keys::mods(self.modifiers);
                    self.send(InputEvent::Mouse(MouseEvent::Release { col, row, mods }));
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.key(&event);
    }

    fn repeat_key(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
        // Repeats arrive through the callback given when the keyboard was set up.
    }

    fn release_key(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &WaylandConnection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch2<WpViewporter, App> for Quiet {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewporter,
        _: <WpViewporter as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &WaylandConnection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpViewport, App> for Quiet {
    fn event(
        &self,
        _: &mut App,
        _: &WpViewport,
        _: <WpViewport as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &WaylandConnection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpFractionalScaleManagerV1, App> for Quiet {
    fn event(
        &self,
        _: &mut App,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &WaylandConnection,
        _: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<WpFractionalScaleV1, App> for ScaleListener {
    fn event(
        &self,
        app: &mut App,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &WaylandConnection,
        _: &QueueHandle<App>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            app.set_scale(scale);
        }
    }
}

delegate_registry!(App);
smithay_client_toolkit::delegate_dispatch2!(App);
