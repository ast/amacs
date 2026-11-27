//! Capture and the journal of Org, which are written in Scheme:
//! `runtime/lib/org-capture.scm`.

mod common;

use common::Harness;

/// An editor whose files of Org are in a directory of their own.
fn setup() -> (Harness, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(80, 20);
    h.eval(&format!(
        "(set-option! 'org-directory {0:?})
         (set-option! 'org-journal-dir (string-append {0:?} \"/journal\"))",
        dir.path().display().to_string()
    ));
    (h, dir)
}

/// What a date looks like today, in a format of strftime.
fn today(h: &mut Harness, format: &str) -> String {
    h.eval(&format!("(format-current-time {format:?})"))
        .trim_matches('"')
        .to_owned()
}

#[test]
fn an_entry_goes_under_its_heading_at_the_level_below_it() {
    let mut h = Harness::new(60, 8);
    let mut edit = |text: &str, path: &str, entry: &str, more: &str| {
        h.eval(&format!(
            "(let* ((text {text:?}) (edit (org-capture-edit text {path} {entry:?} {more})))
               (string-append (substring text 0 (car edit)) (cdr edit)
                              (substring text (car edit) (string-length text))))"
        ))
    };
    let text = "intro\n* Inbox\n** old\ntext\n*** deep\n* Other\n";
    // Last under the heading, and first.
    assert_eq!(
        edit(text, "'(\"Inbox\")", "* TODO new\n  more\n** sub", ""),
        r#""intro\n* Inbox\n** old\ntext\n*** deep\n** TODO new\n  more\n*** sub\n* Other\n""#
    );
    assert_eq!(
        edit(text, "'(\"Inbox\")", "* new", "#t"),
        r#""intro\n* Inbox\n** new\n** old\ntext\n*** deep\n* Other\n""#
    );
    // In the file itself: at its end, or before its first heading.
    assert_eq!(
        edit(text, "'()", "*** new", ""),
        r#""intro\n* Inbox\n** old\ntext\n*** deep\n* Other\n* new\n""#
    );
    assert_eq!(
        edit(text, "'()", "* new", "#t"),
        r#""intro\n* new\n* Inbox\n** old\ntext\n*** deep\n* Other\n""#
    );
    // Headings that are missing are made, in a text without a last newline
    // too, and those that are there are used.
    assert_eq!(
        edit(
            "* 2026\n** May\ntext",
            "'(\"2026\" \"June\" \"Day\")",
            "* new",
            ""
        ),
        r#""* 2026\n** May\ntext\n** June\n*** Day\n**** new\n""#
    );
    assert_eq!(
        edit("", "'(\"Tasks\")", "* new\n\n\n", ""),
        r#""* Tasks\n** new\n""#
    );
    // A heading of the same title elsewhere is not the one.
    assert_eq!(
        edit("* A\n** Day\n* B\n", "'(\"B\" \"Day\")", "plain text", ""),
        r#""* A\n** Day\n* B\n** Day\nplain text\n""#
    );
}

#[test]
fn a_template_is_filled_in() {
    let mut h = Harness::new(60, 8);
    let date = today(&mut h, "%Y-%m-%d %a");
    assert_eq!(
        h.eval(r#"(org-capture-expand "* TODO %?\n  %i\n  %a\n  100%% on %u" "" "[[file:a]]")"#),
        format!(r#"("* TODO \n  [[file:a]]\n  100% on [{date}]" . 7)"#)
    );
    assert_eq!(
        h.eval(r#"(org-capture-expand "* %t %i!" "words" "")"#),
        format!(r#"("* <{date}> words!" . #f)"#)
    );
}

#[test]
fn an_entry_is_captured_to_a_file_that_is_not_open() {
    let (mut h, dir) = setup();
    h.eval(
        r#"(set-option! 'org-capture-templates
             '(("t" "Todo" (file+headline "tasks/inbox.org" "Inbox") "* TODO %?\n  %i\n  %a" prepend #t)
               ("j" "Journal" (file+datetree "journal.org") "* %?\nEntered on %U")))"#,
    );
    std::fs::create_dir(dir.path().join("tasks")).unwrap();
    let inbox = dir.path().join("tasks/inbox.org");
    std::fs::write(&inbox, "* Inbox\n** TODO old\n").unwrap();

    h.paste("some text here");
    h.keys("C-c x");
    assert_eq!(h.prompt(), "Capture:");
    h.type_text("t");
    assert_eq!(h.eval("(buffer-name)"), "\"*capture*\"");
    assert!(
        h.mode_line().contains("[Todo for inbox.org]"),
        "{}",
        h.mode_line()
    );
    assert_eq!(h.screen.cursor(), Some((0, 7)));
    h.type_text("Call back");
    h.keys("C-c C-c");
    assert!(h.echo().starts_with("Captured to "), "{}", h.echo());
    assert_eq!(
        std::fs::read_to_string(&inbox).unwrap(),
        "* Inbox\n** TODO Call back\n** TODO old\n"
    );
    // Back where the capture started, with no buffer left over.
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    assert_eq!(h.eval("(member \"*capture*\" (buffer-list))"), "#f");
    assert_eq!(h.eval("(member \"inbox.org\" (buffer-list))"), "#f");

    // A file that is not there is made, with the headings of the day.
    h.keys("C-c x").type_text("j");
    h.type_text("A thought");
    h.keys("C-c C-c");
    let (year, month, day) = (
        today(&mut h, "%Y"),
        today(&mut h, "%Y-%m %B"),
        today(&mut h, "%Y-%m-%d %A"),
    );
    let journal = std::fs::read_to_string(dir.path().join("journal.org")).unwrap();
    assert!(
        journal.starts_with(&format!(
            "* {year}\n** {month}\n*** {day}\n**** A thought\nEntered on ["
        )),
        "{journal}"
    );
    // A second entry of the day goes under the same headings.
    h.keys("C-c x").type_text("j");
    h.type_text("Another");
    h.keys("C-c C-c");
    let journal = std::fs::read_to_string(dir.path().join("journal.org")).unwrap();
    assert_eq!(
        journal.matches(&format!("*** {day}\n")).count(),
        1,
        "{journal}"
    );
    assert!(
        journal.contains("]\n**** Another\nEntered on ["),
        "{journal}"
    );
}

#[test]
fn a_capture_from_a_file_links_to_it_and_takes_the_region() {
    let (mut h, dir) = setup();
    h.eval(
        r#"(set-option! 'org-capture-templates
             '(("t" "Todo" (file "inbox.org") "* TODO %?\n  %i\n  %a")))"#,
    );
    let notes = dir.path().join("notes.org");
    std::fs::write(&notes, "* One\n** Two :tag:\nthe words to keep\n").unwrap();
    h.eval(&format!("(find-file {:?})", notes.display().to_string()));
    h.eval("(begin (goto-char 20) (set-mark! 23) (activate-mark!) (goto-char 28))");
    h.keys("C-c x").type_text("t");
    let name = h.eval(&format!(
        "(abbreviate-file-name {:?})",
        notes.display().to_string()
    ));
    let name = name.trim_matches('"');
    assert_eq!(
        h.text(),
        format!("* TODO \n  words\n  [[file:{name}::*Two][Two]]")
    );
    // The file of the entry is open here: its buffer gets the entry and is
    // saved.
    let inbox = dir.path().join("inbox.org");
    std::fs::write(&inbox, "old\n").unwrap();
    h.eval(&format!(
        "(begin (find-file {:?}) (switch-to-buffer \"*capture*\"))",
        inbox.display().to_string()
    ));
    h.type_text("Read");
    h.keys("C-c C-c");
    let wanted = format!("old\n* TODO Read\n  words\n  [[file:{name}::*Two][Two]]\n");
    assert_eq!(std::fs::read_to_string(&inbox).unwrap(), wanted);
    assert_eq!(h.eval("(buffer-name)"), "\"notes.org\"");
    assert_eq!(
        h.eval("(buffer-string \"inbox.org\")"),
        format!("{wanted:?}")
    );
    assert_eq!(h.eval("(buffer-modified? \"inbox.org\")"), "#f");
}

#[test]
fn a_capture_can_be_abandoned() {
    let (mut h, dir) = setup();
    h.keys("C-c x").type_text("t");
    h.type_text("Not this");
    h.keys("C-c C-k");
    assert_eq!(h.echo(), "The entry was abandoned");
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    assert!(!dir.path().join("notes.org").exists());

    // An empty entry is not captured, and a key that is no template is an
    // error.
    h.keys("C-c x").type_text("t");
    h.eval("(delete-region 0 (point-max))");
    h.keys("C-c C-c");
    assert_eq!(h.echo(), "The entry is empty");
    h.keys("C-c C-k C-c x").type_text("x");
    assert_eq!(h.echo(), "x is not in the menu");
    h.keys("C-c C-c");
    assert!(!dir.path().join("notes.org").exists());
}

#[test]
fn the_journal_of_today_gets_an_entry_with_the_time() {
    let (mut h, dir) = setup();
    h.keys("C-c j");
    let (file, date) = (today(&mut h, "%Y%m%d"), today(&mut h, "%A, %d %B %Y"));
    assert_eq!(h.eval("(buffer-name)"), format!("{file:?}"));
    assert!(h.mode_line().contains("(Org)"), "{}", h.mode_line());
    let text = h.text();
    assert!(text.starts_with(&format!("* {date}\n** ")), "{text}");
    assert_eq!(text.len(), date.len() + 3 + 3 + 6, "{text}");
    h.type_text("First");
    h.keys("C-x C-s C-c j");
    h.type_text("Second");
    let text = h.text();
    assert_eq!(text.matches("\n** ").count(), 2, "{text}");
    assert!(text.ends_with(" Second"), "{text}");
    assert!(text.contains(" First\n** "), "{text}");
    assert!(dir.path().join("journal").join(&file).exists());
}

#[test]
fn a_file_whose_bytes_are_not_its_text_is_written_as_saving_writes_it() {
    let (mut h, dir) = setup();
    // Files that end in .rev hold their text backwards.
    h.eval(
        r#"(define-file-handler ".rev"
             (lambda (file) (list->string (reverse (string->list (read-file file)))))
             (lambda (file text) (write-file file (list->string (reverse (string->list text))))))
           (set-option! 'org-capture-templates
             '(("t" "Todo" (file+headline "inbox.org.rev" "Inbox") "* TODO %?")))"#,
    );
    let file = dir.path().join("inbox.org.rev");
    let backwards = |text: &str| text.chars().rev().collect::<String>();
    std::fs::write(&file, backwards("* Inbox\n** TODO old\n")).unwrap();

    h.keys("C-c x").type_text("t");
    h.type_text("new");
    h.keys("C-c C-c");
    h.wait_until(|h| h.echo().starts_with("Captured to "));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        backwards("* Inbox\n** TODO old\n** TODO new\n")
    );
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
}
