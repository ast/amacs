//! The `amacs` binary: the editor as a single process in a terminal, as a
//! server for `amacsclient` (`--daemon`), or without any display (`--batch`).

mod askpass;
mod clipboard;
mod daemon;
mod standalone;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use amacs_core::Value;
use amacs_script::Session;
use anyhow::{Context, Result, anyhow, bail};

const USAGE: &str = "Usage: amacs [OPTION]... [FILE]...

Edit FILEs in the terminal. The last one given is shown first.

Options:
      --daemon      Run as a server for amacsclient, without a terminal
  -q, --no-init     Do not load the init file
      --init FILE   Load FILE instead of $XDG_CONFIG_HOME/amacs/init.scm;
                    the AMACS_INIT variable names such a file too
  -l, --load FILE   Load a Scheme file after the init file
      --eval EXPR   Evaluate a Scheme expression after the init file
      --batch       Run --load and --eval without a terminal, print the
                    values of --eval, and exit
      --test        With --batch: run the tests defined by the loaded files
                    and exit with status 1 if any fails
  -h, --help        Show this help
  -V, --version     Show the version";

enum Action {
    Load(PathBuf),
    Eval(String),
}

#[derive(Default)]
struct Options {
    files: Vec<PathBuf>,
    actions: Vec<Action>,
    init: Option<PathBuf>,
    no_init: bool,
    daemon: bool,
    batch: bool,
    test: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("amacs: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the command line. Returns `None` if it asked for help or the
/// version, which has then been printed.
fn parse_args() -> Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = std::env::args_os().skip(1);
    let mut options_done = false;
    while let Some(arg) = args.next() {
        let Some(flag) = arg
            .to_str()
            .filter(|s| !options_done && s.starts_with('-') && *s != "-")
        else {
            options.files.push(arg.into());
            continue;
        };
        let mut value = |what: &str| {
            args.next()
                .ok_or_else(|| anyhow!("{flag} needs {what}\n{USAGE}"))
        };
        match flag {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("amacs {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--" => options_done = true,
            "-q" | "--no-init" => options.no_init = true,
            "--batch" => options.batch = true,
            "--daemon" => options.daemon = true,
            "--test" => options.test = true,
            "--init" => options.init = Some(value("a file name")?.into()),
            "-l" | "--load" => options
                .actions
                .push(Action::Load(value("a file name")?.into())),
            "--eval" => {
                let expr = value("an expression")?;
                let expr = expr
                    .into_string()
                    .map_err(|_| anyhow!("--eval needs valid UTF-8"))?;
                options.actions.push(Action::Eval(expr));
            }
            _ => bail!("unknown option `{flag}`\n{USAGE}"),
        }
    }
    if options.test && !options.batch {
        bail!("--test needs --batch");
    }
    if options.daemon && options.batch {
        bail!("--daemon and --batch cannot be combined");
    }
    Ok(Some(options))
}

/// The init file to load: the one given, else the one that the
/// environment names, else the default one if it exists.
fn init_file(options: &Options) -> Option<PathBuf> {
    if options.no_init {
        return None;
    }
    if let Some(path) = &options.init {
        return Some(path.clone());
    }
    // A checkout of the editor sets this, so that the servers started in
    // it are set up alike whoever starts them.
    // A file that is not there is not the one that was meant, as when
    // the name is relative and the server was started elsewhere.
    let named = std::env::var_os("AMACS_INIT")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    if named.is_some() {
        return named;
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    let path = config.join("amacs/init.scm");
    path.exists().then_some(path)
}

/// How long the editor must be idle before unsaved changes are auto-saved.
const AUTO_SAVE_IDLE: Duration = Duration::from_secs(5);
/// Unsaved changes are auto-saved at least this often while the editor is busy.
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// How long the editor must be idle before the undo history that is not
/// on disk yet is written there. It is short, since that is little to
/// write: after a power failure, what was typed up to the last pause of
/// this length is there.
const UNDO_JOURNAL_IDLE: Duration = Duration::from_secs(1);

/// How long to wait for the next event: until the editor has something due,
/// but no longer than the idle time after which unsaved changes are saved,
/// or the undo history if there is some to write.
fn wait_time(session: &Session) -> Duration {
    let idle = if session.editor().undo_journals_due() {
        UNDO_JOURNAL_IDLE
    } else {
        AUTO_SAVE_IDLE
    };
    let until_due = session
        .next_deadline()
        .map(|due| due.saturating_duration_since(std::time::Instant::now()));
    until_due.map_or(idle, |wait| wait.min(idle))
}

/// What is done when no event came for a while. `idle` is the time since
/// the user last did something. Returns whether unsaved changes were
/// auto-saved.
fn when_idle(session: &mut Session, idle: Duration) -> bool {
    if idle >= AUTO_SAVE_IDLE {
        save_everything(session, false);
        return true;
    }
    if idle >= UNDO_JOURNAL_IDLE {
        session.editor_mut().write_undo_journals();
    }
    false
}

/// Writes what must survive the process: unsaved changes go to the
/// auto-save directory, and input history to the state directory.
fn save_everything(session: &mut Session, force: bool) -> usize {
    let mut editor = session.editor_mut();
    editor.save_state();
    editor.auto_save(force)
}

/// Where auto-save files and other persistent state go:
/// `$XDG_STATE_HOME/amacs`, or `~/.local/state/amacs`.
fn state_directory() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/state")))?;
    Some(base.join("amacs"))
}

/// Creates the session and runs what the command line asks for before any
/// input: the init file, `--load` and `--eval`, and the files to visit.
///
/// Problems in the user's files are shown in the echo area and logged to
/// `*Messages*`. They must not keep the editor from starting.
fn start_session(options: &Options, cols: u16, rows: u16) -> Result<Session> {
    amacs_core::prepare_languages();
    let mut session = Session::new(cols, rows)
        .map_err(|error| anyhow!("the Scheme runtime failed to load:\n{}", error.report))?;
    session.editor_mut().set_state_directory(state_directory());
    // A server on a socket of its own, such as the one for working on
    // the editor, is not what the user works in: its telemetry is kept
    // apart, in a file named after the socket.
    let socket = amacs_proto::socket_path();
    if let Some(name) = socket.file_stem().and_then(|name| name.to_str())
        && name != "server"
    {
        session.editor_mut().set_telemetry_name(name);
    }
    if let Some(clipboard) = clipboard::WaylandClipboard::detect() {
        session.editor_mut().set_clipboard(Box::new(clipboard));
    }
    if let Some(init) = init_file(options) {
        let _ = session.load_file(&init);
    }
    for action in &options.actions {
        let _ = match action {
            Action::Load(path) => session.load_file(path),
            Action::Eval(expr) => session.eval(expr).map(drop),
        };
    }
    for file in &options.files {
        let path = std::path::absolute(Path::new(file)).context("cannot resolve file name")?;
        let result = session.editor_mut().find_file(&path);
        if let Err(error) = result {
            session.editor_mut().message(error.to_string());
        }
    }
    // Run the hooks queued by opening the files.
    session.run_pending();
    Ok(session)
}

fn run() -> Result<ExitCode> {
    let Some(options) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    if options.batch {
        run_batch(&options)
    } else if options.daemon {
        daemon::run(&options)
    } else {
        standalone::run(&options)
    }
}

/// Runs without a terminal: loads files, evaluates expressions and prints
/// their values, and optionally runs the Scheme tests.
fn run_batch(options: &Options) -> Result<ExitCode> {
    let mut session = Session::new(80, 24)
        .map_err(|error| anyhow!("the Scheme runtime failed to load:\n{}", error.report))?;
    let script_error = |error: amacs_script::EvalError| anyhow!("{}", error.report.trim_end());

    if let Some(init) = init_file(options) {
        session.load_file(&init).map_err(script_error)?;
    }
    for action in &options.actions {
        match action {
            Action::Load(path) => session.load_file(path).map_err(script_error)?,
            Action::Eval(expr) => println!("{}", session.eval(expr).map_err(script_error)?),
        }
    }
    if !options.test {
        return Ok(ExitCode::SUCCESS);
    }

    // `run-tests` returns the number of passed tests followed by a
    // (name message) list for each failure.
    let results = session.eval_value("(run-tests)").map_err(script_error)?;
    let Value::List(results) = results else {
        bail!("run-tests returned an unexpected value: {results:?}");
    };
    let Some((Value::Int(passed), failures)) = results.split_first() else {
        bail!("run-tests returned an unexpected value: {results:?}");
    };
    for failure in failures {
        match failure {
            Value::List(parts) if parts.len() == 2 => {
                let text = |value: &Value| {
                    value
                        .as_name()
                        .map_or_else(|| format!("{value:?}"), str::to_owned)
                };
                println!("FAIL {}: {}", text(&parts[0]), text(&parts[1]));
            }
            other => println!("FAIL {other:?}"),
        }
    }
    println!("{passed} passed, {} failed", failures.len());
    Ok(if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
