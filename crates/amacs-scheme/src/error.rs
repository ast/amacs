//! Errors, and how they are shown.

use std::fmt;

use crate::source::{Location, Span};
use crate::value::Value;

/// One call that was in progress when an error was raised.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceFrame {
    /// The procedure's name, or `None` for code at the top level of a file.
    pub name: Option<String>,
    pub span: Option<Span>,
}

/// An error inside the interpreter. It refers to values on the heap, so it
/// is only meaningful while the interpreter is; [`Report`] is what leaves.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub message: String,
    /// Values that the message is about, shown after it.
    pub irritants: Vec<Value>,
    /// Where in the source the error is, if it is known before the machine
    /// adds what it knows.
    pub span: Option<Span>,
    pub trace: Vec<TraceFrame>,
    /// The value given to `raise`, when the program raised something of its
    /// own rather than an error.
    pub payload: Option<Value>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            irritants: Vec::new(),
            span: None,
            trace: Vec::new(),
            payload: None,
        }
    }

    pub fn with_irritants(mut self, irritants: Vec<Value>) -> Self {
        self.irritants = irritants;
        self
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

/// An error as text, for whoever asked for the code to be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// One line: what went wrong.
    pub message: String,
    /// Where the error itself is.
    pub location: Option<Location>,
    /// The calls in progress, innermost first, with where each had got to.
    pub trace: Vec<(Option<String>, Option<Location>)>,
    /// If this is not an error but an escape that nothing took, such as an
    /// interruption: the symbol that says which.
    pub escape: Option<String>,
}

impl Report {
    /// The message with its location in front, as compilers write it.
    pub fn summary(&self) -> String {
        match &self.location {
            Some(location) => format!("{location}: {}", self.message),
            None => self.message.clone(),
        }
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}", self.message)?;
        if let Some(location) = &self.location {
            write!(f, "\n  at {location}")?;
        }
        for (name, location) in &self.trace {
            let name = name.as_deref().unwrap_or("top level");
            match location {
                Some(location) => write!(f, "\n  in {name}, at {location}")?,
                None => write!(f, "\n  in {name}")?,
            }
        }
        Ok(())
    }
}

impl std::error::Error for Report {}
