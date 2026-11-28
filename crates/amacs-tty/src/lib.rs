//! Terminal frontend: turns terminal input into [`InputEvent`]s and draws
//! [`DrawOp`]s with escape sequences.

mod input;

use std::collections::HashMap;
use std::io::{self, BufWriter, Stdout, Write};

use amacs_proto::{
    Color, CursorShape, DisplayLine, DrawOp, FaceAttrs, InputEvent, PaneId, PaneRect,
};
use crossterm::cursor::{Hide, MoveTo, SetCursorStyle, Show};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::style::{
    Attribute, Color as TermColor, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
    SetUnderlineColor,
};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

/// What tells a terminal to draw its cursor in a colour (OSC 12). The
/// terminal's own cursor is drawn in a colour chosen for the terminal's
/// own background, which on the background of the editor's theme can be
/// next to invisible.
fn cursor_color_sequence(color: Color) -> String {
    format!(
        "\x1b]12;#{:02x}{:02x}{:02x}\x1b\\",
        color.r, color.g, color.b
    )
}

/// What gives the terminal's cursor its own colour back (OSC 112).
const RESET_CURSOR_COLOR: &str = "\x1b]112\x1b\\";

/// What tells a terminal the colour of its own background (OSC 11): the
/// colour of what no cell covers. A window is seldom a whole number of
/// cells high and wide, and the strip that is left over at its bottom
/// and right is drawn in this colour, which is that of the terminal's
/// own theme unless it is told the editor's.
fn background_color_sequence(color: Color) -> String {
    format!(
        "\x1b]11;#{:02x}{:02x}{:02x}\x1b\\",
        color.r, color.g, color.b
    )
}

/// What gives the terminal its own background back (OSC 111).
const RESET_BACKGROUND_COLOR: &str = "\x1b]111\x1b\\";

/// Puts the terminal back the way it was. Safe to call more than once, and
/// from a panic hook.
fn restore_terminal(enhanced_keys: bool) {
    let mut out = io::stdout();
    // Nothing useful can be done if the terminal is gone, so errors are ignored.
    if enhanced_keys {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        Print(RESET_CURSOR_COLOR),
        Print(RESET_BACKGROUND_COLOR),
        SetCursorStyle::DefaultUserShape,
        Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

/// Blocks until the user does something the editor should know about. This
/// may be called from a thread other than the one that draws.
pub fn read_event() -> io::Result<InputEvent> {
    loop {
        if let Some(event) = input::translate(crossterm::event::read()?) {
            return Ok(event);
        }
    }
}

/// Size of the terminal in cells, as (columns, rows). Works before and
/// after the terminal is taken over.
pub fn terminal_size() -> io::Result<(u16, u16)> {
    terminal::size()
}

/// The terminal in raw mode on the alternate screen. Dropping it restores
/// the terminal.
pub struct Tty {
    out: BufWriter<Stdout>,
    panes: HashMap<PaneId, PaneRect>,
    faces: Vec<FaceAttrs>,
    cursor: Option<(u16, u16, CursorShape)>,
    /// The colour the terminal was last told to draw its cursor in.
    cursor_color: Option<Color>,
    /// The colour the terminal was last told its background is.
    background: Option<Color>,
    enhanced_keys: bool,
    in_update: bool,
}

impl Tty {
    /// Takes over the terminal. Also installs a panic hook that restores it,
    /// so that a panic message is readable.
    pub fn new() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // The kitty keyboard protocol lets the terminal report keys such as
        // C-ö, C-. and C-S-p that have no legacy encoding.
        let enhanced_keys = terminal::supports_keyboard_enhancement().unwrap_or(false);
        input::ENHANCED_KEYS.store(enhanced_keys, std::sync::atomic::Ordering::Relaxed);

        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal(enhanced_keys);
            previous_hook(info);
        }));

        let mut out = BufWriter::new(io::stdout());
        queue!(
            out,
            EnterAlternateScreen,
            EnableBracketedPaste,
            Clear(ClearType::All)
        )?;
        if enhanced_keys {
            queue!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        out.flush()?;
        Ok(Self {
            out,
            panes: HashMap::new(),
            faces: Vec::new(),
            cursor: None,
            cursor_color: None,
            background: None,
            enhanced_keys,
            in_update: false,
        })
    }

    /// Size of the terminal in cells, as (columns, rows).
    pub fn size(&self) -> io::Result<(u16, u16)> {
        terminal::size()
    }

    /// Draws a batch of operations. Nothing reaches the terminal until the
    /// batch's `Flush`.
    pub fn apply(&mut self, ops: &[DrawOp]) -> io::Result<()> {
        for op in ops {
            if !self.in_update {
                queue!(self.out, BeginSynchronizedUpdate, Hide)?;
                self.in_update = true;
            }
            match op {
                DrawOp::DefineFaces(faces) => {
                    self.faces.clone_from(faces);
                    // The first face is the theme's default: its
                    // background is that of the whole frame, also of
                    // what is left over beside the cells.
                    let background = faces.first().and_then(|face| face.bg);
                    if background != self.background {
                        let sequence = match background {
                            Some(color) => background_color_sequence(color),
                            None => RESET_BACKGROUND_COLOR.to_owned(),
                        };
                        queue!(self.out, Print(sequence))?;
                        self.background = background;
                    }
                }
                DrawOp::Layout { panes, .. } => {
                    self.panes = panes.iter().map(|pane| (pane.id, *pane)).collect();
                    self.cursor = None;
                    queue!(
                        self.out,
                        SetAttribute(Attribute::Reset),
                        Clear(ClearType::All)
                    )?;
                }
                DrawOp::Lines {
                    pane,
                    first_row,
                    lines,
                } => {
                    let Some(rect) = self.panes.get(pane).copied() else {
                        continue;
                    };
                    for (offset, line) in lines.iter().enumerate() {
                        let row = first_row.saturating_add(offset as u16);
                        if row < rect.h {
                            self.draw_line(rect, row, line)?;
                        }
                    }
                }
                DrawOp::Cursor {
                    pane,
                    row,
                    col,
                    shape,
                    color,
                } => {
                    self.cursor = self
                        .panes
                        .get(pane)
                        .map(|rect| (rect.x + col, rect.y + row, *shape));
                    if *color != self.cursor_color {
                        let sequence = match color {
                            Some(color) => cursor_color_sequence(*color),
                            None => RESET_CURSOR_COLOR.to_owned(),
                        };
                        queue!(self.out, Print(sequence))?;
                        self.cursor_color = *color;
                    }
                }
                // A terminal draws no pictures and is sent none.
                DrawOp::DefineImage { .. } | DrawOp::DropImages(_) => {}
                DrawOp::Flush => {
                    if let Some((x, y, shape)) = self.cursor {
                        let style = match shape {
                            CursorShape::Block => SetCursorStyle::SteadyBlock,
                            CursorShape::Bar => SetCursorStyle::SteadyBar,
                        };
                        queue!(self.out, MoveTo(x, y), style, Show)?;
                    }
                    queue!(self.out, EndSynchronizedUpdate)?;
                    self.in_update = false;
                    self.out.flush()?;
                }
            }
        }
        Ok(())
    }

    fn draw_line(&mut self, rect: PaneRect, row: u16, line: &DisplayLine) -> io::Result<()> {
        queue!(self.out, MoveTo(rect.x, rect.y + row))?;
        let mut used = 0;
        for (segment, face) in line.segments() {
            self.set_face(face.0)?;
            queue!(self.out, Print(segment))?;
            // As the server counts, which a terminal's own count of a string
            // can differ from for a sequence of emojis.
            used += amacs_proto::text_cells(segment);
        }
        let padding = usize::from(rect.w).saturating_sub(used);
        if padding > 0 {
            self.set_face(line.fill.0)?;
            queue!(self.out, Print(" ".repeat(padding)))?;
        }
        Ok(())
    }

    fn set_face(&mut self, face: u32) -> io::Result<()> {
        let attrs = self.faces.get(face as usize).copied().unwrap_or_default();
        queue!(self.out, SetAttribute(Attribute::Reset))?;
        if let Some(color) = attrs.fg {
            queue!(self.out, SetForegroundColor(term_color(color)))?;
        }
        if let Some(color) = attrs.bg {
            queue!(self.out, SetBackgroundColor(term_color(color)))?;
        }
        if let Some(color) = attrs.underline_color.filter(|_| attrs.underline) {
            queue!(self.out, SetUnderlineColor(term_color(color)))?;
        }
        for (enabled, attribute) in [
            (attrs.bold, Attribute::Bold),
            (attrs.italic, Attribute::Italic),
            (attrs.underline && !attrs.undercurl, Attribute::Underlined),
            (attrs.undercurl, Attribute::Undercurled),
            (attrs.inverse, Attribute::Reverse),
        ] {
            if enabled {
                queue!(self.out, SetAttribute(attribute))?;
            }
        }
        Ok(())
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        let _ = queue!(self.out, SetAttribute(Attribute::Reset));
        let _ = self.out.flush();
        restore_terminal(self.enhanced_keys);
    }
}

fn term_color(color: Color) -> TermColor {
    TermColor::Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_colour_is_set_as_xterm_does() {
        assert_eq!(
            cursor_color_sequence(Color::rgb(0xd0, 0x00, 0x0a)),
            "\x1b]12;#d0000a\x1b\\"
        );
        assert_eq!(
            background_color_sequence(Color::rgb(0xfb, 0xf7, 0xf0)),
            "\x1b]11;#fbf7f0\x1b\\"
        );
    }
}
