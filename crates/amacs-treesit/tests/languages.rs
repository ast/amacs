//! Every language's grammar loads and its highlighting query compiles.

use amacs_text::Text;
use amacs_treesit::{Syntax, languages};

#[test]
fn every_query_fits_its_grammar() {
    for language in languages() {
        if let Err(error) = language.check_query() {
            panic!("{}: {error}", language.name);
        }
        if language.has_grammar() {
            let mut syntax = Syntax::new(language).expect("the grammar loads");
            let text = Text::from("x = 1\n");
            syntax.update(&text);
            assert!(syntax.sexp().is_some(), "{} parses", language.name);
        }
    }
}
