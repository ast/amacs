//! Property tests: arbitrary key sequences must never panic, must always
//! leave a drawable screen, and must be fully undoable.

use amacs_testkit::Harness;
use proptest::prelude::*;

/// Keys that edit, move, kill, undo and rearrange windows, all within the
/// scratch buffer. Nothing here opens the minibuffer or touches files.
const KEYS: &[&str] = &[
    "a", "b", "SPC", "ö", "日", "TAB", "RET", "C-o", "C-d", "DEL", "C-k", "M-d", "M-DEL", "C-w",
    "M-w", "C-y", "M-y", "C-/", "C-/", "C-?", "C-SPC", "C-x C-x", "C-x h", "C-g", "C-f", "C-b",
    "C-n", "C-p", "C-a", "C-e", "M-f", "M-b", "M-}", "M-{", "M-<", "M->", "C-v", "M-v", "C-l",
    "C-u", "C-u 3", "M-2", "C-x 2", "C-x 3", "C-x 0", "C-x 1", "C-x o", "C-u -", "C-M-i", "M-+",
    "S-TAB",
];
// ESC is deliberately absent: it would turn the first key of the next entry
// into a Meta key and leave the rest as a `C-x` prefix, which reaches
// commands that prompt and so breaks the assumptions of the undo properties.

/// Keys that also open and leave the minibuffer and switch or kill buffers.
/// No file commands, so nothing is written to disk.
const MINIBUFFER_KEYS: &[&str] = &["M-x", "C-x b", "C-x k", "RET", "TAB", "C-g", "y", "n"];

/// Keys for source code: delimiters, indentation, and the commands that
/// work on whole expressions, with and without paredit.
const CODE_KEYS: &[&str] = &[
    "(", ")", "[", "]", "{", "}", "\"", "'", ";", "/", "#", "\\", "x", "SPC", "RET", "TAB", "DEL",
    "C-d", "C-k", "C-f", "C-b", "C-n", "C-p", "C-a", "C-e", "M-<", "M->", "C-M-f", "C-M-b",
    "C-M-k", "C-M-DEL", "C-M-u", "C-M-d", "C-M-n", "C-M-SPC", "C-w", "C-y", "C-/", "C-?", "M-(",
    "M-s", "M-r", "C-)", "C-}", "C-(", "C-{", "C-x h", "C-M-\\", "C-g", "C-u", "C-M-i", "M-+",
    "S-TAB", "f", "n",
];

/// Snippets whose names the keys can type, with fields, a mirrored field,
/// placeholders and indentation.
fn define_snippets(h: &mut Harness) {
    use amacs_core::Value;
    let sym = Value::sym;
    h.editor.define_snippet(
        "fundamental-mode",
        "a",
        vec![
            Value::str("a("),
            sym("p"),
            Value::List(vec![sym("p"), Value::str("日b")]),
            Value::str(") {"),
            sym("n>"),
            Value::List(vec![sym("s"), sym("x")]),
            sym("r>"),
            sym("n"),
            Value::List(vec![sym("s"), sym("x")]),
            Value::str("}"),
            sym("q"),
        ],
    );
    h.editor.define_snippet(
        "fundamental-mode",
        "fn",
        vec![Value::str("fn "), sym("p"), Value::str("()")],
    );
}

fn code_key_sequences() -> impl Strategy<Value = Vec<&'static str>> {
    prop::collection::vec(prop::sample::select(CODE_KEYS), 0..100)
}

fn key_sequences() -> impl Strategy<Value = Vec<&'static str>> {
    prop::collection::vec(prop::sample::select(KEYS), 0..120)
}

fn key_sequences_with_minibuffer() -> impl Strategy<Value = Vec<&'static str>> {
    let all: Vec<&'static str> = KEYS.iter().chain(MINIBUFFER_KEYS).copied().collect();
    prop::collection::vec(prop::sample::select(all), 0..120)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn any_key_sequence_leaves_a_consistent_editor(
        keys in key_sequences_with_minibuffer(),
        cols in 1u16..50,
        rows in 1u16..14,
    ) {
        let mut h = Harness::new(cols, rows);
        define_snippets(&mut h);
        for key in &keys {
            h.keys(key);
            let (width, height) = h.screen.size();
            if let Some((row, col)) = h.screen.cursor() {
                prop_assert!(row < height && col < width);
            }
            prop_assert!(h.editor.point() <= h.editor.text().len());
        }
    }

    #[test]
    fn undoing_everything_restores_the_empty_buffer(keys in key_sequences()) {
        let mut h = Harness::new(30, 10);
        for key in &keys {
            h.keys(key);
        }
        // Leave any pending prefix, then undo until there is nothing left.
        h.keys("C-g C-g");
        for _ in 0..keys.len() * 2 + 1 {
            h.keys("C-/");
            if h.echo() == "No further undo information" {
                break;
            }
        }
        prop_assert_eq!(h.echo(), "No further undo information");
        prop_assert_eq!(h.buffer_text(), "");
    }

    #[test]
    fn redo_after_undo_restores_the_text(keys in key_sequences()) {
        let mut h = Harness::new(30, 10);
        for key in &keys {
            h.keys(key);
        }
        h.keys("C-g C-g");
        let before = h.buffer_text();
        h.keys("C-/");
        if h.echo() == "Undo" {
            h.keys("C-?");
            prop_assert_eq!(h.buffer_text(), before);
        }
    }

    #[test]
    fn code_keys_keep_the_syntax_tree_in_step(
        keys in code_key_sequences(),
        language in prop::sample::select(vec!["rust", "scheme", "python", "bash"]),
        paredit in any::<bool>(),
    ) {
        let mut h = Harness::new(40, 10);
        h.editor.set_language(Some(language)).unwrap();
        h.editor
            .set_local_option("line-numbers", amacs_core::Value::sym("relative"))
            .unwrap();
        let mode = if paredit { "paredit-mode" } else { "auto-pair-mode" };
        h.editor.set_minor_mode(mode, false, true).unwrap();
        define_snippets(&mut h);
        for key in &keys {
            h.keys(key);
            let (width, height) = h.screen.size();
            if let Some((row, col)) = h.screen.cursor() {
                prop_assert!(row < height && col < width);
            }
            prop_assert!(h.editor.point() <= h.editor.text().len());

            // The tree that followed the edits highlights the text exactly
            // as a tree parsed from scratch does.
            let buffer = h.editor.buffer();
            let all = 0..buffer.text().len_bytes();
            let mut fresh = amacs_core::Buffer::with_text("fresh", buffer.text().clone());
            fresh.set_language(buffer.language());
            fresh.update_syntax();
            prop_assert_eq!(buffer.highlights(all.clone()), fresh.highlights(all));
        }
        // Everything typed can be undone.
        h.keys("C-g C-g");
        for _ in 0..keys.len() * 2 + 1 {
            h.keys("C-/");
            if h.echo() == "No further undo information" {
                break;
            }
        }
        prop_assert_eq!(h.buffer_text(), "");
    }
}
