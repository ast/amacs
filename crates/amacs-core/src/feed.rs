//! Feeds of news: RSS 0.9x, 1.0 and 2.0, Atom and JSON Feed, read by
//! feed-rs into what a reader of feeds keeps of them. Character encodings
//! and the many ways of writing dates are feed-rs's business.

/// A feed and its entries.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Feed {
    pub title: Option<String>,
    /// The address of the site that the feed is of.
    pub link: Option<String>,
    pub entries: Vec<Entry>,
}

/// An entry of a feed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entry {
    /// What tells the entry from the others of its feed, stable from one
    /// fetch to the next.
    pub id: String,
    pub title: String,
    pub link: Option<String>,
    pub author: Option<String>,
    /// When the entry was published or, if not said, last changed, in
    /// seconds since 1970.
    pub date: Option<i64>,
    /// The entry's content, or its summary if it has no content, as HTML.
    pub content: Option<String>,
}

/// Reads a feed. `url`, where it was fetched from, makes relative links
/// whole.
pub fn parse_feed(bytes: &[u8], url: Option<&str>) -> Result<Feed, String> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(url)
        .build()
        .parse(bytes)
        .map_err(|error| error.to_string())?;
    Ok(Feed {
        title: feed.title.as_ref().map(text),
        link: link(&feed.links),
        entries: feed
            .entries
            .into_iter()
            .map(|entry| Entry {
                title: entry.title.as_ref().map(text).unwrap_or_default(),
                link: link(&entry.links),
                author: entry
                    .authors
                    .iter()
                    .filter_map(|person| person.name.as_deref())
                    .map(str::trim)
                    .find(|name| !name.is_empty())
                    .map(str::to_owned),
                date: entry
                    .published
                    .or(entry.updated)
                    .map(|date| date.timestamp().as_second()),
                content: entry
                    .content
                    .and_then(|content| content.body)
                    .or_else(|| entry.summary.as_ref().map(html)),
                id: entry.id,
            })
            .collect(),
    })
}

fn is_html(text: &feed_rs::model::Text) -> bool {
    text.content_type.to_string().contains("html")
}

/// A text of a feed as plain text, on one line.
fn text(text: &feed_rs::model::Text) -> String {
    let plain = if is_html(text) {
        html2text::config::with_decorator(html2text::render::TrivialDecorator::new())
            .string_from_read(text.content.as_bytes(), 10_000)
            .unwrap_or_else(|_| text.content.clone())
    } else {
        text.content.clone()
    };
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A text of a feed as HTML: plain text is escaped, with a paragraph for
/// each part between blank lines.
fn html(text: &feed_rs::model::Text) -> String {
    if is_html(text) {
        return text.content.clone();
    }
    let escaped = text
        .content
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    escaped
        .split("\n\n")
        .map(|part| format!("<p>{}</p>", part.trim()))
        .collect()
}

/// The link to the page itself: the one that is an alternate, or else the
/// first.
fn link(links: &[feed_rs::model::Link]) -> Option<String> {
    links
        .iter()
        .find(|link| matches!(link.rel.as_deref(), None | Some("alternate")))
        .or(links.first())
        .map(|link| link.href.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_entries_are_read() {
        let feed = parse_feed(
            br#"<?xml version="1.0"?>
<rss version="2.0"><channel><title>Planet</title><link>https://planet.example/</link>
<item><title>First &amp; best</title><link>https://planet.example/1</link>
<guid>one</guid><pubDate>Tue, 03 Nov 2026 10:00:00 +0000</pubDate>
<description>&lt;p&gt;Hello &lt;b&gt;world&lt;/b&gt;&lt;/p&gt;</description>
<author>ann@example.org (Ann)</author></item>
<item><title>Second</title><link>https://planet.example/2</link></item>
</channel></rss>"#,
            Some("https://planet.example/rss.xml"),
        )
        .unwrap();
        assert_eq!(feed.title.as_deref(), Some("Planet"));
        assert_eq!(feed.entries.len(), 2);
        let first = &feed.entries[0];
        assert_eq!(first.id, "one");
        assert_eq!(first.title, "First & best");
        assert_eq!(first.link.as_deref(), Some("https://planet.example/1"));
        assert_eq!(first.date, Some(1_793_700_000));
        assert!(first.content.as_deref().unwrap().contains("<b>world</b>"));
        // An entry without a guid gets an id that is the same each time.
        let again = parse_feed(
            br#"<rss version="2.0"><channel><title>P</title>
<item><title>Second</title><link>https://planet.example/2</link></item></channel></rss>"#,
            None,
        )
        .unwrap();
        assert_eq!(feed.entries[1].id, again.entries[0].id);
    }

    #[test]
    fn atom_entries_are_read_with_relative_links() {
        let feed = parse_feed(
            br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Blog</title>
<link href="https://blog.example/"/>
<entry><id>tag:blog.example,2026:1</id><title type="html">&lt;em&gt;Hi&lt;/em&gt; there</title>
<link rel="alternate" href="/posts/1"/><updated>2026-11-01T12:00:00Z</updated>
<author><name>Bo</name></author>
<content type="html">&lt;p&gt;Body&lt;/p&gt;</content></entry></feed>"#,
            Some("https://blog.example/atom.xml"),
        )
        .unwrap();
        let entry = &feed.entries[0];
        assert_eq!(entry.title, "Hi there");
        assert_eq!(entry.author.as_deref(), Some("Bo"));
        assert_eq!(entry.link.as_deref(), Some("https://blog.example/posts/1"));
        assert_eq!(entry.content.as_deref(), Some("<p>Body</p>"));
    }

    #[test]
    fn latin1_is_read() {
        let mut bytes = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss version=\"2.0\"><channel><title>R\xe4ksm\xf6rg\xe5s</title></channel></rss>".to_vec();
        bytes.shrink_to_fit();
        let feed = parse_feed(&bytes, None).unwrap();
        assert_eq!(feed.title.as_deref(), Some("Räksmörgås"));
    }

    #[test]
    fn what_is_not_a_feed_is_an_error() {
        assert!(parse_feed(b"<html><body>no</body></html>", None).is_err());
    }
}
