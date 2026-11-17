//! Frames: independent views of the editor, each with its own windows.
//!
//! A frontend that attaches to the editor gets a frame. Input arrives for a
//! frame and makes it the current one; commands act on the current frame.

use amacs_proto::{DrawOp, InputEvent};
use slotmap::new_key_type;

use crate::buffer::{Buffer, BufferId};
use crate::editor::{Editor, InputState};
use crate::error::{CmdError, CmdResult};
use crate::redisplay::RedisplayState;
use crate::window::{Layout, WindowId};

new_key_type! {
    pub struct FrameId;
}

pub(crate) struct Frame {
    pub layout: Layout,
    pub selected: WindowId,
    /// The one-row window at the bottom that shows the minibuffer.
    pub minibuffer_window: WindowId,
    /// Columns and rows.
    pub size: (u16, u16),
    pub redisplay: RedisplayState,
    /// The message in the echo area.
    pub echo: Option<String>,
    /// Lines shown under the echo area until the next key, such as the keys
    /// that can follow a prefix.
    pub hint: Vec<String>,
    /// The buffer that the minibuffer window shows.
    pub minibuffer_buffer: BufferId,
    /// The input in progress here, while another frame is the current one.
    pub input: InputState,
}

impl Editor {
    pub(crate) fn frame(&self) -> &Frame {
        &self.frames[self.current_frame]
    }

    pub(crate) fn frame_mut(&mut self) -> &mut Frame {
        &mut self.frames[self.current_frame]
    }

    pub(crate) fn selected(&self) -> WindowId {
        self.frame().selected
    }

    pub(crate) fn minibuffer_window(&self) -> WindowId {
        self.frame().minibuffer_window
    }

    pub(crate) fn new_frame(&mut self, buffer: BufferId, cols: u16, rows: u16) -> FrameId {
        let selected = self.make_window(buffer);
        // Not in the list of buffers: nobody switches to it.
        let minibuffer_buffer = self.buffers.insert(Buffer::new(" *Minibuf*"));
        let minibuffer_window = self.make_window(minibuffer_buffer);
        self.frames.insert(Frame {
            minibuffer_buffer,
            input: InputState::default(),
            layout: Layout::Leaf(selected),
            selected,
            minibuffer_window,
            size: (cols, rows),
            redisplay: RedisplayState::default(),
            echo: None,
            hint: Vec::new(),
        })
    }

    /// The frame that commands act on: the one that last received input.
    pub fn has_frame(&self, id: FrameId) -> bool {
        self.frames.contains_key(id)
    }

    pub fn current_frame(&self) -> FrameId {
        self.current_frame
    }

    /// The frame the editor was created with. It always exists.
    pub fn base_frame(&self) -> FrameId {
        self.base_frame
    }

    pub fn frame_ids(&self) -> impl Iterator<Item = FrameId> + '_ {
        self.frames.keys()
    }

    /// Creates a frame showing the current buffer. It does not become the
    /// current frame until it is selected or receives input.
    pub fn create_frame(&mut self, cols: u16, rows: u16) -> FrameId {
        let buffer = self.window().buffer;
        // The new window starts where the current one is looking.
        let (point, start) = (self.point(), self.buffer().marker(self.window().start));
        self.buffers[buffer].save_view(point, start);
        self.new_frame(buffer, cols, rows)
    }

    /// Makes a frame current. A prompt or key sequence in progress stays
    /// with the frame it is in, and is there again when that frame is.
    pub fn select_frame(&mut self, id: FrameId) {
        if id == self.current_frame || !self.frames.contains_key(id) {
            return;
        }
        let input = self.stash_input();
        self.frame_mut().input = input;
        self.current_frame = id;
        let input = std::mem::take(&mut self.frame_mut().input);
        self.restore_input(input);
    }

    /// Removes a frame and its windows. The base frame cannot be removed.
    pub fn delete_frame(&mut self, id: FrameId) -> CmdResult {
        if id == self.base_frame {
            return Err(CmdError::msg("Attempt to delete the sole frame"));
        }
        if !self.frames.contains_key(id) {
            return Ok(());
        }
        // What was being asked in the frame is given up, there.
        let back = self.current_frame;
        self.select_frame(id);
        self.cancel_input();
        let next = if back == id {
            // Prefer another frame that someone is looking at.
            self.frames
                .keys()
                .find(|other| *other != id && *other != self.base_frame)
                .unwrap_or(self.base_frame)
        } else {
            back
        };
        self.select_frame(next);
        let frame = self.frames.remove(id).expect("the frame exists");

        for window in frame.layout.leaves() {
            self.remove_window(window);
        }
        self.remove_window(frame.minibuffer_window);
        self.buffers.remove(frame.minibuffer_buffer);
        Ok(())
    }

    /// Handles input from a frame, which becomes the current frame.
    pub fn handle_frame_event(&mut self, frame: FrameId, event: InputEvent) {
        if self.frames.contains_key(frame) {
            self.select_frame(frame);
            self.handle_event(event);
        }
    }

    /// Returns the operations that bring a frame's frontend up to date.
    pub fn redisplay_frame(&mut self, frame: FrameId) -> Vec<DrawOp> {
        if !self.frames.contains_key(frame) {
            return Vec::new();
        }
        // Drawing reads the same state that commands do, so the frame is
        // made current for the duration.
        let current = self.current_frame;
        self.select_frame(frame);
        let ops = self.redisplay();
        self.select_frame(current);
        ops
    }
}
