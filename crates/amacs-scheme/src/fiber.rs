//! A thread of execution: a stack, the calls in progress on it, and the
//! handlers for errors. The machine runs one fiber at a time; coroutines
//! are fibers that are waiting.

use std::rc::Rc;

use crate::code::Proto;
use crate::heap::Marker;
use crate::value::{Handle, Value};

/// What to do when a call returns, besides returning.
#[derive(Debug, Clone)]
pub enum Cleanup {
    None,
    /// The call was the body of a handler form: take its handler off.
    PopHandler,
    /// The call was a handler running.
    RestoreHandlers(Box<Restore>),
}

/// What to do when a handler returns.
#[derive(Debug, Clone)]
pub struct Restore {
    /// The handlers that were set aside while it ran, to put back. Its
    /// value is then the value of the `raise-continuable` that called it.
    pub handlers: Vec<Handler>,
    /// What was raised, if that cannot be continued from: it goes on to
    /// the next handler instead.
    pub reraise: Option<Value>,
}

/// A call in progress.
#[derive(Debug)]
pub struct Frame {
    pub closure: Handle,
    pub proto: Rc<Proto>,
    /// The next instruction.
    pub pc: usize,
    /// Where on the stack the call's local variables start. The closure
    /// itself is in the slot before.
    pub base: usize,
    pub cleanup: Cleanup,
}

/// Something that takes an error.
#[derive(Debug, Clone)]
pub enum Handler {
    /// Unwind to a call and call a procedure with the error there.
    Catch {
        /// Number of frames to keep.
        frames: usize,
        /// Where on the stack the result goes.
        slot: usize,
        handler: Value,
    },
    /// Call a procedure with the error where it was raised.
    Procedure(Value),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiberState {
    /// Made but not started: its stack holds the procedure to call.
    New,
    Running,
    /// Waiting to be resumed.
    Suspended,
    Done,
}

#[derive(Debug)]
pub struct Fiber {
    pub stack: Vec<Value>,
    pub frames: Vec<Frame>,
    pub handlers: Vec<Handler>,
    pub state: FiberState,
    /// The fiber that resumed this one and waits for it to yield or end.
    pub parent: Option<Box<Fiber>>,
    /// The coroutine object this fiber belongs to, if it is a coroutine.
    pub object: Option<Handle>,
}

impl Fiber {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            frames: Vec::new(),
            handlers: Vec::new(),
            state: FiberState::Running,
            parent: None,
            object: None,
        }
    }

    pub fn mark(&self, marker: &mut Marker) {
        marker.values(&self.stack);
        for frame in &self.frames {
            marker.handle(frame.closure);
            if let Cleanup::RestoreHandlers(restore) = &frame.cleanup {
                mark_handlers(&restore.handlers, marker);
                if let Some(payload) = restore.reraise {
                    marker.value(payload);
                }
            }
        }
        mark_handlers(&self.handlers, marker);
        if let Some(object) = self.object {
            marker.handle(object);
        }
        if let Some(parent) = &self.parent {
            parent.mark(marker);
        }
    }
}

impl Default for Fiber {
    fn default() -> Self {
        Self::new()
    }
}

fn mark_handlers(handlers: &[Handler], marker: &mut Marker) {
    for handler in handlers {
        match handler {
            Handler::Catch { handler, .. } | Handler::Procedure(handler) => marker.value(*handler),
        }
    }
}
