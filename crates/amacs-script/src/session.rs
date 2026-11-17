use std::cell::{Ref, RefCell, RefMut};
use std::path::Path;
use std::rc::Rc;

use amacs_core::{Editor, FrameId, ScriptCall, ScriptOutcome, ScriptTarget, Value};
use amacs_proto::{DrawOp, InputEvent};

use crate::host::{EvalError, EvalResult, InterruptHandle};
use crate::scheme::SchemeHost;

/// An editor together with the Scheme interpreter that extends it.
///
/// The session owns the control flow between the two: the editor never calls
/// Scheme directly. It queues calls, and the session runs them after each
/// event, when nothing holds a borrow of the editor.
pub struct Session {
    editor: Rc<RefCell<Editor>>,
    host: SchemeHost,
}

impl Session {
    /// Creates an editor with a frame of the given size and loads the Scheme
    /// runtime. Fails only if the runtime itself is broken.
    pub fn new(cols: u16, rows: u16) -> Result<Self, EvalError> {
        let mut editor = Editor::new(cols, rows);
        editor.attach_script_host();
        let editor = Rc::new(RefCell::new(editor));
        let host = SchemeHost::new(&editor)?;
        let mut session = Self { editor, host };
        session.pump();
        Ok(session)
    }

    pub fn editor(&self) -> Ref<'_, Editor> {
        self.editor.borrow()
    }

    pub fn editor_mut(&mut self) -> RefMut<'_, Editor> {
        self.editor.borrow_mut()
    }

    /// A handle that stops running Scheme code from another thread.
    pub fn interrupt_handle(&self) -> InterruptHandle {
        self.host.interrupt_handle()
    }

    pub fn handle_event(&mut self, event: InputEvent) {
        self.editor.borrow_mut().handle_event(event);
        self.pump();
    }

    /// Does the editor's work that is due now and takes in results from its
    /// worker threads. The host calls this when woken and when the time
    /// from [`Session::next_deadline`] has come.
    pub fn poll(&mut self) {
        self.editor.borrow_mut().poll(std::time::Instant::now());
        self.pump();
    }

    /// When [`Session::poll`] next has something to do, if it does.
    pub fn next_deadline(&self) -> Option<std::time::Instant> {
        self.editor.borrow().next_deadline()
    }

    pub fn redisplay(&mut self) -> Vec<DrawOp> {
        self.editor.borrow_mut().redisplay()
    }

    /// Handles input from one of several frames.
    pub fn handle_frame_event(&mut self, frame: FrameId, event: InputEvent) {
        self.editor.borrow_mut().handle_frame_event(frame, event);
        self.pump();
    }

    pub fn redisplay_frame(&mut self, frame: FrameId) -> Vec<DrawOp> {
        self.editor.borrow_mut().redisplay_frame(frame)
    }

    /// Runs the Scheme calls queued by work done directly on the editor,
    /// such as the hooks of a file opened through [`Session::editor_mut`].
    pub fn run_pending(&mut self) {
        self.pump();
    }

    /// Evaluates Scheme source for a program rather than for the user: the
    /// result or error is returned and nothing is shown in the echo area.
    pub fn eval_quietly(&mut self, source: &str) -> EvalResult {
        self.editor.borrow_mut().begin_script();
        let result = self.host.eval(&self.editor, source, None);
        let call = ScriptCall {
            target: ScriptTarget::Eval(String::new()),
            args: Vec::new(),
            command: None,
            echo_result: false,
            frame: None,
        };
        self.editor
            .borrow_mut()
            .end_script(&call, ScriptOutcome::Returned(String::new()));
        self.pump();
        result
    }

    /// What Scheme code has written with `display` and its like since this
    /// was last asked. It is also in the message log.
    pub fn take_output(&mut self) -> String {
        self.host.take_output()
    }

    pub fn should_exit(&self) -> bool {
        self.editor.borrow().should_exit()
    }

    /// Evaluates Scheme source and returns the printed form of its value. An
    /// error is also shown in the echo area and logged.
    pub fn eval(&mut self, source: &str) -> EvalResult {
        self.eval_top_level(source, None)
    }

    /// Evaluates Scheme source and returns its value as data. Unlike
    /// [`Session::eval`], an error is only returned, not shown.
    pub fn eval_value(&mut self, source: &str) -> Result<Value, EvalError> {
        self.editor.borrow_mut().begin_script();
        let result = self.host.eval_value(&self.editor, source);
        let call = ScriptCall {
            target: ScriptTarget::Eval(String::new()),
            args: Vec::new(),
            command: None,
            echo_result: false,
            frame: None,
        };
        self.editor
            .borrow_mut()
            .end_script(&call, ScriptOutcome::Returned(String::new()));
        self.pump();
        result
    }

    /// Loads a Scheme file, such as the user's init file. An error is shown
    /// in the echo area and logged, and is also returned.
    pub fn load_file(&mut self, path: &Path) -> Result<(), EvalError> {
        let shown = path.display();
        let source = std::fs::read_to_string(path).map_err(|error| {
            let summary = format!("Cannot read {shown}: {error}");
            self.editor.borrow_mut().message(summary.clone());
            EvalError::plain(summary)
        })?;
        self.eval_top_level(&source, Some(path))
            .map(drop)
            .inspect_err(|error| {
                let summary = format!("Error in {shown}: {}", error.summary);
                self.editor.borrow_mut().message(summary);
            })
    }

    /// Evaluates source as one unit of work for the editor, with its own
    /// undo boundary, as a queued call would be.
    fn eval_top_level(&mut self, source: &str, path: Option<&Path>) -> EvalResult {
        let call = ScriptCall {
            target: ScriptTarget::Eval(String::new()),
            args: Vec::new(),
            command: None,
            echo_result: false,
            frame: None,
        };
        self.editor.borrow_mut().begin_script();
        let result = self.host.eval(&self.editor, source, path);
        let outcome = match &result {
            Ok(printed) => ScriptOutcome::Returned(printed.clone()),
            Err(error) => ScriptOutcome::Failed {
                summary: error.summary.clone(),
                report: error.report.clone(),
            },
        };
        self.editor.borrow_mut().end_script(&call, outcome);
        self.pump();
        result
    }

    /// Runs the Scheme calls that the editor has queued, including any that
    /// they queue in turn.
    fn pump(&mut self) {
        loop {
            let Some(call) = self.editor.borrow_mut().take_script_call() else {
                break;
            };
            self.editor.borrow_mut().begin_script();
            let outcome = self.host.run(&self.editor, &call);
            self.editor.borrow_mut().end_script(&call, outcome);
        }
    }
}
