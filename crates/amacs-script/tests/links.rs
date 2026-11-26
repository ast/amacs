//! Following links, which is written in Scheme: `runtime/lib/links.scm`,
//! with what Markdown knows of links in `runtime/lib/markdown.scm`.

mod common;

use std::path::{Path, PathBuf};

use common::Harness;

/// An editor with a Markdown buffer that holds a text, point at its start.
fn setup(text: &str) -> Harness {
    let mut h = Harness::new(80, 16);
    h.eval(r#"(switch-to-buffer "notes.md") (markdown-mode)"#);
    h.paste(text);
    h.keys("M-<");
    h
}

/// Puts point inside the first occurrence of a text in the buffer.
fn goto(h: &mut Harness, text: &str) {
    h.eval(&format!(
        "(goto-char (+ 1 (string-search (buffer-string) {text:?})))"
    ));
}

/// A program that writes what it is given to a file, as the program that
/// opens addresses, and the file it writes to.
fn recording_opener(h: &mut Harness, dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let (program, record) = (dir.join("opener"), dir.join("opened"));
    std::fs::write(
        &program,
        format!("#!/bin/sh\necho \"$1\" >> {}\n", record.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    h.eval(&format!(
        "(set-option! 'dired-open-program {:?})",
        program.display().to_string()
    ));
    record
}

fn wait_for_record(record: &Path, wanted: &str) {
    let start = std::time::Instant::now();
    loop {
        let found = std::fs::read_to_string(record).unwrap_or_default();
        if found == wanted {
            return;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "the opener was given {found:?}, not {wanted:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_web_address_goes_to_the_open_program() {
    let tools = tempfile::tempdir().unwrap();
    let mut h = setup("Read [the manual](https://example.org/manual \"Manual\") first.\n");
    let record = recording_opener(&mut h, tools.path());
    goto(&mut h, "manual");
    h.keys("C-c C-o");
    wait_for_record(&record, "https://example.org/manual\n");
    assert_eq!(h.echo(), "Opened https://example.org/manual");

    // The program for addresses can be another than the one for files.
    let other = tools.path().join("other");
    std::fs::write(&other, "").unwrap();
    h.eval(&format!(
        "(set-option! 'browse-url-program \"touch\") (browse-url {:?})",
        other.join("made").display().to_string()
    ));
    assert_eq!(
        std::fs::read_to_string(&record).unwrap(),
        "https://example.org/manual\n"
    );
}

#[test]
fn an_address_written_out_is_a_link_in_any_mode() {
    let mut h = Harness::new(80, 10);
    h.paste("See https://en.wikipedia.org/wiki/Scheme_(language), or (https://example.org/a).\n");
    let at = |h: &mut Harness, text: &str| {
        goto(h, text);
        h.eval("(link-at)")
    };
    assert_eq!(
        at(&mut h, "wikipedia"),
        "(4 51 \"https://en.wikipedia.org/wiki/Scheme_(language)\" #f)"
    );
    assert_eq!(
        at(&mut h, "example"),
        "(57 78 \"https://example.org/a\" #f)"
    );
    assert_eq!(at(&mut h, "See"), "#f");

    // The end of an address is still the address.
    assert_eq!(h.eval("(caddr (url-at 78))"), "\"https://example.org/a\"");
    assert_eq!(h.eval("(url-at 79)"), "#f");
}

#[test]
fn a_reference_link_is_followed_through_its_definition() {
    let tools = tempfile::tempdir().unwrap();
    let mut h = setup(
        "The [plan][Site] and the [notes] and [none].\n\n\
         [site]: <https://example.org/plan>\n[notes]: https://example.org/notes\n",
    );
    let record = recording_opener(&mut h, tools.path());
    goto(&mut h, "plan");
    h.keys("C-c C-o");
    wait_for_record(&record, "https://example.org/plan\n");
    goto(&mut h, "notes]");
    h.keys("C-c C-o");
    wait_for_record(
        &record,
        "https://example.org/plan\nhttps://example.org/notes\n",
    );

    // The line that gives the address is a link too.
    goto(&mut h, "[site]:");
    assert_eq!(h.eval("(caddr (link-at))"), "\"https://example.org/plan\"");

    // Brackets without an address are text.
    goto(&mut h, "none");
    h.keys("C-c C-o");
    assert_eq!(h.echo(), "There is no link at point");
}

#[test]
fn a_link_to_a_heading_goes_to_it_and_leaves_the_mark() {
    let mut h = setup("See [below](#the-second-part).\n\n# First\n\n## The second part!\n\ntext\n");
    goto(&mut h, "below");
    let from = h.eval("(point)");
    h.keys("C-c C-o");
    assert_eq!(h.eval("(line-number-at-pos)"), "5");
    assert_eq!(h.eval("(mark)"), from);

    h.keys("M-<");
    h.paste("[gone](#nowhere) ");
    h.keys("M-< C-c C-o");
    assert_eq!(h.echo(), "Found no place for #nowhere");
}

#[test]
fn a_link_to_a_file_visits_it_at_the_place_it_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(
        root.join("a.md"),
        "[other](sub/b.md#second-part) [plain](sub/b.md) [new](new.md)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("sub/b.md"),
        "# First\n\none\n\n# Second part\n\ntwo words\n",
    )
    .unwrap();
    let mut h = Harness::new(80, 12);
    let visit = |h: &mut Harness| {
        h.eval(&format!(
            "(find-file {:?})",
            root.join("a.md").display().to_string()
        ));
    };

    visit(&mut h);
    goto(&mut h, "other");
    h.keys("C-c C-o");
    assert_eq!(h.eval("(buffer-name)"), "\"b.md\"");
    assert_eq!(h.eval("(line-number-at-pos)"), "5");

    visit(&mut h);
    goto(&mut h, "plain");
    h.keys("C-c C-o");
    assert_eq!(h.eval("(buffer-name)"), "\"b.md\"");

    // A file that is not there yet is a new one.
    visit(&mut h);
    goto(&mut h, "new");
    h.keys("C-c C-o");
    assert_eq!(h.eval("(buffer-name)"), "\"new.md\"");
    assert_eq!(h.text(), "");

    // A target can name a line, or text to look for; a directory is shown
    // in the file manager.
    visit(&mut h);
    h.eval("(open-link \"file:sub/b.md::3\")");
    assert_eq!(h.eval("(line-number-at-pos)"), "3");
    visit(&mut h);
    h.eval("(open-link \"./sub/b.md::two words\")");
    assert_eq!(h.eval("(line-number-at-pos)"), "7");
    visit(&mut h);
    h.eval("(open-link \"file:sub\")");
    assert_eq!(h.eval("(major-mode)"), "dired-mode");
}

#[test]
fn a_kind_of_link_can_be_added() {
    let mut h = setup("[the ticket](ticket:42)\n");
    h.eval(
        "(define-link-type \"ticket\"
           (lambda (number) (message \"Ticket \" number)))",
    );
    goto(&mut h, "ticket");
    h.keys("C-c C-o");
    assert_eq!(h.echo(), "Ticket 42");
}

#[test]
fn the_middle_button_follows_a_link() {
    let mut h = setup("# Top\n\nBack to [the top](#top), please.\n");
    h.keys("M->");
    h.press("<mouse-2>", 12, 2);
    assert_eq!(h.eval("(point)"), "0");

    // Where there is no link, the button only puts point there.
    h.press("C-<mouse-1>", 2, 2);
    assert_eq!(h.eval("(point)"), "9");
    assert_eq!(h.echo(), "");
}
