//! Encrypting and decrypting with gpg and age: `runtime/lib/crypt.scm`.
//! The tests make keys of their own, in a directory of their own, and run
//! gpg and age.

mod common;

use common::Harness;
use common::crypt::{GpgHome, age_key, quoted, wait_for};

#[test]
fn a_text_is_encrypted_to_a_gpg_key_and_decrypted() {
    let home = GpgHome::new();
    home.key("Test Person <test@example.org>", "");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.gpg");
    let mut h = Harness::new(80, 8);
    home.used_by(&mut h);

    assert_eq!(h.eval(&format!("(crypt-backend {})", quoted(&file))), "gpg");
    assert_eq!(h.eval("(crypt-backend \"/a/note.txt\")"), "#f");
    let text = "hemligt\\nrad två 😀\\n";
    let wrote = wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"{text}\" '(\"test@example.org\") {})",
            quoted(&file)
        ),
    );
    assert!(!wrote.contains("error"), "{wrote}");
    let bytes = std::fs::read(&file).unwrap();
    assert!(!bytes.is_empty());
    assert!(
        !String::from_utf8_lossy(&bytes).contains("hemligt"),
        "the file is not the text"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "nothing else is left there"
    );
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        format!("\"{text}\"")
    );

    // Who it is for, and who there is.
    let recipients = wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&file)));
    assert!(
        recipients.starts_with("(\"") && recipients.len() == 20,
        "one key id: {recipients}"
    );
    let keys = wait_for(&mut h, "(crypt-keys 'gpg)");
    assert!(
        keys.contains("\"Test Person <test@example.org>\""),
        "{keys}"
    );
    // The id that the file names can be encrypted to again.
    let again = wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"again\" (crypt-recipients {0}) {0})",
            quoted(&file)
        ),
    );
    assert!(!again.contains("error"), "{again}");
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "\"again\""
    );

    // A name that ends in .asc is written as text.
    let armored = dir.path().join("note.asc");
    wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"x\" '(\"test@example.org\") {})",
            quoted(&armored)
        ),
    );
    assert!(
        std::fs::read_to_string(&armored)
            .unwrap()
            .starts_with("-----BEGIN PGP MESSAGE-----")
    );
}

#[test]
fn what_goes_wrong_with_gpg_is_said_in_words() {
    let home = GpgHome::new();
    home.key("Test Person <test@example.org>", "");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("note.gpg");
    let mut h = Harness::new(80, 8);
    home.used_by(&mut h);
    wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"x\" '(\"test@example.org\") {})",
            quoted(&file)
        ),
    );
    let before = std::fs::read(&file).unwrap();

    // Nobody to encrypt to, and somebody who has no key here: the file
    // stays as it was.
    assert_eq!(
        wait_for(
            &mut h,
            &format!("(crypt-encrypt \"y\" '() {})", quoted(&file))
        ),
        "(error \"There is nobody to encrypt note.gpg to\")"
    );
    let wrong = wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"y\" '(\"nobody@example.org\") {})",
            quoted(&file)
        ),
    );
    assert_eq!(
        wrong,
        "(error \"note.gpg: a recipient has no key here that can be used; a key must be there and trusted\")"
    );
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    // Someone else, who has no key for it.
    let other = GpgHome::new();
    other.used_by(&mut h);
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "(error \"note.gpg: no key here can decrypt it\")"
    );
    home.used_by(&mut h);

    // A file that is not what its name says.
    let broken = dir.path().join("broken.gpg");
    std::fs::write(&broken, "not encrypted at all").unwrap();
    let said = wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&broken)));
    assert!(said.starts_with("(error \"broken.gpg: "), "{said}");
    // A name that says nothing.
    assert_eq!(
        wait_for(&mut h, "(crypt-decrypt \"/tmp/plain.txt\")"),
        "(error \"plain.txt is not an encrypted file by its name\")"
    );
    // A program that is not there.
    h.eval("(set-option! 'crypt-gpg-program \"no-such-gpg\")");
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "(error \"note.gpg: cannot run no-such-gpg: No such file or directory (os error 2)\")"
    );
}

#[test]
fn a_passphrase_is_asked_for_in_the_minibuffer_when_that_is_wanted() {
    let home = GpgHome::new();
    home.key("Locked <locked@example.org>", "open sesame");
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("locked.gpg");
    let mut h = Harness::new(80, 8);
    home.used_by(&mut h);
    wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"treasure\" '(\"locked@example.org\") {})",
            quoted(&file)
        ),
    );
    home.forget();
    h.eval("(set-option! 'crypt-pinentry 'loopback)");
    h.eval(&format!(
        "(define got #f)
         (define-command (open-it) \"Decrypt the file.\"
           (set! got (guard (e (#t (error-object-message e)))
                       (crypt-decrypt {}))))",
        quoted(&file)
    ));

    h.keys("M-x").type_text("open-it").keys("RET");
    h.wait_until(|h| h.prompt().starts_with("Passphrase for locked.gpg:"));
    h.type_text("wrong");
    assert_eq!(h.prompt(), "Passphrase for locked.gpg: *****");
    h.keys("RET");
    h.wait_until(|h| h.eval("got") != "#f");
    assert_eq!(h.eval("got"), "\"locked.gpg: the passphrase is wrong\"");

    h.eval("(set! got #f)");
    h.keys("M-x").type_text("open-it").keys("RET");
    h.wait_until(|h| h.prompt().starts_with("Passphrase for locked.gpg:"));
    h.type_text("open sesame").keys("RET");
    h.wait_until(|h| h.eval("got") != "#f");
    assert_eq!(h.eval("got"), "\"treasure\"");
}

#[test]
fn a_text_is_encrypted_with_age_and_decrypted() {
    let dir = tempfile::tempdir().unwrap();
    let (identity, public) = age_key(dir.path(), "key.txt");
    let (stranger, _) = age_key(dir.path(), "other.txt");
    let file = dir.path().join("note.age");
    let mut h = Harness::new(80, 8);
    h.eval("(set-option! 'crypt-age-program \"age\")");
    h.eval(&format!(
        "(set-option! 'crypt-age-identities (list {}))",
        quoted(&identity)
    ));

    assert_eq!(h.eval(&format!("(crypt-backend {})", quoted(&file))), "age");
    let wrote = wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"hemligt\\n\" '(\"{public}\") {})",
            quoted(&file)
        ),
    );
    assert!(!wrote.contains("error"), "{wrote}");
    assert!(!String::from_utf8_lossy(&std::fs::read(&file).unwrap()).contains("hemligt"));
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "\"hemligt\\n\""
    );
    // An age file does not say who it is for.
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-recipients {})", quoted(&file))),
        "#f"
    );
    assert_eq!(h.eval("(crypt-keys 'age)"), "()");

    // Recipients can be a file of public keys.
    let list = dir.path().join("recipients");
    std::fs::write(&list, format!("{public}\n")).unwrap();
    let wrote = wait_for(
        &mut h,
        &format!(
            "(crypt-encrypt \"two\" (list {}) {})",
            quoted(&list),
            quoted(&file)
        ),
    );
    assert!(!wrote.contains("error"), "{wrote}");
    assert_eq!(
        wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file))),
        "\"two\""
    );

    // With another key, it is not read.
    h.eval(&format!(
        "(set-option! 'crypt-age-identities (list {}))",
        quoted(&stranger)
    ));
    let said = wait_for(&mut h, &format!("(crypt-decrypt {})", quoted(&file)));
    assert!(said.starts_with("(error \"note.age: "), "{said}");
}
