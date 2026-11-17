//! What the Scheme host and the session share.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use amacs_core::Editor;

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
    /// The message alone.
    pub message: String,
    /// Where the error is, as `file:line:column`.
    pub location: Option<String>,
    /// The calls that were in progress, innermost first: the procedure's
    /// name and where it had got to.
    pub trace: Vec<(Option<String>, Option<String>)>,
}

impl EvalError {
    /// An error that is one line and nothing more.
    pub fn plain(summary: impl Into<String>) -> Self {
        let summary = summary.into();
        Self {
            report: summary.clone(),
            message: summary.clone(),
            summary,
            location: None,
            trace: Vec::new(),
        }
    }
}
