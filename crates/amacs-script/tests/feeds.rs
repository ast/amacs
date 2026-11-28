//! The reader of feeds, which is written in Scheme: `runtime/lib/feeds.scm`.
//! The feeds are files, fetched by curl as file:// addresses, so that no
//! network is needed.

mod common;

use std::path::Path;

use common::Harness;

/// An RFC 3339 time some days ago.
fn days_ago(days: i64) -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * days)).to_string()
}

/// Writes two feeds to a directory: an Atom one of two entries, and an
/// RSS one of one entry with HTML content.
fn write_feeds(dir: &Path) {
    std::fs::write(
        dir.join("blog.xml"),
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Rust Blog</title>
<entry><id>r1</id><title>Rust released</title><link href="https://blog.example/r1"/>
<updated>{}</updated><content type="html">&lt;p&gt;Big news.&lt;/p&gt;</content></entry>
<entry><id>r2</id><title>Ancient post</title><link href="https://blog.example/r2"/>
<updated>{}</updated></entry></feed>"#,
            days_ago(1),
            days_ago(400)
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("planet.xml"),
        format!(
            r#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Planet Test</title><link>https://planet.example/</link>
<item><title>Org mode tips</title><link>https://planet.example/org</link><guid>org</guid>
<pubDate>{}</pubDate>
<description>&lt;p&gt;Some &lt;b&gt;bold&lt;/b&gt; text and &lt;a href="/more"&gt;a link&lt;/a&gt;.&lt;/p&gt;</description>
</item></channel></rss>"#,
            jiff::fmt::rfc2822::to_string(
                &(jiff::Timestamp::now() - jiff::SignedDuration::from_hours(72))
                    .to_zoned(jiff::tz::TimeZone::UTC)
            )
            .unwrap()
        ),
    )
    .unwrap();
}

/// An editor whose feeds are the two of `write_feeds` in `dir`, kept in a
/// database in `dir` too, with no filter but +unread.
fn harness(dir: &Path) -> Harness {
    let mut h = Harness::new(100, 20);
    let url = |name: &str| format!("file://{}", dir.join(name).display());
    h.eval(&format!(
        "(set-option! 'feeds-directory {:?})
         (set-option! 'feeds-default-filter \"+unread\")
         (set-option! 'feeds '(({:?} rust news) ({:?} emacs)))",
        dir.join("db").display().to_string(),
        url("blog.xml"),
        url("planet.xml"),
    ));
    h
}

fn fetch(h: &mut Harness) {
    h.keys("C-x w G");
    h.wait_until(|h| h.echo().starts_with("Feeds: "));
}

fn rows(h: &Harness) -> Vec<String> {
    h.text().lines().map(str::to_owned).collect()
}

#[test]
fn entries_are_listed_newest_first_once() {
    let dir = tempfile::tempdir().unwrap();
    write_feeds(dir.path());
    let mut h = harness(dir.path());
    fetch(&mut h);
    assert_eq!(h.echo(), "Feeds: 3 new entries");
    let rows = rows(&h);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[0].contains("Rust released") && rows[0].contains("Rust Blog"));
    assert!(rows[0].contains("(unread rust news)"), "{}", rows[0]);
    assert!(rows[1].contains("Org mode tips") && rows[1].contains("Planet Test"));
    assert!(rows[2].contains("Ancient post"));
    assert!(
        h.mode_line().contains("+unread  3 shown"),
        "{}",
        h.mode_line()
    );

    // A second fetch finds the same entries.
    h.keys("G");
    h.wait_until(|h| h.echo() == "Feeds: 0 new entries");
    assert_eq!(h.text().lines().count(), 3);
}

#[test]
fn filters_choose_entries() {
    let dir = tempfile::tempdir().unwrap();
    write_feeds(dir.path());
    let mut h = harness(dir.path());
    fetch(&mut h);
    let titles = |h: &mut Harness, filter: &str| -> Vec<String> {
        let found = h.eval(&format!("(map feed-entry-title (feeds-search {filter:?}))"));
        found
            .trim_matches(|c| c == '(' || c == ')')
            .split("\" \"")
            .map(|title| title.trim_matches('"').to_owned())
            .filter(|title| !title.is_empty())
            .collect()
    };
    assert_eq!(titles(&mut h, "+emacs"), ["Org mode tips"]);
    assert_eq!(titles(&mut h, "-emacs"), ["Rust released", "Ancient post"]);
    assert_eq!(titles(&mut h, "=planet"), ["Org mode tips"]);
    assert_eq!(titles(&mut h, "~planet"), ["Rust released", "Ancient post"]);
    assert_eq!(
        titles(&mut h, "@1-week-ago"),
        ["Rust released", "Org mode tips"]
    );
    assert_eq!(titles(&mut h, "@1-week-ago--2-days-ago"), ["Org mode tips"]);
    assert_eq!(titles(&mut h, "#2"), ["Rust released", "Org mode tips"]);
    assert_eq!(titles(&mut h, "RELEASED"), ["Rust released"]);
    assert_eq!(titles(&mut h, "!released +rust"), ["Ancient post"]);
    assert!(titles(&mut h, "@soon").is_empty());

    // s changes the filter of the list, which follows it as it is typed;
    // C-g gives back the one before.
    h.keys("s C-a C-k").type_text("+emacs");
    assert_eq!(rows(&h).len(), 1);
    h.keys("C-g");
    assert_eq!(rows(&h).len(), 3);
    assert!(
        h.mode_line().contains("+unread  3 shown"),
        "{}",
        h.mode_line()
    );
    // A filter that is not whole yet leaves the list as it was.
    h.keys("s C-a C-k").type_text("+emacs (");
    assert_eq!(rows(&h).len(), 1);
    h.keys("DEL DEL RET");
    // c sets the default one again.
    assert_eq!(rows(&h).len(), 1);
    assert!(
        h.mode_line().contains("+emacs  1 shown"),
        "{}",
        h.mode_line()
    );
    h.keys("c");
    assert_eq!(rows(&h).len(), 3);
}

#[test]
fn tags_are_changed_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    write_feeds(dir.path());
    let mut h = harness(dir.path());
    fetch(&mut h);
    // r on a region of two lines marks both read. They stay listed, as in
    // elfeed, until g draws the list anew.
    h.keys("M-< C-SPC n n r");
    let marked = rows(&h);
    assert_eq!(marked.len(), 3, "{marked:?}");
    assert!(
        marked[0].ends_with("(rust news)") && marked[1].ends_with("(emacs)"),
        "{marked:?}"
    );
    h.keys("+").type_text("starred").keys("RET");
    assert!(
        rows(&h)[2].contains("(unread rust news starred)"),
        "{:?}",
        rows(&h)
    );
    h.keys("g");
    let left = rows(&h);
    assert_eq!(left.len(), 1, "{left:?}");
    assert!(left[0].contains("Ancient post"));
    h.eval("(run-later feeds-save!)");
    h.wait_until(|_| {
        std::fs::read_to_string(dir.path().join("db/index.scm"))
            .is_ok_and(|index| index.contains("starred"))
    });

    // Another editor reads the same.
    let mut again = harness(dir.path());
    again.keys("C-x w");
    let kept = rows(&again);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].contains("Ancient post") && kept[0].contains("starred"));
    again
        .keys("s C-a C-k")
        .type_text("+starred -unread")
        .keys("RET");
    assert!(rows(&again).is_empty());
}

#[test]
fn an_entry_is_shown_and_its_links_followed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    write_feeds(dir.path());
    let mut h = harness(dir.path());
    let (opener, record) = (dir.path().join("opener"), dir.path().join("opened"));
    std::fs::write(
        &opener,
        format!("#!/bin/sh\necho \"$1\" >> {}\n", record.display()),
    )
    .unwrap();
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
    h.eval(&format!(
        "(set-option! 'browse-url-program {:?})",
        opener.display().to_string()
    ));
    fetch(&mut h);

    h.keys("n RET");
    let text = h.text();
    assert!(text.starts_with("Title: Org mode tips\n"), "{text}");
    assert!(
        text.contains("Feed: Planet Test\nLink: https://planet.example/org\nTags: emacs\n"),
        "{text}"
    );
    assert!(text.contains("\nSome bold text and a link.\n"), "{text}");
    assert_eq!(
        h.eval("(map caddr %feeds-shown-links)"),
        "(\"https://planet.example/more\")"
    );

    // TAB goes to the link and RET opens it.
    h.keys("TAB RET");
    h.wait_until(|_| {
        std::fs::read_to_string(&record)
            .is_ok_and(|opened| opened == "https://planet.example/more\n")
    });

    // The entry is read now; n shows the next one of the list, and q goes
    // back to it.
    h.keys("n");
    assert!(
        h.text().starts_with("Title: Ancient post\n"),
        "{}",
        h.text()
    );
    assert!(h.text().contains("(The entry has no content.)"));
    h.keys("q");
    let rows = rows(&h);
    assert!(
        rows[0].contains("(unread rust news)") && rows[1].contains("(emacs)"),
        "{rows:?}"
    );
}

#[test]
fn a_feed_that_fails_is_reported_and_the_others_are_fetched() {
    let dir = tempfile::tempdir().unwrap();
    write_feeds(dir.path());
    let mut h = harness(dir.path());
    h.eval(&format!(
        "(set-option! 'feeds (cons '(\"file://{}/missing.xml\") (option 'feeds)))",
        dir.path().display()
    ));
    fetch(&mut h);
    assert_eq!(
        h.echo(),
        "Feeds: 3 new entries, 1 with problems (M-x feeds-show-errors)"
    );
    h.keys("M-x").type_text("feeds-show-errors").keys("RET");
    assert!(
        h.text().contains("missing.xml\n  curl: (37)"),
        "{}",
        h.text()
    );
}
