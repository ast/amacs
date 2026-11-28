//! HTML laid out as text, for reading in a buffer: wrapped to a width,
//! with the places of emphasis, code and links beside it, as shr does it in
//! Emacs. The layout is html2text's; what is here turns its tagged lines
//! into one string and the spans of faces over it, counted in chars.

use html2text::render::{TaggedLine, TextDecorator};

/// What a piece of the laid out text is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Mark {
    #[default]
    Plain,
    Link(String),
    Image(String),
    Emphasis,
    Strong,
    Strikeout,
    Code,
    Preformat,
}

#[derive(Clone)]
struct Decorator;

impl TextDecorator for Decorator {
    type Annotation = Mark;

    fn decorate_link_start(&mut self, url: &str) -> (String, Mark) {
        (String::new(), Mark::Link(url.to_owned()))
    }
    fn decorate_link_end(&mut self) -> String {
        String::new()
    }
    fn decorate_em_start(&self) -> (String, Mark) {
        (String::new(), Mark::Emphasis)
    }
    fn decorate_em_end(&self) -> String {
        String::new()
    }
    fn decorate_strong_start(&self) -> (String, Mark) {
        (String::new(), Mark::Strong)
    }
    fn decorate_strong_end(&self) -> String {
        String::new()
    }
    fn decorate_strikeout_start(&self) -> (String, Mark) {
        (String::new(), Mark::Strikeout)
    }
    fn decorate_strikeout_end(&self) -> String {
        String::new()
    }
    fn decorate_code_start(&self) -> (String, Mark) {
        (String::new(), Mark::Code)
    }
    fn decorate_code_end(&self) -> String {
        String::new()
    }
    fn decorate_preformat_first(&self) -> Mark {
        Mark::Preformat
    }
    fn decorate_preformat_cont(&self) -> Mark {
        Mark::Preformat
    }
    fn decorate_image(&mut self, src: &str, title: &str) -> (String, Mark) {
        let title = title.trim();
        let text = if title.is_empty() {
            "[image]".to_owned()
        } else {
            format!("[image: {title}]")
        };
        (text, Mark::Image(src.to_owned()))
    }
    fn header_prefix(&self, level: usize) -> String {
        "#".repeat(level) + " "
    }
    fn quote_prefix(&self) -> String {
        "> ".to_owned()
    }
    fn unordered_item_prefix(&self) -> String {
        "• ".to_owned()
    }
    fn ordered_item_prefix(&self, i: i64) -> String {
        format!("{i}. ")
    }
    fn make_subblock_decorator(&self) -> Self {
        self.clone()
    }
}

/// HTML laid out as text.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    /// The text, with a line feed after each line.
    pub text: String,
    /// Where faces go, as start and end in chars and the name of the face.
    pub spans: Vec<(usize, usize, &'static str)>,
    /// Where links are, as start and end in chars and the target, made
    /// whole against the base address if one was given.
    pub links: Vec<(usize, usize, String)>,
}

/// Lays out HTML as text that fits in a width of columns. Relative links
/// and pictures are made whole against `base`, when it is given.
pub fn render_html(html: &str, width: usize, base: Option<&str>) -> Rendered {
    let base = base.and_then(|base| url::Url::parse(base).ok());
    let lines = html2text::config::with_decorator(Decorator)
        .allow_width_overflow()
        .no_table_borders()
        .lines_from_read(html.as_bytes(), width.max(20))
        .unwrap_or_default();
    lay_out(lines, base.as_ref())
}

fn lay_out(lines: Vec<TaggedLine<Vec<Mark>>>, base: Option<&url::Url>) -> Rendered {
    let mut out = Rendered::default();
    let mut at = 0;
    let mut previous_blank = true;
    for line in lines {
        let start = at;
        let start_byte = out.text.len();
        for piece in line.tagged_strings() {
            let length = piece.s.chars().count();
            out.text.push_str(&piece.s);
            for mark in &piece.tag {
                let face = match mark {
                    Mark::Plain => continue,
                    Mark::Link(target) | Mark::Image(target) => {
                        let target = whole(target, base);
                        push_link(&mut out.links, at, at + length, target);
                        "feeds-link"
                    }
                    Mark::Emphasis => "feeds-italic",
                    Mark::Strong => "feeds-bold",
                    Mark::Strikeout => "feeds-strikeout",
                    Mark::Code | Mark::Preformat => "feeds-code",
                };
                push_span(&mut out.spans, at, at + length, face);
            }
            at += length;
        }
        let text = &out.text[start_byte..];
        let hashes = text.chars().take_while(|&c| c == '#').count();
        if previous_blank && (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
            push_span(&mut out.spans, start, at, "feeds-heading");
        }
        previous_blank = text.trim().is_empty();
        out.text.push('\n');
        at += 1;
    }
    out
}

fn whole(target: &str, base: Option<&url::Url>) -> String {
    match base.and_then(|base| base.join(target).ok()) {
        Some(url) => url.into(),
        None => target.to_owned(),
    }
}

/// Adds a span, or makes the last one longer when it is of the same face
/// and ends where this one starts.
fn push_span(
    spans: &mut Vec<(usize, usize, &'static str)>,
    start: usize,
    end: usize,
    face: &'static str,
) {
    if start == end {
        return;
    }
    if let Some(last) = spans.iter_mut().rev().find(|span| span.2 == face)
        && last.1 == start
    {
        last.1 = end;
        return;
    }
    spans.push((start, end, face));
}

fn push_link(links: &mut Vec<(usize, usize, String)>, start: usize, end: usize, target: String) {
    if start == end {
        return;
    }
    if let Some(last) = links.last_mut()
        && last.1 == start
        && last.2 == target
    {
        last.1 = end;
        return;
    }
    links.push((start, end, target));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(text: &str, start: usize, end: usize) -> String {
        text.chars().skip(start).take(end - start).collect()
    }

    #[test]
    fn faces_and_links_are_where_their_words_are() {
        let out = render_html(
            "<h2>Hello</h2><p>Some <b>bold</b> and <a href=\"/x\">a link</a> and <code>code</code>.</p>",
            60,
            Some("https://example.org/blog/post"),
        );
        assert!(out.text.starts_with("## Hello\n"), "{}", out.text);
        let face = |name| {
            out.spans
                .iter()
                .filter(|span| span.2 == name)
                .map(|span| slice(&out.text, span.0, span.1))
                .collect::<Vec<_>>()
        };
        assert_eq!(face("feeds-bold"), ["bold"]);
        assert_eq!(face("feeds-code"), ["code"]);
        assert_eq!(face("feeds-heading"), ["## Hello"]);
        assert_eq!(out.links.len(), 1);
        let (start, end, target) = &out.links[0];
        assert_eq!(slice(&out.text, *start, *end), "a link");
        assert_eq!(target, "https://example.org/x");
    }

    #[test]
    fn pictures_are_links_with_their_text() {
        let out = render_html(
            "<p><img src=\"cat.png\" alt=\"A cat\"></p>",
            60,
            Some("https://e.org/a/"),
        );
        assert!(out.text.contains("[image: A cat]"), "{}", out.text);
        assert_eq!(out.links[0].2, "https://e.org/a/cat.png");
    }

    #[test]
    fn text_is_wrapped_and_counted_in_chars() {
        let out = render_html("<p>åäö åäö åäö åäö åäö åäö åäö <i>slut</i></p>", 20, None);
        assert!(
            out.text.lines().all(|line| line.chars().count() <= 20),
            "{}",
            out.text
        );
        let span = out
            .spans
            .iter()
            .find(|span| span.2 == "feeds-italic")
            .unwrap();
        assert_eq!(slice(&out.text, span.0, span.1), "slut");
    }

    #[test]
    fn lists_have_bullets() {
        let out = render_html("<ul><li>one</li><li>two</li></ul>", 40, None);
        assert!(out.text.contains("• one\n• two"), "{}", out.text);
    }
}
