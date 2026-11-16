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
};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use unicode_width::UnicodeWidthStr;

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
        SetCursorStyle::DefaultUserShape,
        Show,
        LeaveAlternateScreen
    );
    let _ = terminal::disable_raw_mode();
}

/// The terminal in raw mode on the alternate screen. Dropping it restores
/// the terminal.
pub struct Tty {
    out: BufWriter<Stdout>,
    panes: HashMap<PaneId, PaneRect>,
    faces: Vec<FaceAttrs>,
    cursor: Option<(u16, u16, CursorShape)>,
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
            enhanced_keys,
            in_update: false,
        })
    }

    /// Size of the terminal in cells, as (columns, rows).
    pub fn size(&self) -> io::Result<(u16, u16)> {
        terminal::size()
    }

    /// Blocks until the user does something the editor should know about.
    pub fn read_event(&mut self) -> io::Result<InputEvent> {
        loop {
            if let Some(event) = input::translate(crossterm::event::read()?) {
                return Ok(event);
            }
        }
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
                DrawOp::DefineFaces(faces) => self.faces.clone_from(faces),
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
                } => {
                    self.cursor = self
                        .panes
                        .get(pane)
                        .map(|rect| (rect.x + col, rect.y + row, *shape));
                }
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
            used += segment.width();
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
        for (enabled, attribute) in [
            (attrs.bold, Attribute::Bold),
            (attrs.italic, Attribute::Italic),
            (attrs.underline, Attribute::Underlined),
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
