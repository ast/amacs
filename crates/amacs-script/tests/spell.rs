//! Spelling: the marks on misspelled words, which the editor makes, and
//! what is done about them, which is written in Scheme:
//! `runtime/lib/spell.scm`.

mod common;

use std::path::Path;

use common::Harness;

/// An editor that finds two small dictionaries, `en_XX` and `sv_XX`, and
/// keeps its own words in a directory that is thrown away, showing a file
/// of that directory.
fn setup(name: &str, text: &str) -> (tempfile::TempDir, Harness) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(name), text).unwrap();
    let mut h = Harness::new(60, 12);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spell");
    h.eval(&format!(
        "(set-option! 'spell-dictionary-path '({:?}))
         (set-option! 'spell-languages '(\"en_XX\"))
         (set-option! 'spell-personal-file {:?})
         (find-file {:?})",
        fixtures.display().to_string(),
        dir.path().join("mine/words").display().to_string(),
        dir.path().join(name).display().to_string(),
    ));
    (dir, h)
}

fn marked(h: &Harness, row: usize, col: usize) -> bool {
    h.screen.face_at(row, col).undercurl
}

#[test]
fn a_misspelled_word_gets_a_wave_under_it() {
    let (_dir, mut h) = setup(
        "note.txt",
        "The quick brwn fox\nand a URL: https://exampel.org\n",
    );
    assert_eq!(h.eval("(spell-dictionaries)"), "(\"en_XX\" \"sv_XX\")");
    h.wait_until(|h| marked(h, 0, 10));
    let face = h.screen.face_at(0, 10);
    assert_eq!(face.underline_color, h.face("misspelled").underline_color);
    assert!(marked(&h, 0, 13) && !marked(&h, 0, 14), "the whole word");
    assert!(!marked(&h, 0, 0) && !marked(&h, 0, 4) && !marked(&h, 0, 15));
    assert!(!marked(&h, 1, 20), "an address is not words");

    // A word of another language is right once the buffer is in both.
    h.keys("M->").type_text("hej ");
    h.wait_until(|h| marked(h, 2, 0));
    h.eval("(set-local! 'spell-languages '(\"en_XX\" \"sv_XX\"))");
    h.wait_until(|h| !marked(h, 2, 0) && marked(h, 0, 10));

    // The mode can be turned off.
    h.keys("M-x").type_text("spell-mode").keys("RET");
    assert!(!marked(&h, 0, 10));
}

#[test]
fn in_code_only_comments_and_strings_are_checked() {
    let code = "// A brwn fox\nfn brwn() {\n    let qick = \"a qick fox\";\n}\n";
    let (_dir, mut h) = setup("code.rs", code);
    h.wait_until(|h| marked(h, 0, 5) && marked(h, 2, 18));
    assert!(!marked(&h, 1, 3), "the name of a function");
    assert!(!marked(&h, 2, 8), "the name of a variable");
    assert!(!marked(&h, 0, 10), "a word that is right");
}

#[test]
fn a_word_is_not_marked_while_it_is_typed() {
    let (_dir, mut h) = setup("note.txt", "");
    h.type_text("the qick ");
    h.wait_until(|h| marked(h, 0, 4));
    h.keys("DEL");
    assert!(!marked(&h, 0, 4), "point is at its end again");
    h.keys("C-a");
    assert!(marked(&h, 0, 4));
}

#[test]
fn the_nearest_misspelled_word_is_corrected_with_a_key() {
    let (_dir, mut h) = setup("note.txt", "The quick brwn fox jumps\n");
    h.keys("M->").keys("M-$");
    h.wait_until(|h| h.prompt().contains("Correct brwn:"));
    let offered = h.screen.text();
    assert!(offered.contains("brown"), "{offered}");
    assert!(
        offered.contains("@brwn") && offered.contains("*brwn"),
        "{offered}"
    );
    assert_eq!(
        h.screen.face_at(0, 10).bg,
        h.face("region").bg,
        "the word is shown"
    );
    h.type_text("brown").keys("RET");
    assert_eq!(h.text(), "The quick brown fox jumps\n");
    assert_eq!(h.screen.cursor(), Some((1, 0)), "point is where it was");
    h.wait_until(|h| !marked(h, 0, 10));

    h.keys("M-$");
    h.wait_until(|h| h.echo().contains("No misspelled word in sight"));
}

#[test]
fn a_word_can_be_kept_for_good_for_now_or_for_a_project() {
    let (dir, mut h) = setup("note.txt", "amacs and brwn and qick\n");
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    h.wait_until(|h| marked(h, 0, 0) && marked(h, 0, 10) && marked(h, 0, 19));

    // Saved in the user's own file, which did not exist.
    h.keys("M-$");
    h.wait_until(|h| h.prompt().contains("Correct amacs:"));
    h.type_text("@amacs").keys("RET");
    assert!(!marked(&h, 0, 0) && marked(&h, 0, 10));
    let mine = dir.path().join("mine/words");
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), "amacs\n");
    assert_eq!(h.text(), "amacs and brwn and qick\n");

    // Accepted until the editor exits.
    h.keys("M-f M-f M-f M-$");
    h.wait_until(|h| h.prompt().contains("Correct brwn:"));
    h.type_text("*brwn").keys("RET");
    assert!(!marked(&h, 0, 10) && marked(&h, 0, 19));
    assert_eq!(std::fs::read_to_string(&mine).unwrap(), "amacs\n");

    // Saved in the project's file, which other buffers of it read.
    h.keys("M-> M-$");
    h.wait_until(|h| h.prompt().contains("Correct qick:"));
    assert!(h.screen.text().contains("+qick"));
    h.type_text("+qick").keys("RET");
    assert!(!marked(&h, 0, 19));
    let project = dir.path().join(".spelling");
    assert_eq!(std::fs::read_to_string(&project).unwrap(), "qick\n");
    std::fs::write(dir.path().join("other.txt"), "qick amacs zzz\n").unwrap();
    h.eval(&format!(
        "(find-file {:?})",
        dir.path().join("other.txt").display().to_string()
    ));
    h.wait_until(|h| marked(h, 0, 11));
    assert!(!marked(&h, 0, 0) && !marked(&h, 0, 5));
}

#[test]
fn code_can_ask_for_the_misspelled_words_and_for_suggestions() {
    let (_dir, mut h) = setup("note.txt", "The qick brwn fox\n");
    h.eval("(define found #f) (define meant #f)");
    h.eval("(run-later (lambda () (set! found (spell-misspellings))))");
    h.wait_until(|h| h.eval("found") != "#f");
    assert_eq!(h.eval("found"), "((4 8) (9 13))");
    h.eval("(run-later (lambda () (set! meant (spell-suggest \"qick\"))))");
    h.wait_until(|h| h.eval("meant") != "#f");
    assert!(h.eval("meant").contains("\"quick\""), "{}", h.eval("meant"));
    assert_eq!(h.eval("(syntax-at 0)"), "#f");

    // The commands that move from one to the next.
    h.keys("M-< M-x").type_text("spell-next").keys("RET");
    h.wait_until(|h| h.screen.cursor() == Some((0, 4)));
    h.keys("M-> M-x").type_text("spell-previous").keys("RET");
    h.wait_until(|h| h.screen.cursor() == Some((0, 9)));
}

#[test]
fn a_language_without_a_dictionary_marks_nothing_and_says_so() {
    let (_dir, mut h) = setup("note.txt", "Zzz qick\n");
    h.wait_until(|h| marked(h, 0, 4));
    h.eval("(set-local! 'spell-languages '(\"zz_ZZ\"))");
    h.wait_until(|h| {
        let log = "(if (member \"*Messages*\" (buffer-list))
                       (with-current-buffer \"*Messages*\" (buffer-string))
                       \"\")";
        h.eval(log).contains("no dictionary zz_ZZ.aff")
    });
    assert!(!marked(&h, 0, 4));
}
