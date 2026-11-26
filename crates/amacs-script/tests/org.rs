//! Org, which is written in Scheme, `runtime/lib/org.scm`, but for its
//! colours, which are in `crates/amacs-treesit/src/org.rs`.

mod common;

use common::Harness;

#[test]
fn an_org_file_opens_in_org_mode_with_its_colours() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.org");
    std::fs::write(
        &file,
        "#+title: Notes\n* TODO One :tag:\n** DONE Two\nSee ~code~ and [[https://example.org][a site]].\n\
         #+begin_src sh\necho hi # there\n#+end_src\n",
    )
    .unwrap();
    let mut h = Harness::new(60, 12);
    h.eval(&format!("(find-file {:?})", file.display().to_string()));
    assert!(h.mode_line().contains("(Org)"), "{}", h.mode_line());

    assert_eq!(h.screen.face_at(0, 0), h.face("prose-metadata"));
    assert_eq!(h.screen.face_at(0, 9), h.face("heading"));
    assert_eq!(h.screen.face_at(1, 0), h.face("heading"));
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-todo"));
    assert_eq!(h.screen.face_at(1, 11), h.face("prose-tag"));
    assert_eq!(h.screen.face_at(2, 0), h.face("heading-2"));
    assert_eq!(h.screen.face_at(2, 3), h.face("prose-done"));
    assert_eq!(h.screen.face_at(3, 5), h.face("code"));
    assert_eq!(h.screen.face_at(3, 38), h.face("link"));
    assert_eq!(h.screen.face_at(4, 0), h.face("prose-metadata"));
    assert_eq!(h.screen.face_at(5, 0), h.face("function-name"));
    assert_eq!(h.screen.face_at(5, 8), h.face("comment"));
    assert_eq!(
        h.eval("(begin (goto-char 3) (syntax-at))"),
        "\"prose-metadata\""
    );

    // The words for the states of a heading are those that the buffer is
    // given.
    h.eval("(set-syntax-keywords! '(\"WAIT\") '(\"TODO\"))");
    assert_eq!(h.screen.face_at(1, 2), h.face("prose-done"));
    assert_eq!(h.screen.face_at(2, 3), h.face("heading-2"));
}
