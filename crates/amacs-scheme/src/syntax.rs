//! Source code as the reader delivers it: data with positions.
//!
//! Identifiers carry a mark when a macro put them where they are, which is
//! what keeps a macro's own names apart from the names of the code around
//! its use.

use std::rc::Rc;

use crate::compile::Scope;
use crate::source::Span;
use crate::value::Symbol;

/// What tells an identifier that a macro introduced from one written in the
/// source. All identifiers introduced by one use of a macro share a mark.
#[derive(Debug)]
pub struct Mark {
    pub id: u32,
    /// The mark the identifier had in the macro's own definition, if that
    /// definition was itself produced by a macro.
    pub inner: Option<Rc<Mark>>,
    /// Where the macro was defined: where names it introduces are looked up
    /// if nothing in the expansion binds them. `None` is the top level.
    pub scope: Option<Rc<Scope>>,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: Symbol,
    pub mark: Option<Rc<Mark>>,
}

impl Ident {
    pub fn plain(name: Symbol) -> Self {
        Self { name, mark: None }
    }

    /// Whether two identifiers are the same name from the same place.
    pub fn same(&self, other: &Ident) -> bool {
        self.name == other.name
            && match (&self.mark, &other.mark) {
                (None, None) => true,
                (Some(a), Some(b)) => a.id == b.id,
                _ => false,
            }
    }
}

#[derive(Debug, Clone)]
pub enum Datum {
    Bool(bool),
    Int(i64),
    Float(f64),
    Char(char),
    Str(Rc<str>),
    Ident(Ident),
    /// A keyword such as `#:key`, which stands for itself.
    Keyword(Symbol),
    /// A proper list.
    List(Vec<Syntax>),
    /// A list that ends in something other than the empty list.
    Dotted(Vec<Syntax>, Box<Syntax>),
    Vector(Vec<Syntax>),
}

/// A datum and where it was read.
#[derive(Debug, Clone)]
pub struct Syntax {
    pub datum: Datum,
    pub span: Span,
}

impl Syntax {
    pub fn new(datum: Datum, span: Span) -> Self {
        Self { datum, span }
    }

    pub fn ident(&self) -> Option<&Ident> {
        match &self.datum {
            Datum::Ident(ident) => Some(ident),
            _ => None,
        }
    }

    pub fn list(&self) -> Option<&[Syntax]> {
        match &self.datum {
            Datum::List(items) => Some(items),
            _ => None,
        }
    }

    /// A list of the given items, at the same position as this.
    pub fn list_like(&self, items: Vec<Syntax>) -> Syntax {
        Syntax::new(Datum::List(items), self.span)
    }

    /// An unmarked identifier at the same position as this.
    pub fn symbol_like(&self, name: Symbol) -> Syntax {
        Syntax::new(Datum::Ident(Ident::plain(name)), self.span)
    }
}
