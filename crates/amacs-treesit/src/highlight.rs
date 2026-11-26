//! The kinds of syntax that are drawn differently.

/// What a piece of source text is, as far as its appearance goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Highlight {
    Comment,
    Doc,
    String,
    Escape,
    Regexp,
    Constant,
    Number,
    Keyword,
    Operator,
    Bracket,
    Delimiter,
    FunctionName,
    FunctionCall,
    Builtin,
    Preprocessor,
    Type,
    VariableName,
    Property,
    Heading,
    Emphasis,
    Strong,
    Code,
    Link,
    /// Headings below the first level, which `Heading` is.
    Heading2,
    Heading3,
    Heading4,
    Heading5,
    Heading6,
    Heading7,
    Heading8,
}

impl Highlight {
    pub const ALL: [Highlight; 30] = [
        Highlight::Comment,
        Highlight::Doc,
        Highlight::String,
        Highlight::Escape,
        Highlight::Regexp,
        Highlight::Constant,
        Highlight::Number,
        Highlight::Keyword,
        Highlight::Operator,
        Highlight::Bracket,
        Highlight::Delimiter,
        Highlight::FunctionName,
        Highlight::FunctionCall,
        Highlight::Builtin,
        Highlight::Preprocessor,
        Highlight::Type,
        Highlight::VariableName,
        Highlight::Property,
        Highlight::Heading,
        Highlight::Emphasis,
        Highlight::Strong,
        Highlight::Code,
        Highlight::Link,
        Highlight::Heading2,
        Highlight::Heading3,
        Highlight::Heading4,
        Highlight::Heading5,
        Highlight::Heading6,
        Highlight::Heading7,
        Highlight::Heading8,
    ];

    /// The name of the face that draws this kind of text.
    pub fn name(self) -> &'static str {
        match self {
            Highlight::Comment => "comment",
            Highlight::Doc => "doc",
            Highlight::String => "string",
            Highlight::Escape => "escape",
            Highlight::Regexp => "regexp",
            Highlight::Constant => "constant",
            Highlight::Number => "number",
            Highlight::Keyword => "keyword",
            Highlight::Operator => "operator",
            Highlight::Bracket => "bracket",
            Highlight::Delimiter => "delimiter",
            Highlight::FunctionName => "function-name",
            Highlight::FunctionCall => "function-call",
            Highlight::Builtin => "builtin",
            Highlight::Preprocessor => "preprocessor",
            Highlight::Type => "type",
            Highlight::VariableName => "variable-name",
            Highlight::Property => "property",
            Highlight::Heading => "heading",
            Highlight::Emphasis => "emphasis",
            Highlight::Strong => "strong",
            Highlight::Code => "code",
            Highlight::Link => "link",
            Highlight::Heading2 => "heading-2",
            Highlight::Heading3 => "heading-3",
            Highlight::Heading4 => "heading-4",
            Highlight::Heading5 => "heading-5",
            Highlight::Heading6 => "heading-6",
            Highlight::Heading7 => "heading-7",
            Highlight::Heading8 => "heading-8",
        }
    }

    /// The kind that the heading of a level is drawn as, from 1. Levels
    /// beyond the eighth start again from the first.
    pub fn heading(level: usize) -> Self {
        const LEVELS: [Highlight; 8] = [
            Highlight::Heading,
            Highlight::Heading2,
            Highlight::Heading3,
            Highlight::Heading4,
            Highlight::Heading5,
            Highlight::Heading6,
            Highlight::Heading7,
            Highlight::Heading8,
        ];
        LEVELS[level.saturating_sub(1) % LEVELS.len()]
    }

    pub(crate) fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(usize::from(index)).copied()
    }
}

/// What a capture of a highlighting query does to the text it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    /// Nothing: the capture is there for the query's own use.
    Ignore,
    /// The text is drawn plainly, whatever an outer or losing pattern says.
    Plain,
    Is(Highlight),
}

impl Capture {
    /// The meaning of a capture name such as `function.builtin`. A name with
    /// no entry of its own gets that of the name with its last part removed,
    /// so `keyword.control` is a keyword.
    pub(crate) fn for_name(name: &str) -> Self {
        const TABLE: &[(&str, Option<Highlight>)] = &[
            ("attribute", Some(Highlight::Preprocessor)),
            ("boolean", Some(Highlight::Constant)),
            ("comment", Some(Highlight::Comment)),
            ("comment.documentation", Some(Highlight::Doc)),
            ("conditional", Some(Highlight::Keyword)),
            ("constant", Some(Highlight::Constant)),
            ("constant.character.escape", Some(Highlight::Escape)),
            ("constructor", Some(Highlight::Type)),
            ("escape", Some(Highlight::Escape)),
            ("exception", Some(Highlight::Keyword)),
            ("float", Some(Highlight::Number)),
            ("function", Some(Highlight::FunctionName)),
            ("function.builtin", Some(Highlight::Builtin)),
            ("function.call", Some(Highlight::FunctionCall)),
            ("function.macro", Some(Highlight::Preprocessor)),
            ("function.method.call", Some(Highlight::FunctionCall)),
            ("function.special", Some(Highlight::Preprocessor)),
            ("include", Some(Highlight::Keyword)),
            ("keyword", Some(Highlight::Keyword)),
            ("label", Some(Highlight::Constant)),
            ("markup.heading", Some(Highlight::Heading)),
            ("markup.heading.2", Some(Highlight::Heading2)),
            ("markup.heading.3", Some(Highlight::Heading3)),
            ("markup.heading.4", Some(Highlight::Heading4)),
            ("markup.heading.5", Some(Highlight::Heading5)),
            ("markup.heading.6", Some(Highlight::Heading6)),
            ("markup.italic", Some(Highlight::Emphasis)),
            ("markup.link", Some(Highlight::Link)),
            ("markup.raw", Some(Highlight::Code)),
            ("markup.strong", Some(Highlight::Strong)),
            ("module", Some(Highlight::Constant)),
            ("namespace", Some(Highlight::Constant)),
            ("number", Some(Highlight::Number)),
            ("operator", Some(Highlight::Operator)),
            ("preproc", Some(Highlight::Preprocessor)),
            ("property", Some(Highlight::Property)),
            ("punctuation", Some(Highlight::Delimiter)),
            ("punctuation.bracket", Some(Highlight::Bracket)),
            ("repeat", Some(Highlight::Keyword)),
            ("string", Some(Highlight::String)),
            ("string.escape", Some(Highlight::Escape)),
            ("string.regex", Some(Highlight::Regexp)),
            ("string.regexp", Some(Highlight::Regexp)),
            ("string.special", Some(Highlight::Regexp)),
            ("tag", Some(Highlight::FunctionName)),
            ("tag.attribute", Some(Highlight::Property)),
            ("text.emphasis", Some(Highlight::Emphasis)),
            ("text.literal", Some(Highlight::Code)),
            ("text.reference", Some(Highlight::Link)),
            ("text.strong", Some(Highlight::Strong)),
            ("text.title", Some(Highlight::Heading)),
            ("text.uri", Some(Highlight::Link)),
            ("type", Some(Highlight::Type)),
            // Ordinary variables are the bulk of the text and stay plain.
            ("variable", None),
            ("variable.builtin", Some(Highlight::Builtin)),
            ("variable.parameter", Some(Highlight::VariableName)),
        ];
        let mut name = name;
        loop {
            if let Some((_, highlight)) = TABLE.iter().find(|(entry, _)| *entry == name) {
                return highlight.map_or(Capture::Plain, Capture::Is);
            }
            match name.rfind('.') {
                Some(dot) => name = &name[..dot],
                None => return Capture::Ignore,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_names_fall_back_to_their_prefix() {
        assert_eq!(
            Capture::for_name("keyword.control.flow"),
            Capture::Is(Highlight::Keyword)
        );
        assert_eq!(
            Capture::for_name("function.builtin"),
            Capture::Is(Highlight::Builtin)
        );
        assert_eq!(Capture::for_name("variable"), Capture::Plain);
        assert_eq!(Capture::for_name("variable.member"), Capture::Plain);
        assert_eq!(Capture::for_name("_define"), Capture::Ignore);
    }

    #[test]
    fn every_highlight_has_its_index() {
        for (index, highlight) in Highlight::ALL.iter().enumerate() {
            assert_eq!(*highlight as usize, index);
            assert_eq!(Highlight::from_index(index as u8), Some(*highlight));
        }
    }
}
