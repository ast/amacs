//! The source blocks of Org, which are run and written to files by Scheme:
//! `runtime/lib/org-src.scm`.

mod common;

use common::Harness;

/// An editor that visits a file of Org with a text, in a directory of its
/// own, and does not ask before it runs a block.
fn setup(text: &str) -> (Harness, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.org");
    std::fs::write(&file, text).unwrap();
    let mut h = Harness::new(80, 20);
    h.eval("(set-option! 'org-confirm-run #f)");
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    (h, dir)
}

/// Puts point at the first occurrence of a text in the buffer.
fn goto(h: &mut Harness, text: &str) {
    h.eval(&format!(
        "(goto-char (string-search (buffer-string) {text:?}))"
    ));
}

/// Presses keys that run a program, and waits for what it leaves in the
/// echo area.
fn run(h: &mut Harness, keys: &str) -> String {
    h.keys(keys);
    h.wait_until(|h| !h.echo().starts_with("Running") && !h.echo().is_empty());
    h.echo()
}

#[test]
fn a_block_is_read_into_its_language_its_keys_and_its_code() {
    let (mut h, _dir) = setup(
        "  #+begin_src sh -n :tangle a b.sh :results silent\n    echo one\n\n  ,* not a heading\n  ,#+end_src\n  #+end_src\n",
    );
    assert_eq!(
        h.eval("(org-src-header (org-element-at 20))"),
        r#"("sh" tangle "a b.sh" results "silent")"#
    );
    assert_eq!(
        h.eval("(org-src-code (org-element-at 20))"),
        r#""  echo one\n\n* not a heading\n#+end_src\n""#
    );
    assert_eq!(h.eval("(length (org-src-blocks))"), "1");
}

#[test]
fn a_block_is_run_and_what_it_wrote_goes_under_it() {
    let (mut h, dir) = setup(
        "* Run\n#+begin_src sh\necho \"in $(basename \"$PWD\")\"\necho\necho two\n#+end_src\nafter\n",
    );
    goto(&mut h, "echo");
    assert_eq!(run(&mut h, "C-c C-c"), "Done");
    let name = dir.path().file_name().unwrap().to_str().unwrap().to_owned();
    assert_eq!(
        h.text(),
        format!(
            "* Run\n#+begin_src sh\necho \"in $(basename \"$PWD\")\"\necho\necho two\n#+end_src\n\n\
             #+RESULTS:\n: in {name}\n:\n: two\nafter\n"
        )
    );
    assert_eq!(h.eval("(point)"), "21", "point stays in the block");

    // A second run puts the new result in the place of the old one.
    h.eval("(begin (goto-char (string-search (buffer-string) \"echo two\")) (insert \"echo more\\n\"))");
    assert_eq!(run(&mut h, "C-c C-v e"), "Done");
    assert!(
        h.text().ends_with(&format!(
            "#+end_src\n\n#+RESULTS:\n: in {name}\n:\n: more\n: two\nafter\n"
        )),
        "{}",
        h.text()
    );
    // One undo takes the result away.
    h.keys("C-/");
    assert!(
        h.text().contains(": in ") && !h.text().contains(": more"),
        "{}",
        h.text()
    );
}

#[test]
fn many_lines_go_in_an_example_block_and_no_output_takes_the_result_away() {
    let (mut h, _dir) =
        setup("#+begin_src bash\nseq 12\n#+end_src\n\n#+RESULTS:\n: old\n\nafter\n");
    goto(&mut h, "seq");
    assert_eq!(run(&mut h, "C-c C-c"), "Done");
    let lines: String = (1..=12).map(|n| format!("{n}\n")).collect();
    assert_eq!(
        h.text(),
        format!(
            "#+begin_src bash\nseq 12\n#+end_src\n\n#+RESULTS:\n#+begin_example\n{lines}#+end_example\n\nafter\n"
        )
    );
    h.eval("(begin (goto-char 17) (delete-region 17 23) (insert \"true\"))");
    assert_eq!(run(&mut h, "C-c C-c"), "The block wrote nothing");
    assert_eq!(h.text(), "#+begin_src bash\ntrue\n#+end_src\n\nafter\n");
}

#[test]
fn a_block_that_fails_says_so_and_leaves_the_text() {
    let text = "#+begin_src sh\necho before; echo 'no such thing' >&2; exit 3\n#+end_src\n";
    let (mut h, _dir) = setup(text);
    goto(&mut h, "echo");
    assert_eq!(
        run(&mut h, "C-c C-c"),
        "sh failed with status 3: no such thing"
    );
    assert_eq!(h.text(), text);

    // A language without a program, and a place that is no block.
    let (mut h, _dir) = setup("#+begin_src cobol\nx\n#+end_src\ntext\n");
    h.keys("C-n C-c C-c");
    assert_eq!(
        h.echo(),
        "There is no program for the language cobol: see the org-src-programs option"
    );
    h.keys("M-> C-p C-c C-v e");
    assert_eq!(h.echo(), "Point is not in a source block");
}

#[test]
fn a_silent_block_shows_what_it_wrote_and_a_block_names_its_directory() {
    let (mut h, dir) = setup("");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    h.paste("#+begin_src sh :results silent :dir sub\nbasename \"$PWD\"\n#+end_src\n");
    let text = h.text();
    h.keys("C-p C-p");
    assert_eq!(run(&mut h, "C-c C-c"), "sub");
    assert_eq!(h.text(), text);
}

#[test]
fn running_a_block_asks_first() {
    let (mut h, dir) = setup("#+begin_src sh\ntouch made\n#+end_src\n");
    h.eval("(set-option! 'org-confirm-run #t)");
    h.keys("C-n C-c C-c");
    assert_eq!(h.prompt(), "Run this block of sh? (y or n)");
    h.type_text("n");
    assert_eq!(h.echo(), "The block was not run");
    assert!(!dir.path().join("made").exists());
    h.keys("C-c C-c");
    h.type_text("y");
    h.wait_until(|h| h.echo() == "The block wrote nothing");
    assert!(dir.path().join("made").exists());
}

#[test]
fn the_blocks_of_a_file_are_written_to_the_files_they_name() {
    let (mut h, dir) = setup(
        "* Setup\n#+begin_src sh :tangle setup.sh\necho one\n#+end_src\ntext\n\
         #+begin_src conf :tangle etc/app.conf :mkdirp yes\n  [app]\n    key = 1\n#+end_src\n\
         #+begin_src sh :tangle no\necho not this\n#+end_src\n#+begin_src sh\necho nor this\n#+end_src\n\
         #+BEGIN_SRC sh :tangle setup.sh\necho two\n#+END_SRC\n#+begin_src python :tangle yes\nprint(1)\n#+end_src\n",
    );
    h.keys("C-c C-v t");
    assert_eq!(h.echo(), "4 blocks written to 3 files");
    let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
    assert_eq!(read("setup.sh"), "echo one\n\necho two\n");
    assert_eq!(read("etc/app.conf"), "[app]\n  key = 1\n");
    assert_eq!(read("notes.python"), "print(1)\n");

    // A directory that is missing is not made unless the block says so.
    let (mut h, dir) = setup("#+begin_src sh :tangle bin/run.sh\necho\n#+end_src\n");
    h.keys("C-c C-v C-t");
    assert!(
        h.echo().starts_with("There is no directory "),
        "{}",
        h.echo()
    );
    assert!(!dir.path().join("bin").exists());

    let (mut h, _dir) = setup("text\n");
    h.keys("C-c C-v t");
    assert_eq!(
        h.echo(),
        "No source block of this buffer names a file with :tangle"
    );
}
