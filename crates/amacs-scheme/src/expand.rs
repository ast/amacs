//! `syntax-rules` macros: matching a use against patterns and filling in a
//! template.
//!
//! Hygiene comes from marks. Every identifier that a template introduces
//! gets the mark of that one expansion, so a variable the macro binds for
//! its own use is a different variable from any of the same name in the
//! code it was given. An introduced name that nothing in the expansion
//! binds is looked up where the macro was defined.

use std::collections::HashMap;
use std::rc::Rc;

use crate::compile::Scope;
use crate::error::Error;
use crate::source::Span;
use crate::syntax::{Datum, Ident, Mark, Syntax};
use crate::value::Symbol;

/// A macro defined with `syntax-rules`.
#[derive(Debug)]
pub struct Macro {
    pub name: Symbol,
    literals: Vec<Ident>,
    /// The identifier that means "and so on": `...` unless another was named.
    ellipsis: Ident,
    rules: Vec<(Syntax, Syntax)>,
    /// Where the macro was defined; `None` at the top level.
    scope: Option<Rc<Scope>>,
}

/// What a pattern variable matched.
#[derive(Debug, Clone)]
enum Match {
    One(Syntax),
    /// A variable followed by an ellipsis: what it matched each time.
    Many(Vec<Match>),
}

/// A pattern variable: its name and the mark it has, if any.
type Key = (Symbol, Option<u32>);

type Bindings = HashMap<Key, Match>;

fn key(ident: &Ident) -> Key {
    (ident.name, ident.mark.as_ref().map(|mark| mark.id))
}

/// The names the expander has to recognise, which the compiler interns once.
#[derive(Debug, Clone, Copy)]
pub struct Names {
    pub ellipsis: Symbol,
    pub underscore: Symbol,
    pub syntax_rules: Symbol,
}

impl Macro {
    /// Reads `(syntax-rules (literal ...) (pattern template) ...)`.
    pub fn parse(
        name: Symbol,
        spec: &Syntax,
        scope: Option<Rc<Scope>>,
        names: &Names,
    ) -> Result<Self, Error> {
        let bad = |what: &str, at: &Syntax| Error::new(what).at(at.span);
        let parts = spec
            .list()
            .filter(|parts| {
                parts
                    .first()
                    .and_then(Syntax::ident)
                    .is_some_and(|head| head.name == names.syntax_rules)
            })
            .ok_or_else(|| {
                bad(
                    "a macro is defined with (syntax-rules (literal ...) rule ...)",
                    spec,
                )
            })?;
        // An identifier before the literals replaces `...`.
        let (ellipsis, rest) = match parts.get(1).and_then(Syntax::ident) {
            Some(custom) => (custom.clone(), &parts[2..]),
            None => (Ident::plain(names.ellipsis), &parts[1..]),
        };
        let literals = rest
            .first()
            .and_then(Syntax::list)
            .ok_or_else(|| {
                bad(
                    "syntax-rules needs a list of literals, which may be empty",
                    spec,
                )
            })?
            .iter()
            .map(|literal| {
                literal
                    .ident()
                    .cloned()
                    .ok_or_else(|| bad("a literal must be an identifier", literal))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let rules = rest[1..]
            .iter()
            .map(|rule| match rule.list() {
                Some([pattern, template]) => Ok((pattern.clone(), template.clone())),
                _ => Err(bad("a rule is a pattern and a template", rule)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name,
            literals,
            ellipsis,
            rules,
            scope,
        })
    }

    /// Whether this is the "and so on" mark. A literal of the same name
    /// takes precedence, and leaves the macro without one.
    fn is_ellipsis(&self, syntax: &Syntax) -> bool {
        syntax
            .ident()
            .is_some_and(|ident| ident.same(&self.ellipsis) && !self.is_literal(ident))
    }

    fn is_literal(&self, ident: &Ident) -> bool {
        self.literals.iter().any(|literal| literal.same(ident))
    }

    /// Expands a use of the macro. `mark_id` is a number no other expansion
    /// has; `is_bound` says whether an identifier of the use is a local
    /// variable there, in which case it is not the literal of that name.
    pub fn expand(
        self: &Rc<Self>,
        usage: &Syntax,
        mark_id: u32,
        names: &Names,
        is_bound: &dyn Fn(&Ident) -> bool,
    ) -> Result<Syntax, Error> {
        for (pattern, template) in &self.rules {
            let mut bindings = Bindings::new();
            // The first element of the pattern stands for the macro's name.
            let matched = match (&pattern.datum, &usage.datum) {
                (Datum::List(pattern), Datum::List(input)) if !pattern.is_empty() => self
                    .match_list(
                        &pattern[1..],
                        None,
                        &input[1..],
                        None,
                        names,
                        is_bound,
                        &mut bindings,
                    ),
                (Datum::Dotted(pattern, tail), Datum::List(input)) if !pattern.is_empty() => self
                    .match_list(
                        &pattern[1..],
                        Some(tail),
                        &input[1..],
                        None,
                        names,
                        is_bound,
                        &mut bindings,
                    ),
                _ => false,
            };
            if matched {
                let mark = Rc::new(MarkMaker {
                    id: mark_id,
                    scope: self.scope.clone(),
                });
                let matches = Matches {
                    here: Vec::new(),
                    outside: None,
                    all: &bindings,
                };
                return self.fill(template, &matches, &mark, usage.span);
            }
        }
        Err(Error::new("no rule of the macro matches this use").at(usage.span))
    }

    #[allow(clippy::too_many_arguments)]
    fn match_list(
        &self,
        pattern: &[Syntax],
        pattern_tail: Option<&Syntax>,
        input: &[Syntax],
        input_tail: Option<&Syntax>,
        names: &Names,
        is_bound: &dyn Fn(&Ident) -> bool,
        bindings: &mut Bindings,
    ) -> bool {
        let ellipsis = pattern.iter().position(|item| self.is_ellipsis(item));
        match ellipsis {
            None => {
                if input.len() < pattern.len() {
                    return false;
                }
                let (head, rest) = input.split_at(pattern.len());
                if !pattern
                    .iter()
                    .zip(head)
                    .all(|(p, i)| self.match_one(p, i, names, is_bound, bindings))
                {
                    return false;
                }
                // What is left over goes to the tail pattern, if any.
                match pattern_tail {
                    Some(tail) => {
                        let rest_syntax = match (rest, input_tail) {
                            ([], Some(last)) => last.clone(),
                            (rest, Some(last)) => Syntax::new(
                                Datum::Dotted(rest.to_vec(), Box::new(last.clone())),
                                last.span,
                            ),
                            (rest, None) => Syntax::new(Datum::List(rest.to_vec()), tail.span),
                        };
                        self.match_one(tail, &rest_syntax, names, is_bound, bindings)
                    }
                    None => rest.is_empty() && input_tail.is_none(),
                }
            }
            Some(0) => false,
            Some(at) => {
                let (before, repeated, after) =
                    (&pattern[..at - 1], &pattern[at - 1], &pattern[at + 1..]);
                if input.len() < before.len() + after.len() {
                    return false;
                }
                let middle_end = input.len() - after.len();
                if !before
                    .iter()
                    .zip(input)
                    .all(|(p, i)| self.match_one(p, i, names, is_bound, bindings))
                {
                    return false;
                }
                let mut each = Vec::new();
                for item in &input[before.len()..middle_end] {
                    let mut inner = Bindings::new();
                    if !self.match_one(repeated, item, names, is_bound, &mut inner) {
                        return false;
                    }
                    each.push(inner);
                }
                // Every variable under the ellipsis gets a sequence, which
                // is empty if nothing was there to match.
                let mut variables = Vec::new();
                self.pattern_variables(repeated, names, &mut variables);
                for variable in variables {
                    let matches = each
                        .iter()
                        .filter_map(|inner| inner.get(&variable).cloned())
                        .collect();
                    bindings.insert(variable, Match::Many(matches));
                }
                self.match_list(
                    after,
                    pattern_tail,
                    &input[middle_end..],
                    input_tail,
                    names,
                    is_bound,
                    bindings,
                )
            }
        }
    }

    fn match_one(
        &self,
        pattern: &Syntax,
        input: &Syntax,
        names: &Names,
        is_bound: &dyn Fn(&Ident) -> bool,
        bindings: &mut Bindings,
    ) -> bool {
        match (&pattern.datum, &input.datum) {
            (Datum::Ident(ident), _) if self.is_literal(ident) => input
                .ident()
                .is_some_and(|given| given.name == ident.name && !is_bound(given)),
            (Datum::Ident(ident), _) if ident.name == names.underscore && ident.mark.is_none() => {
                true
            }
            (Datum::Ident(ident), _) => {
                bindings.insert(key(ident), Match::One(input.clone()));
                true
            }
            (Datum::List(pattern), Datum::List(input)) => {
                self.match_list(pattern, None, input, None, names, is_bound, bindings)
            }
            (Datum::List(pattern), Datum::Dotted(input, tail)) => {
                self.match_list(pattern, None, input, Some(tail), names, is_bound, bindings)
            }
            (Datum::Dotted(pattern, tail), Datum::List(input)) => {
                self.match_list(pattern, Some(tail), input, None, names, is_bound, bindings)
            }
            (Datum::Dotted(pattern, tail), Datum::Dotted(input, input_tail)) => self.match_list(
                pattern,
                Some(tail),
                input,
                Some(input_tail),
                names,
                is_bound,
                bindings,
            ),
            (Datum::Vector(pattern), Datum::Vector(input)) => {
                self.match_list(pattern, None, input, None, names, is_bound, bindings)
            }
            // `(a ... . rest)` with no `a`s takes anything as its rest.
            (Datum::Dotted(pattern, tail), _) => self.match_list(
                pattern,
                Some(tail),
                &[],
                Some(input),
                names,
                is_bound,
                bindings,
            ),
            (Datum::Bool(a), Datum::Bool(b)) => a == b,
            (Datum::Int(a), Datum::Int(b)) => a == b,
            (Datum::Float(a), Datum::Float(b)) => a == b,
            (Datum::Char(a), Datum::Char(b)) => a == b,
            (Datum::Str(a), Datum::Str(b)) => a == b,
            (Datum::Keyword(a), Datum::Keyword(b)) => a == b,
            _ => false,
        }
    }

    /// The variables a pattern binds.
    fn pattern_variables(
        &self,
        pattern: &Syntax,
        names: &Names,
        out: &mut Vec<(Symbol, Option<u32>)>,
    ) {
        match &pattern.datum {
            Datum::Ident(ident) => {
                let special = self.is_literal(ident)
                    || ident.same(&self.ellipsis)
                    || (ident.name == names.underscore && ident.mark.is_none());
                if !special {
                    out.push(key(ident));
                }
            }
            Datum::List(items) | Datum::Vector(items) => {
                for item in items {
                    self.pattern_variables(item, names, out);
                }
            }
            Datum::Dotted(items, tail) => {
                for item in items {
                    self.pattern_variables(item, names, out);
                }
                self.pattern_variables(tail, names, out);
            }
            _ => {}
        }
    }

    /// The pattern variables a template uses, with what they matched.
    fn template_variables<'a>(
        &self,
        template: &Syntax,
        matches: &Matches<'a>,
        out: &mut Vec<(Key, &'a Match)>,
    ) {
        match &template.datum {
            Datum::Ident(ident) => {
                if let Some(matched) = matches.get(key(ident)) {
                    out.push((key(ident), matched));
                }
            }
            Datum::List(items) | Datum::Vector(items) => {
                for item in items {
                    self.template_variables(item, matches, out);
                }
            }
            Datum::Dotted(items, tail) => {
                for item in items {
                    self.template_variables(item, matches, out);
                }
                self.template_variables(tail, matches, out);
            }
            _ => {}
        }
    }

    fn fill(
        &self,
        template: &Syntax,
        matches: &Matches,
        mark: &Rc<MarkMaker>,
        usage: Span,
    ) -> Result<Syntax, Error> {
        match &template.datum {
            Datum::Ident(ident) => match matches.get(key(ident)) {
                Some(Match::One(syntax)) => Ok(syntax.clone()),
                Some(Match::Many(_)) => Err(Error::new(
                    "a pattern variable that had an ellipsis after it needs one in the template too",
                )
                .at(template.span)),
                // An identifier of the macro's own: marked, and placed at
                // the use so that errors point at the user's code.
                None => Ok(Syntax::new(Datum::Ident(mark.apply(ident)), usage)),
            },
            // `(... template)` is the template with ellipses taken literally.
            Datum::List(items) if items.len() == 2 && self.is_ellipsis(&items[0]) => {
                self.fill_verbatim(&items[1], matches, mark, usage)
            }
            Datum::List(items) => Ok(Syntax::new(
                Datum::List(self.fill_sequence(items, matches, mark, usage)?),
                usage,
            )),
            Datum::Vector(items) => Ok(Syntax::new(
                Datum::Vector(self.fill_sequence(items, matches, mark, usage)?),
                usage,
            )),
            Datum::Dotted(items, tail) => {
                let mut filled = self.fill_sequence(items, matches, mark, usage)?;
                let tail = self.fill(tail, matches, mark, usage)?;
                // A tail that is a list continues the list.
                Ok(match tail.datum {
                    Datum::List(rest) => {
                        filled.extend(rest);
                        Syntax::new(Datum::List(filled), usage)
                    }
                    Datum::Dotted(rest, last) => {
                        filled.extend(rest);
                        Syntax::new(Datum::Dotted(filled, last), usage)
                    }
                    _ => Syntax::new(Datum::Dotted(filled, Box::new(tail)), usage),
                })
            }
            _ => Ok(Syntax::new(template.datum.clone(), usage)),
        }
    }

    /// Fills a template in which the ellipsis is an ordinary identifier.
    fn fill_verbatim(
        &self,
        template: &Syntax,
        matches: &Matches,
        mark: &Rc<MarkMaker>,
        usage: Span,
    ) -> Result<Syntax, Error> {
        match &template.datum {
            Datum::List(items) => Ok(Syntax::new(
                Datum::List(
                    items
                        .iter()
                        .map(|item| self.fill_verbatim(item, matches, mark, usage))
                        .collect::<Result<_, _>>()?,
                ),
                usage,
            )),
            Datum::Ident(ident) if ident.same(&self.ellipsis) => Ok(template.clone()),
            _ => self.fill(template, matches, mark, usage),
        }
    }

    fn fill_sequence(
        &self,
        items: &[Syntax],
        matches: &Matches,
        mark: &Rc<MarkMaker>,
        usage: Span,
    ) -> Result<Vec<Syntax>, Error> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < items.len() {
            let item = &items[index];
            // How many ellipses follow: each flattens one level.
            let mut depth = 0;
            while items
                .get(index + 1 + depth)
                .is_some_and(|next| self.is_ellipsis(next))
            {
                depth += 1;
            }
            if depth == 0 {
                out.push(self.fill(item, matches, mark, usage)?);
            } else {
                self.fill_repeated(item, depth, matches, mark, usage, &mut out)?;
            }
            index += 1 + depth;
        }
        Ok(out)
    }

    fn fill_repeated(
        &self,
        item: &Syntax,
        depth: usize,
        matches: &Matches,
        mark: &Rc<MarkMaker>,
        usage: Span,
        out: &mut Vec<Syntax>,
    ) -> Result<(), Error> {
        let mut used = Vec::new();
        self.template_variables(item, matches, &mut used);
        let sequences: Vec<(Key, &Vec<Match>)> = used
            .iter()
            .filter_map(|(name, matched)| match matched {
                Match::Many(items) => Some((*name, items)),
                Match::One(_) => None,
            })
            .collect();
        let Some(count) = sequences.iter().map(|(_, items)| items.len()).max() else {
            return Err(Error::new(
                "nothing before this ellipsis in the template had one in the pattern",
            )
            .at(item.span));
        };
        if sequences.iter().any(|(_, items)| items.len() != count) {
            return Err(Error::new(
                "the pattern variables under this ellipsis matched different numbers of things",
            )
            .at(usage));
        }
        for round in 0..count {
            // In this round each sequence stands for one of its items.
            let inner = Matches {
                here: sequences
                    .iter()
                    .map(|(name, items)| (*name, &items[round]))
                    .collect(),
                outside: Some(matches),
                all: matches.all,
            };
            if depth > 1 {
                self.fill_repeated(item, depth - 1, &inner, mark, usage, out)?;
            } else {
                out.push(self.fill(item, &inner, mark, usage)?);
            }
        }
        Ok(())
    }
}

/// What the pattern variables stand for at a place in a template: under
/// an ellipsis, one item of each sequence at a time.
struct Matches<'a> {
    here: Vec<(Key, &'a Match)>,
    outside: Option<&'a Matches<'a>>,
    all: &'a Bindings,
}

impl<'a> Matches<'a> {
    fn get(&self, wanted: Key) -> Option<&'a Match> {
        let mut level = Some(self);
        while let Some(matches) = level {
            if let Some((_, found)) = matches.here.iter().find(|(name, _)| *name == wanted) {
                return Some(found);
            }
            level = matches.outside;
        }
        self.all.get(&wanted)
    }
}

/// Marks the identifiers that one expansion introduces.
#[derive(Debug)]
struct MarkMaker {
    id: u32,
    scope: Option<Rc<Scope>>,
}

impl MarkMaker {
    fn apply(&self, ident: &Ident) -> Ident {
        Ident {
            name: ident.name,
            mark: Some(Rc::new(Mark {
                id: self.id,
                inner: ident.mark.clone(),
                scope: self.scope.clone(),
            })),
        }
    }
}
