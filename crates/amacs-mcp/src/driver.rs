//! A connection to the amacs server with a frame that nobody looks at:
//! keys go in, and what the server draws is kept as a grid of cells.

use std::path::Path;
use std::time::{Duration, Instant};

use amacs_client::Connection;
use amacs_gui::render::Renderer;
use amacs_proto::{
    ClientMsg, DrawOp, FrameRequest, InputEvent, Key, Screen, ServerMsg, SubpixelOrder, parse_keys,
};
use anyhow::{Context, Result, anyhow, bail};

/// What evaluating some Scheme came to.
pub struct Evaluated {
    pub ok: bool,
    /// The value as written, or the report of the error.
    pub text: String,
    /// What the code wrote with `display` and its like.
    pub output: String,
}

pub struct Driver {
    connection: Connection,
    pub screen: Screen,
    size: (u16, u16),
    /// The font that the server would have a window use, and the order of
    /// subpixels it would draw for.
    font: (String, u16, SubpixelOrder),
    /// Draws the frame as a window would, once a picture has been asked for.
    renderer: Option<Renderer>,
    /// The connection failed: the server is gone, or was started anew.
    pub lost: bool,
}

impl Driver {
    /// Connects, starting a server if none is running, and opens a frame.
    pub fn open(socket: &Path, cols: u16, rows: u16) -> Result<Self> {
        let mut connection = Connection::open(socket)?;
        connection.send(&ClientMsg::Edit {
            files: Vec::new(),
            wait: false,
            frame: FrameRequest::Headless { cols, rows },
        })?;
        let mut driver = Self {
            connection,
            screen: Screen::default(),
            size: (cols, rows),
            font: ("monospace".to_owned(), 16, SubpixelOrder::None),
            renderer: None,
            lost: false,
        };
        loop {
            match driver.receive()? {
                ServerMsg::Attached {
                    font_family,
                    font_size,
                    subpixel,
                } => {
                    driver.font = (font_family, font_size, subpixel);
                    break;
                }
                ServerMsg::Draw(ops) => driver.draw(&ops),
                ServerMsg::Exit { message, .. } => {
                    bail!(
                        message
                            .unwrap_or_else(|| "the server turned the connection down".to_owned())
                    )
                }
                other => bail!("unexpected reply from the server: {other:?}"),
            }
        }
        driver.sync()?;
        Ok(driver)
    }

    fn draw(&mut self, ops: &[DrawOp]) {
        self.screen.apply(ops);
        if let Some(renderer) = &mut self.renderer {
            renderer.apply(ops);
        }
    }

    /// Evaluates Scheme in this connection's frame.
    pub fn eval(&mut self, source: &str) -> Result<Evaluated> {
        self.transmit(&ClientMsg::Eval {
            source: source.to_owned(),
        })?;
        loop {
            match self.receive()? {
                ServerMsg::Draw(ops) => self.draw(&ops),
                ServerMsg::EvalResult {
                    ok, text, output, ..
                } => return Ok(Evaluated { ok, text, output }),
                ServerMsg::Exit { message, .. } => {
                    bail!(message.unwrap_or_else(|| "the server closed the connection".to_owned()))
                }
                other => bail!("unexpected reply from the server: {other:?}"),
            }
        }
    }

    /// Waits until the server has handled everything sent so far and what
    /// it drew for that has arrived.
    pub fn sync(&mut self) -> Result<()> {
        self.transmit(&ClientMsg::Sync)?;
        loop {
            match self.receive()? {
                ServerMsg::Draw(ops) => self.draw(&ops),
                ServerMsg::Synced => return Ok(()),
                ServerMsg::Exit { message, .. } => {
                    bail!(message.unwrap_or_else(|| "the server closed the connection".to_owned()))
                }
                other => bail!("unexpected reply from the server: {other:?}"),
            }
        }
    }

    pub fn send(&mut self, event: InputEvent) -> Result<()> {
        self.transmit(&ClientMsg::Input(event))
    }

    fn transmit(&mut self, message: &ClientMsg) -> Result<()> {
        let sent = self.connection.send(message);
        self.lost |= sent.is_err();
        sent
    }

    fn receive(&mut self) -> Result<ServerMsg> {
        let received = self.connection.receive();
        // Being told to go is the end of the connection too.
        self.lost |= matches!(received, Err(_) | Ok(ServerMsg::Exit { .. }));
        received
    }

    /// Presses keys given in Emacs notation, such as `C-x C-f`.
    pub fn keys(&mut self, keys: &str) -> Result<()> {
        let keys = parse_keys(keys).map_err(|error| anyhow!("{keys}: {error}"))?;
        for key in keys {
            self.send(InputEvent::Key(key))?;
        }
        self.sync()
    }

    /// Types text, a key for each char.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        for c in text.chars() {
            let key = match c {
                '\n' => parse_keys("RET")
                    .ok()
                    .and_then(|keys| keys.first().copied()),
                '\t' => parse_keys("TAB")
                    .ok()
                    .and_then(|keys| keys.first().copied()),
                c => Some(Key::char(c)),
            };
            if let Some(key) = key {
                self.send(InputEvent::Key(key))?;
            }
        }
        self.sync()
    }

    /// Waits until the screen shows a text, for what happens after a
    /// moment: a program that the editor runs, a timer. Returns whether it
    /// came to show it.
    pub fn wait_for(&mut self, text: &str, timeout: Duration) -> Result<bool> {
        let started = Instant::now();
        loop {
            self.sync()?;
            if self.screen.text().contains(text) {
                return Ok(true);
            }
            if started.elapsed() >= timeout {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.size = (cols, rows);
        if let Some(renderer) = &mut self.renderer {
            let (cell_w, cell_h) = renderer.cell_size();
            renderer.resize(u32::from(cols) * cell_w, u32::from(rows) * cell_h);
        }
        self.send(InputEvent::Resize { cols, rows })?;
        self.sync()
    }

    /// The frame as text, with where the cursor is.
    pub fn screen_text(&self) -> String {
        let (cols, rows) = self.screen.size();
        let cursor = match self.screen.cursor() {
            Some((row, col)) => format!("cursor at row {row}, column {col}"),
            None => "no cursor".to_owned(),
        };
        let numbered: Vec<String> = self
            .screen
            .text()
            .lines()
            .enumerate()
            .map(|(row, line)| format!("{row:>2}|{line}"))
            .collect();
        format!(
            "{cols}x{rows} frame, {cursor} (rows and columns count from 0)\n{}",
            numbered.join("\n")
        )
    }

    /// The frame as a window would show it, as a PNG image.
    pub fn screen_png(&mut self) -> Result<Vec<u8>> {
        if self.renderer.is_none() {
            // Tests name a directory of fonts, so as not to depend on
            // what is installed.
            let size = f32::from(self.font.1);
            let mut renderer = match std::env::var_os("AMACS_TEST_FONTS") {
                Some(dir) => Renderer::with_font_dir(Path::new(&dir), "JetBrains Mono", size),
                None => Renderer::new(&self.font.0, size),
            }
            .with_subpixel(self.font.2);
            let (cell_w, cell_h) = renderer.cell_size();
            renderer.resize(
                u32::from(self.size.0) * cell_w,
                u32::from(self.size.1) * cell_h,
            );
            self.renderer = Some(renderer);
            // The renderer has seen nothing yet: have everything drawn anew.
            let (cols, rows) = self.size;
            self.send(InputEvent::Resize { cols, rows })?;
        }
        self.sync()?;
        let renderer = self.renderer.as_mut().expect("made above");
        renderer.paint();
        let (width, height) = renderer.size();
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let rgb: Vec<u8> = renderer
            .pixels()
            .iter()
            .flat_map(|pixel| [(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8])
            .collect();
        encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(&rgb))
            .context("cannot encode the picture")?;
        Ok(bytes)
    }
}

/// Evaluates Scheme without a frame of its own: in the frame that the
/// user's clients act in.
pub fn eval_for_user(socket: &Path, source: &str) -> Result<Evaluated> {
    let mut connection = Connection::open(socket)?;
    connection.send(&ClientMsg::Eval {
        source: source.to_owned(),
    })?;
    loop {
        match connection.receive()? {
            ServerMsg::EvalResult {
                ok, text, output, ..
            } => return Ok(Evaluated { ok, text, output }),
            ServerMsg::Draw(_) => {}
            ServerMsg::Exit { message, .. } => {
                bail!(message.unwrap_or_else(|| "the server closed the connection".to_owned()))
            }
            other => bail!("unexpected reply from the server: {other:?}"),
        }
    }
}
