use std::io;

/// Why a command stopped. The message is shown in the echo area.
#[derive(Debug, thiserror::Error)]
pub enum CmdError {
    #[error("{0}")]
    Message(String),
    #[error("Quit")]
    Quit,
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl CmdError {
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

pub type CmdResult<T = ()> = Result<T, CmdError>;
