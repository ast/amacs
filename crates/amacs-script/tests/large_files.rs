//! Files that take a while to read are read on a thread of their own and
//! shown when they are there, as files on other hosts are.

mod common;

use std::path::Path;

use amacs_proto::{InputEvent, parse_keys};
use common::Harness;

/// A file of Rust that is large enough to be read in the background, and
/// a small one beside it.
fn files() -> (tempfile::TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let line = "fn main() { let answer = 42; println!(\"{answer}\"); }\n";
    let big = dir.path().join("big.rs");
    std::fs::write(&big, line.repeat((5 << 20) / line.len())).unwrap();
    let small = dir.path().join("small.rs");
    std::fs::write(&small, line).unwrap();
    let name = |path: &Path| path.display().to_string();
    (dir, name(&big), name(&small))
}

#[test]
fn a_large_file_is_read_in_the_background_and_shown_when_it_is_there() {
    let (_dir, big, small) = files();
    let mut h = Harness::new(100, 12);
    assert_eq!(
        h.eval(&format!(
            "(list (%file-read-in-background? {big:?}) (%file-read-in-background? {small:?}))"
        )),
        "(#t #f)"
    );
    h.keys("C-x C-f C-a C-k");
    h.type_text(&big);
    h.keys("RET");
    // The file is not there at once, and what is typed waits for it.
    assert_eq!(h.eval("(buffer-name)"), "\"*scratch*\"");
    h.type_text("// typed\n");
    h.wait_until(|h| h.eval("(buffer-name)") == "\"big.rs\"");
    h.wait_until(|h| h.text().starts_with("// typed\nfn main()"));
    // It is a file as any other: with the mode of its name.
    h.wait_until(|h| h.eval("(major-mode)") == "rust-mode");
    assert!(h.mode_line().contains("(Rust"), "{}", h.mode_line());
    // Now that a buffer visits it, it is shown at once.
    assert_eq!(
        h.eval(&format!("(%file-read-in-background? {big:?})")),
        "#f"
    );
    h.eval("(switch-to-buffer \"*scratch*\")");
    h.eval(&format!("(find-file {big:?})"));
    assert_eq!(h.eval("(buffer-name)"), "\"big.rs\"");
}

#[test]
fn scheme_visits_a_large_file_and_goes_on_when_it_is_read() {
    let (_dir, big, _) = files();
    let mut h = Harness::new(100, 12);
    h.eval(&format!(
        "(define seen #f)
         (run-later (lambda ()
           (find-file {big:?})
           (set! seen (list (buffer-name) (> (point-max) 5000000)))))"
    ));
    h.wait_until(|h| h.eval("seen") != "#f");
    assert_eq!(h.eval("seen"), "(\"big.rs\" #t)");
}

#[test]
fn a_file_read_in_the_background_gets_its_mode_in_its_own_frame() {
    let (_dir, big, _) = files();
    let mut h = Harness::new(100, 12);
    let first = h.session.editor().current_frame();
    let second = h.session.editor_mut().create_frame(80, 10);
    for key in parse_keys("C-x C-f C-a C-k").unwrap() {
        h.session.handle_frame_event(first, InputEvent::Key(key));
    }
    for c in big.chars() {
        let key = parse_keys(&c.to_string()).unwrap()[0];
        h.session.handle_frame_event(first, InputEvent::Key(key));
    }
    h.session
        .handle_frame_event(first, InputEvent::Key(parse_keys("RET").unwrap()[0]));
    // The user goes on in another frame while the file is read.
    h.session
        .handle_frame_event(second, InputEvent::Key(parse_keys("C-f").unwrap()[0]));
    let started = std::time::Instant::now();
    loop {
        h.session.poll();
        h.session.editor_mut().select_frame(first);
        let shown = h.session.eval("(list (buffer-name) (major-mode))").unwrap();
        h.session.editor_mut().select_frame(second);
        if shown == "(\"big.rs\" rust-mode)" {
            break;
        }
        assert!(
            started.elapsed().as_secs() < 10,
            "the first frame shows {shown}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // The other frame is as it was.
    assert_eq!(h.session.eval("(buffer-name)").unwrap(), "\"*scratch*\"");
}
