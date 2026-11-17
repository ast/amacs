//! What the Scheme engines have in common.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amacs_core::Editor;

/// Which Scheme runs the editor's extensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Steel,
    /// amacs's own Scheme, the `amacs-scheme` crate.
    Own,
}

impl Engine {
    /// The engine that `AMACS_ENGINE` names: `steel`, or `own`.
    pub fn from_env() -> Self {
        match std::env::var("AMACS_ENGINE").as_deref() {
            Ok("own") => Engine::Own,
            _ => Engine::Steel,
        }
    }
}

thread_local! {
    /// The editor that primitives act on while Scheme code runs on this thread.
    static CURRENT: RefCell<Option<Rc<RefCell<Editor>>>> = const { RefCell::new(None) };
}

pub(crate) fn current_editor() -> Option<Rc<RefCell<Editor>>> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Makes `editor` the target of primitives until the guard is dropped.
pub(crate) struct CurrentEditor {
    previous: Option<Rc<RefCell<Editor>>>,
}

impl CurrentEditor {
    pub(crate) fn enter(editor: &Rc<RefCell<Editor>>) -> Self {
        let previous = CURRENT.with(|current| current.replace(Some(editor.clone())));
        Self { previous }
    }
}

impl Drop for CurrentEditor {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.previous.take()));
    }
}

/// Lets another thread stop Scheme code that is running, for `C-g`.
#[derive(Clone)]
pub struct InterruptHandle {
    running: Arc<AtomicBool>,
    stop: Arc<dyn Fn() + Send + Sync>,
}

impl InterruptHandle {
    pub(crate) fn new(running: Arc<AtomicBool>, stop: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            running,
            stop: Arc::new(stop),
        }
    }

    /// Interrupts the running Scheme code. Does nothing, and returns false,
    /// if none is running.
    pub fn interrupt(&self) -> bool {
        let running = self.running.load(Ordering::SeqCst);
        if running {
            (self.stop)();
        }
        running
    }
}

/// What evaluating Scheme code produced.
pub type EvalResult = Result<String, EvalError>;

#[derive(Debug, Clone)]
pub struct EvalError {
    /// One line.
    pub summary: String,
    /// The full report with the source location.
    pub report: String,
}
