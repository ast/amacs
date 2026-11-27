//! Syntax trees for buffers: which languages there are, how their text is
//! highlighted and how their lines are indented.
//!
//! The grammars are compiled in. A buffer owns a [`Syntax`], tells it about
//! every edit, and asks it for highlights and indentation after an
//! [`Syntax::update`].

mod highlight;
mod indent;
mod languages;
mod org;
mod syntax;

pub use highlight::Highlight;
pub use indent::{IndentParams, indent_from_previous_line, line_indentation};
pub use languages::{
    BracketRules, IndentStyle, Injections, Language, language, language_called, languages,
    prepare_languages,
};
pub use syntax::{Highlights, MAX_PARSE_BYTES, Syntax};
