//! The pointer. What is done with it becomes keys, such as `<mouse-1>` and
//! `<drag-mouse-1>`, that are looked up in the keymaps like any others, so
//! that a mode can give them a meaning of its own. The commands they run
//! ask the editor where the pointer is and where its button went down.

use std::ops::Range;

use amacs_proto::{Key, KeyCode, Mods, MouseEvent, NamedKey};
use amacs_text::Text;

use crate::buffer::BufferId;
use crate::editor::Editor;
use crate::error::{CmdError, CmdResult};
use crate::window::WindowId;

/// Where the pointer is and what was last done with it.
pub(crate) struct Mouse {
    /// The cell of the frame that the pointer was last known to be at.
    cell: (u16, u16),
    /// The modifiers held when the pointer last did something.
    mods: Mods,
    press: Option<Press>,
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            cell: (0, 0),
            mods: Mods::empty(),
            press: None,
        }
    }
}

/// The last press of a button.
struct Press {
    button: u8,
    clicks: u8,
    window: WindowId,
    buffer: BufferId,
    pos: usize,
    /// Whether the button is still down.
    held: bool,
    /// Whether the pointer has moved to another cell since.
    moved: bool,
    /// What stays selected wherever the pointer is dragged: nothing after
    /// one click, a word after two, a line after three.
    anchor: Range<usize>,
}

/// The word that a position is in or beside, or the one char there.
fn word_around(text: &Text, pos: usize) -> Range<usize> {
    let is_word = |c: &char| c.is_alphanumeric() || *c == '_';
    let start = pos - text.chars_before(pos).take_while(is_word).count();
    let end = pos + text.chars_from(pos).take_while(is_word).count();
    if start == end {
        pos..(pos + 1).min(text.len())
    } else {
        start..end
    }
}

impl Editor {
    pub(crate) fn handle_mouse(&mut self, event: MouseEvent) {
        self.relayout();
        match event {
            MouseEvent::Scroll { col, row, lines } => {
                let Some((id, _, _)) = self.window_at(col, row) else {
                    return;
                };
                // The window under the pointer scrolls, whichever is selected.
                let selected = self.selected();
                self.frame_mut().selected = id;
                let result = self.scroll_lines(i64::from(lines));
                self.frame_mut().selected = selected;
                self.report(result);
            }
            MouseEvent::Press {
                col,
                row,
                button,
                mods,
                clicks,
            } => {
                self.mouse.press = None;
                self.mouse.cell = (col, row);
                self.mouse.mods = mods;
                if self.in_minibuffer() || self.key_prompt.is_some() {
                    return;
                }
                let Some((id, row, col)) = self.window_at(col, row) else {
                    return;
                };
                self.frame_mut().echo = None;
                self.cancel_pending();
                // A press is in the window it is made in: the keys of that
                // window's buffer say what it does.
                self.frame_mut().selected = id;
                let pos = self.position_at(id, row, col);
                self.mouse.press = Some(Press {
                    button,
                    clicks,
                    window: id,
                    buffer: self.window().buffer,
                    pos,
                    held: true,
                    moved: false,
                    anchor: pos..pos,
                });
                let key = match clicks {
                    0 | 1 => NamedKey::Mouse(button),
                    2 => NamedKey::DoubleMouse(button),
                    _ => NamedKey::TripleMouse(button),
                };
                // A press that has no meaning of its own with these
                // modifiers or this many clicks is still a press.
                let single = NamedKey::Mouse(button);
                let found = [
                    (key, mods),
                    (single, mods),
                    (key, Mods::empty()),
                    (single, Mods::empty()),
                ]
                .into_iter()
                .any(|(key, mods)| self.run_mouse_key(Key::new(KeyCode::Named(key), mods)));
                if !found {
                    self.mouse.press = None;
                }
            }
            MouseEvent::Drag { col, row, mods } => {
                self.mouse.cell = (col, row);
                self.mouse.mods = mods;
                let Some(button) = self.held_button() else {
                    return;
                };
                if let Some(press) = &mut self.mouse.press {
                    press.moved = true;
                }
                self.run_mouse_key(Key::named(NamedKey::MotionMouse(button)));
            }
            MouseEvent::Release { col, row, mods } => {
                self.mouse.cell = (col, row);
                self.mouse.mods = mods;
                let Some(button) = self.held_button() else {
                    return;
                };
                let Some(press) = &mut self.mouse.press else {
                    return;
                };
                press.held = false;
                if press.moved {
                    self.run_mouse_key(Key::named(NamedKey::DragMouse(button)));
                }
            }
        }
    }

    /// The button that is down, if what it was pressed on is still there:
    /// the window may be gone since, or show another buffer, or a prompt
    /// may have come up.
    fn held_button(&mut self) -> Option<u8> {
        let press = self.mouse.press.as_ref().filter(|press| press.held)?;
        let stale = !self.frame().layout.leaves().contains(&press.window)
            || self.windows[press.window].buffer != press.buffer
            || self.selected() != press.window
            || self.in_minibuffer()
            || self.key_prompt.is_some();
        if stale {
            self.mouse.press = None;
            return None;
        }
        Some(press.button)
    }

    /// Runs the command that a key of the pointer is bound to. Returns
    /// whether it is bound.
    fn run_mouse_key(&mut self, key: Key) -> bool {
        let Some(command) = self.key_binding(&[key]) else {
            return false;
        };
        self.last_key = Some(key);
        self.execute(&command);
        true
    }

    // What commands and Scheme code ask

    /// The window that the pointer is in and the position in its buffer of
    /// the char that the pointer is at.
    pub fn mouse_position(&self) -> Option<(WindowId, usize)> {
        let (col, row) = self.mouse.cell;
        let (id, row, col) = self.window_at(col, row)?;
        Some((id, self.position_at(id, row, col)))
    }

    /// The window and the position at which a button was last pressed.
    pub fn mouse_press_position(&self) -> Option<(WindowId, usize)> {
        let press = self.mouse.press.as_ref()?;
        self.windows
            .contains_key(press.window)
            .then_some((press.window, press.pos))
    }

    /// The modifiers held when the pointer last did something.
    pub fn mouse_modifiers(&self) -> Mods {
        self.mouse.mods
    }

    fn press(&mut self) -> CmdResult<&mut Press> {
        let window = self.selected();
        self.mouse
            .press
            .as_mut()
            .filter(|press| press.window == window)
            .ok_or_else(|| CmdError::msg("This command is for a button of the pointer"))
    }
}

/// Puts point where the button was pressed. A second press in quick
/// succession takes the word there and a third the line.
pub(crate) fn mouse_set_point(ed: &mut Editor) -> CmdResult {
    let (pos, clicks) = {
        let press = ed.press()?;
        (press.pos, press.clicks)
    };
    let anchor = match clicks {
        0 | 1 => pos..pos,
        2 => word_around(ed.text(), pos),
        _ => {
            let text = ed.text();
            let line = text.line_of(pos);
            text.line_start(line)..(text.line_end(line) + 1).min(text.len())
        }
    };
    ed.set_point(anchor.end);
    let buffer = ed.buffer_mut();
    buffer.rectangle_mark = false;
    buffer.mark_active = !anchor.is_empty();
    if buffer.mark_active {
        buffer.set_mark(anchor.start);
    }
    ed.press()?.anchor = anchor;
    Ok(())
}

/// Makes the region reach from the mark, or from point if there is none,
/// to where the button was pressed.
pub(crate) fn mouse_extend_region(ed: &mut Editor) -> CmdResult {
    let pos = ed.press()?.pos;
    let point = ed.point();
    let mark = ed.buffer().mark().unwrap_or(point);
    ed.set_point(pos);
    let buffer = ed.buffer_mut();
    buffer.set_mark(mark);
    buffer.mark_active = mark != pos;
    ed.press()?.anchor = mark..mark;
    Ok(())
}

/// Makes the region reach from where the button was pressed to where the
/// pointer is now.
pub(crate) fn mouse_drag_region(ed: &mut Editor) -> CmdResult {
    let (window, anchor) = {
        let press = ed.press()?;
        (press.window, press.anchor.clone())
    };
    let (col, row) = ed.mouse.cell;
    let rect = ed.windows[window].rect;
    let last = rect.y + rect.h.saturating_sub(2);
    // Beyond the top or the bottom of the window, the text follows.
    if row < rect.y {
        ed.scroll_lines(-1)?;
    } else if row > last {
        ed.scroll_lines(1)?;
    }
    let row = usize::from(row.clamp(rect.y, last) - rect.y);
    let col = usize::from(col.saturating_sub(rect.x));
    let pos = ed.position_at(window, row, col);
    let (mark, point) = if pos < anchor.start {
        (anchor.end, pos)
    } else {
        (anchor.start, pos.max(anchor.end))
    };
    ed.set_point(point);
    let buffer = ed.buffer_mut();
    buffer.set_mark(mark);
    buffer.mark_active = mark != point;
    Ok(())
}
