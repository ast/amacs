//! Encrypted files, visited and saved as any file:
//! `runtime/lib/crypt-file.scm`.

mod common;

use common::Harness;
use common::crypt::{GpgHome, age_key, quoted, wait_for};

/// Visits a file by typing its name.
fn visit(h: &mut Harness, path: &std::path::Path) {
    h.keys("C-x C-f C-a C-k")
        .type_text(path.to_str().unwrap())
        .keys("RET");
    let name = path.file_name().unwrap().to_str().unwrap().to_owned();
    h.wait_until(|h| h.mode_line().contains(&name));
}

#[test]
fn an_encrypted_file_is_visited_edited_and_saved() {
    let home = GpgHome::new();
    home.key("Test Person <test@example.org>", "");
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let file = dir.path().join("notes.md.gpg");
    let mut h = Harness::new(80, 8);
    h.session
        .editor_mut()
        .set_state_directory(Some(state.clone()));
    home.used_by(&mut h);
    wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"# Hemligt\\n\" '(\"test@example.org\") {})",
            quoted(&file)
        ),
    );

    visit(&mut h, &file);
    assert_eq!(h.text(), "# Hemligt\n");
    assert!(h.mode_line().contains("(Markdown"), "{}", h.mode_line());
    assert_eq!(h.eval("(option 'buffer-secret)"), "#t");
    assert_eq!(h.eval("(buffer-modified?)"), "#f");
    assert_eq!(h.eval("(minor-mode-enabled? 'spell-mode)"), "#f");

    h.keys("M->").type_text("lösenord");
    // Nothing of it goes to the editor's own files.
    assert_eq!(h.session.editor_mut().auto_save(true), 0);
    assert!(!state.join("auto-save").exists());
    assert!(!state.join("undo").exists());
    h.keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote "));
    assert_eq!(h.eval("(buffer-modified?)"), "#f");
    let bytes = std::fs::read(&file).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Hemligt"));
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "the file, and nothing beside it"
    );

    // Another editor reads what was saved.
    let mut h = Harness::new(80, 8);
    home.used_by(&mut h);
    visit(&mut h, &file);
    assert_eq!(h.text(), "# Hemligt\nlösenord");
}

#[test]
fn a_new_encrypted_file_is_for_who_the_directory_says_or_who_is_asked_for() {
    let home = GpgHome::new();
    home.key("Test Person <test@example.org>", "");
    home.key("Other Person <other@example.org>", "");
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(80, 12);
    home.used_by(&mut h);

    // Nothing says who: the user is asked, with the keys there are.
    let alone = dir.path().join("alone.gpg");
    visit(&mut h, &alone);
    assert_eq!(h.echo(), "(New file)");
    h.type_text("one").keys("C-x C-s");
    h.wait_until(|h| h.prompt().contains("Encrypt alone.gpg to:"));
    h.type_text("Other").keys("RET");
    h.wait_until(|h| h.echo().starts_with("Wrote "));
    let other = wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&alone)));
    // Saved again, it is for who it was for, without a question.
    h.type_text(" more").keys("C-x C-s");
    h.wait_until(|h| h.eval("(buffer-modified?)") == "#f");
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&alone))),
        other
    );
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&alone))),
        "\"one more\""
    );

    // A .gpg-id above the file decides, as in the password store, also
    // against who the file was for.
    let store = dir.path().join("store");
    std::fs::create_dir_all(store.join("sub")).unwrap();
    std::fs::write(store.join(".gpg-id"), "# who\ntest@example.org\n\n").unwrap();
    let entry = store.join("sub/entry.gpg");
    wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"old\" '(\"other@example.org\") {})",
            quoted(&entry)
        ),
    );
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&entry))),
        other
    );
    visit(&mut h, &entry);
    assert_eq!(h.text(), "old");
    h.type_text("new ").keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote "));
    let now = wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&entry)));
    assert_ne!(now, other, "it is for the one in .gpg-id now");
    assert_eq!(
        h.eval(&format!("(crypt-file-recipients {})", quoted(&entry))),
        "(\"test@example.org\")"
    );
}

#[test]
fn an_age_file_is_visited_and_saved_and_the_file_manager_opens_such_files() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, public) = age_key(dir.path(), "key.txt");
    let store = dir.path().join("store");
    std::fs::create_dir(&store).unwrap();
    std::fs::write(store.join(".age-recipients"), format!("{public}\n")).unwrap();
    let file = store.join("entry.age");
    let mut h = Harness::new(80, 10);
    h.eval("(set-option! 'crypt-age-program \"age\")");
    h.eval("(set-option! 'dired-icons #f)");
    h.eval(&format!(
        "(set-option! 'crypt-age-identities (list {}))",
        quoted(&identity)
    ));
    wait_for(
        &mut h,
        &format!("(crypt-encrypt \"pw\" '(\"{public}\") {})", quoted(&file)),
    );

    // RET in the file manager visits it: it is not handed to another
    // program as a file that is not text.
    h.eval(&format!(
        "(dired-open {} {})",
        quoted(&store),
        quoted(&file)
    ));
    h.keys("RET");
    h.wait_until(|h| h.mode_line().contains("entry.age"));
    assert_eq!(h.text(), "pw");
    assert_eq!(h.eval("(option 'buffer-secret)"), "#t");
    h.keys("M->").type_text("2").keys("C-x C-s");
    h.wait_until(|h| h.echo().starts_with("Wrote "));
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "\"pw2\""
    );
}
