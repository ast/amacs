//! From syntax to a tree the code generator can walk: macros expanded,
//! every name resolved to the variable it means, special forms reduced to
//! a few kinds of expression.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::error::Error;
use crate::expand::{Macro, Names};
use crate::source::Span;
use crate::syntax::{Datum, Ident, Mark, Syntax};
use crate::value::{Object, Symbol, Value};
use crate::vm::Core;

/// A local variable.
#[derive(Debug)]
pub struct Var {
    pub name: Symbol,
    /// The function whose frame holds the variable. It changes when a
    /// loop becomes part of the function around it.
    pub func: Cell<u32>,
    pub slot: Cell<u16>,
    /// Whether an inner function refers to it.
    pub captured: Cell<bool>,
    /// Whether `set!` assigns to it.
    pub mutated: Cell<bool>,
    /// Whether it is in scope before it has a value, as the variables of
    /// `letrec` and of definitions inside a body are.
    pub recursive: bool,
}

impl Var {
    /// Whether the variable lives in a cell of its own rather than in its
    /// slot: when closures share it and it changes after they were made.
    pub fn in_cell(&self) -> bool {
        self.captured.get() && (self.mutated.get() || self.recursive)
    }
}

#[derive(Debug, Clone)]
pub enum Binding {
    Var(Rc<Var>),
    Macro(Rc<Macro>),
}

/// The names visible at a place in the source, innermost first.
#[derive(Debug)]
pub struct Scope {
    parent: Option<Rc<Scope>>,
    bindings: RefCell<Vec<(Ident, Binding)>>,
}

type Env = Option<Rc<Scope>>;

impl Scope {
    fn child(parent: &Env) -> Rc<Scope> {
        Rc::new(Scope {
            parent: parent.clone(),
            bindings: RefCell::new(Vec::new()),
        })
    }

    fn bind(&self, ident: Ident, binding: Binding) {
        self.bindings.borrow_mut().push((ident, binding));
    }

    fn find(&self, ident: &Ident) -> Option<Binding> {
        // The latest binding of a name in a scope is the one that counts.
        self.bindings
            .borrow()
            .iter()
            .rev()
            .find(|(bound, _)| bound.same(ident))
            .map(|(_, binding)| binding.clone())
    }
}

/// What a name means where it is used.
enum Resolved {
    Var(Rc<Var>),
    Macro(Rc<Macro>),
    /// Not bound locally: a name of the top level.
    Global(Symbol),
}

fn resolve(ident: &Ident, env: &Env) -> Resolved {
    let mut scope = env.clone();
    while let Some(current) = scope {
        match current.find(ident) {
            Some(Binding::Var(var)) => return Resolved::Var(var),
            Some(Binding::Macro(found)) => return Resolved::Macro(found),
            None => scope = current.parent.clone(),
        }
    }
    // A name a macro introduced means what it meant where the macro was
    // defined.
    match &ident.mark {
        Some(mark) => resolve(
            &Ident {
                name: ident.name,
                mark: mark.inner.clone(),
            },
            &mark.scope,
        ),
        None => Resolved::Global(ident.name),
    }
}

/// The forms the compiler knows by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Quote,
    Quasiquote,
    Lambda,
    Define,
    Set,
    If,
    Begin,
    Let,
    LetStar,
    Letrec,
    Cond,
    Case,
    And,
    Or,
    When,
    Unless,
    Do,
    DefineSyntax,
    LetSyntax,
    DefineRecordType,
    Parameterize,
    Guard,
    LetValues,
    LetStarValues,
    DefineValues,
    Import,
}

pub const SPECIAL_FORMS: &[(&str, Special)] = &[
    ("quote", Special::Quote),
    ("quasiquote", Special::Quasiquote),
    ("lambda", Special::Lambda),
    ("λ", Special::Lambda),
    ("define", Special::Define),
    ("set!", Special::Set),
    ("if", Special::If),
    ("begin", Special::Begin),
    ("let", Special::Let),
    ("let*", Special::LetStar),
    ("letrec", Special::Letrec),
    ("letrec*", Special::Letrec),
    ("cond", Special::Cond),
    ("case", Special::Case),
    ("and", Special::And),
    ("or", Special::Or),
    ("when", Special::When),
    ("unless", Special::Unless),
    ("do", Special::Do),
    ("define-syntax", Special::DefineSyntax),
    ("let-syntax", Special::LetSyntax),
    ("letrec-syntax", Special::LetSyntax),
    ("define-record-type", Special::DefineRecordType),
    ("parameterize", Special::Parameterize),
    ("guard", Special::Guard),
    ("let-values", Special::LetValues),
    ("let*-values", Special::LetStarValues),
    ("define-values", Special::DefineValues),
    ("import", Special::Import),
];

/// Symbols the compiler compares against, interned once.
#[derive(Debug, Clone, Copy)]
pub struct Keywords {
    pub names: Names,
    pub else_: Symbol,
    pub arrow: Symbol,
    pub unquote: Symbol,
    pub unquote_splicing: Symbol,
    pub quasiquote: Symbol,
}

/// A `lambda` after analysis.
#[derive(Debug)]
pub struct Lambda {
    pub func: u32,
    /// The parameters, the one for further arguments last if there is one.
    pub params: Vec<Rc<Var>>,
    pub required: usize,
    pub rest: bool,
    /// The variables of enclosing functions that the body refers to.
    pub captures: Vec<Rc<Var>>,
    pub body: Expr,
    pub name: Option<Symbol>,
    pub doc: Option<Rc<str>>,
    pub span: Span,
    pub locals: u16,
    /// Whether this is a form of the top level rather than a `lambda`.
    pub toplevel: bool,
    /// The local variable that the function is the first value of, as in
    /// a named `let` or a definition inside a body. For as long as nothing
    /// assigns to the variable, a call of it from the function's own body
    /// is a call of the function itself.
    pub self_var: Option<Rc<Var>>,
    /// Every local variable of the function, the parameters first.
    pub vars: Vec<Rc<Var>>,
}

#[derive(Debug)]
pub enum Expr {
    Const(Value),
    Local(Rc<Var>),
    /// A variable that may not have its value yet, and where it is used,
    /// to say so if it does not.
    LocalAt(Rc<Var>, Span),
    SetLocal(Rc<Var>, Box<Expr>),
    /// Gives a recursive variable its first value.
    InitLocal(Rc<Var>, Box<Expr>),
    Global(u32, Span),
    SetGlobal(u32, Box<Expr>, Span),
    DefineGlobal(u32, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    Begin(Vec<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Lambda(Box<Lambda>),
    Call(Box<Expr>, Vec<Expr>, Span),
    /// Binds variables one after the other, then evaluates the body.
    Let(Vec<(Rc<Var>, Expr)>, Box<Expr>),
    /// Brings recursive variables into being, without values yet.
    Letrec(Vec<Rc<Var>>, Box<Expr>),
    /// A named `let` or a `do` that needs no function of its own: its
    /// variables are variables of the function around it, and a call of
    /// its name, which is `var`, gives them new values and goes back to
    /// the start of the body.
    Loop {
        var: Rc<Var>,
        params: Vec<Rc<Var>>,
        inits: Vec<Expr>,
        body: Box<Expr>,
    },
}

struct FuncCtx {
    id: u32,
    captures: Vec<Rc<Var>>,
    next_slot: u16,
    vars: Vec<Rc<Var>>,
}

pub struct Compiler<'a> {
    pub core: &'a mut Core,
    funcs: Vec<FuncCtx>,
    keywords: Keywords,
    /// How deep macro expansion has gone, to stop one that never ends.
    expansions: u32,
    /// How deep in nested expressions the compiler is.
    depth: u32,
}

/// What a form in a body turned out to be.
enum BodyItem {
    Define(Rc<Var>, Syntax, Option<Symbol>),
    /// Several variables from one expression that returns several values.
    DefineValues(Vec<Rc<Var>>, Option<Rc<Var>>, Syntax),
    Expr(Syntax),
}

/// Gives a recursive variable its first value. A function learns here
/// which variable holds it, so that it can tell a call of itself.
fn init_local(var: Rc<Var>, mut value: Expr) -> Expr {
    if let Expr::Lambda(lambda) = &mut value {
        lambda.self_var = Some(var.clone());
    }
    Expr::InitLocal(var, Box::new(value))
}

/// Whether every use of `var` in an expression is a call of it with
/// `count` arguments that is the last thing the expression does, given
/// that the expression is (`last`) or is not the last thing itself. Then
/// nothing is left to do after such a call, and nothing but the call can
/// get at the variable.
fn ends_with_every_call(expr: &Expr, var: &Rc<Var>, count: usize, last: bool) -> bool {
    crate::deep(|| {
        let inner = |expr: &Expr| ends_with_every_call(expr, var, count, false);
        let outer = |expr: &Expr| ends_with_every_call(expr, var, count, last);
        let sequence = |exprs: &[Expr]| match exprs.split_last() {
            Some((end, before)) => before.iter().all(inner) && outer(end),
            None => true,
        };
        match expr {
            Expr::Const(_) | Expr::Global(..) => true,
            Expr::Local(used) | Expr::LocalAt(used, _) => !Rc::ptr_eq(used, var),
            Expr::SetLocal(target, value) | Expr::InitLocal(target, value) => {
                !Rc::ptr_eq(target, var) && inner(value)
            }
            Expr::SetGlobal(_, value, _) | Expr::DefineGlobal(_, value) => inner(value),
            Expr::If(test, then, otherwise) => inner(test) && outer(then) && outer(otherwise),
            Expr::Begin(exprs) | Expr::And(exprs) | Expr::Or(exprs) => sequence(exprs),
            Expr::Lambda(lambda) => !lambda.captures.iter().any(|seen| Rc::ptr_eq(seen, var)),
            Expr::Call(callee, args, _) => {
                let itself = matches!(&**callee, Expr::Local(used) | Expr::LocalAt(used, _)
                    if Rc::ptr_eq(used, var));
                if itself {
                    last && args.len() == count && args.iter().all(inner)
                } else {
                    inner(callee) && args.iter().all(inner)
                }
            }
            Expr::Let(bindings, body) => {
                bindings.iter().all(|(_, value)| inner(value)) && outer(body)
            }
            Expr::Letrec(_, body) => outer(body),
            Expr::Loop { inits, body, .. } => inits.iter().all(inner) && outer(body),
        }
    })
}

fn error(message: &str, at: &Syntax) -> Error {
    Error::new(message).at(at.span)
}

impl<'a> Compiler<'a> {
    pub fn new(core: &'a mut Core) -> Self {
        let keywords = core.keywords;
        Self {
            core,
            funcs: Vec::new(),
            keywords,
            expansions: 0,
            depth: 0,
        }
    }

    fn intern(&mut self, name: &str) -> Symbol {
        self.core.symbols.intern(name)
    }

    /// An identifier that means the top-level binding of `name` wherever it
    /// ends up, whatever the code around it has bound.
    fn global_ident(&mut self, name: &str, span: Span) -> Syntax {
        let name = self.intern(name);
        let id = self.core.fresh_mark();
        let mark = Rc::new(Mark {
            id,
            inner: None,
            scope: None,
        });
        Syntax::new(
            Datum::Ident(Ident {
                name,
                mark: Some(mark),
            }),
            span,
        )
    }

    fn global_ref(&mut self, name: &str, span: Span) -> Expr {
        let symbol = self.intern(name);
        Expr::Global(self.core.global_slot(symbol), span)
    }

    fn call_global(&mut self, name: &str, args: Vec<Expr>, span: Span) -> Expr {
        Expr::Call(Box::new(self.global_ref(name, span)), args, span)
    }

    // Functions and variables

    fn enter_function(&mut self) {
        let id = self.core.fresh_function();
        self.funcs.push(FuncCtx {
            id,
            captures: Vec::new(),
            next_slot: 0,
            vars: Vec::new(),
        });
    }

    fn new_var(&mut self, name: Symbol, recursive: bool) -> Result<Rc<Var>, Error> {
        let func = self.funcs.last_mut().expect("inside a function");
        if func.next_slot == u16::MAX {
            return Err(Error::new("too many local variables in one function"));
        }
        let var = Rc::new(Var {
            name,
            func: Cell::new(func.id),
            slot: Cell::new(func.next_slot),
            captured: Cell::new(false),
            mutated: Cell::new(false),
            recursive,
        });
        func.next_slot += 1;
        func.vars.push(var.clone());
        Ok(var)
    }

    /// Notes a use of a variable. If it belongs to an enclosing function,
    /// every function in between captures it.
    fn refer(&mut self, var: &Rc<Var>) {
        let current = self.funcs.last().expect("inside a function").id;
        if var.func.get() == current {
            return;
        }
        var.captured.set(true);
        for func in self.funcs.iter_mut().rev() {
            if func.id == var.func.get() {
                break;
            }
            if !func.captures.iter().any(|seen| Rc::ptr_eq(seen, var)) {
                func.captures.push(var.clone());
            }
        }
    }

    // Top level

    /// Compiles one form of the top level into a function of no arguments.
    pub fn toplevel(&mut self, form: &Syntax) -> Result<Lambda, Error> {
        self.enter_function();
        let body = self.toplevel_form(form)?;
        let func = self.funcs.pop().expect("the function entered above");
        Ok(Lambda {
            func: func.id,
            params: Vec::new(),
            required: 0,
            rest: false,
            captures: Vec::new(),
            body,
            name: None,
            doc: None,
            span: form.span,
            locals: func.next_slot,
            toplevel: true,
            self_var: None,
            vars: func.vars,
        })
    }

    fn toplevel_form(&mut self, form: &Syntax) -> Result<Expr, Error> {
        let form = self.expand_head(form, &None)?;
        let Some((special, items)) = self.special_form(&form, &None) else {
            return self.expr(&form, &None);
        };
        match special {
            Special::Begin => {
                let forms = items[1..]
                    .iter()
                    .map(|item| self.toplevel_form(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr::Begin(forms))
            }
            Special::Define => {
                let (name, value, _) = self.parse_define(&form, items)?;
                let slot = self.core.global_slot(name.name);
                let value = self.named_expr(&value, &None, Some(name.name))?;
                self.core.note_definition(slot, form.span);
                Ok(Expr::DefineGlobal(slot, Box::new(value)))
            }
            Special::DefineValues => {
                let [_, formals, value] = items else {
                    return Err(error(
                        "define-values takes the variables and an expression",
                        &form,
                    ));
                };
                // Through a list: each variable takes its element.
                let (names, rest) = self.parse_formals(formals)?;
                let list = self.values_as_list(value, &None)?;
                self.enter_scope_let(list, form.span, |compiler, temp| {
                    let mut steps = Vec::new();
                    let mut tail = Expr::Local(temp.clone());
                    for name in &names {
                        let slot = compiler.core.global_slot(name.name);
                        let car = compiler.call_global("car", vec![tail_clone(&tail)], form.span);
                        steps.push(Expr::DefineGlobal(slot, Box::new(car)));
                        tail = compiler.call_global("cdr", vec![tail], form.span);
                    }
                    if let Some(rest) = &rest {
                        let slot = compiler.core.global_slot(rest.name);
                        steps.push(Expr::DefineGlobal(slot, Box::new(tail)));
                    }
                    Ok(Expr::Begin(steps))
                })
            }
            Special::DefineSyntax => {
                let [_, name, spec] = items else {
                    return Err(error("define-syntax takes a name and syntax-rules", &form));
                };
                let name = name
                    .ident()
                    .ok_or_else(|| error("the name of a macro must be an identifier", name))?;
                let defined = Macro::parse(name.name, spec, None, &self.keywords.names)?;
                self.core.macros.insert(name.name, Rc::new(defined));
                Ok(Expr::Const(Value::Void))
            }
            Special::DefineRecordType => {
                let lowered = self.record_type(&form, items)?;
                self.toplevel_form(&lowered)
            }
            Special::Import => Ok(Expr::Const(Value::Void)),
            _ => self.expr(&form, &None),
        }
    }

    /// Binds a temporary to `value` and compiles what `body` makes of it.
    fn enter_scope_let(
        &mut self,
        value: Expr,
        _span: Span,
        body: impl FnOnce(&mut Self, &Rc<Var>) -> Result<Expr, Error>,
    ) -> Result<Expr, Error> {
        let name = self.intern("temporary");
        let temp = self.new_var(name, false)?;
        let body = body(self, &temp)?;
        Ok(Expr::Let(vec![(temp, value)], Box::new(body)))
    }

    /// An expression for the values of `value` as a list.
    fn values_as_list(&mut self, value: &Syntax, env: &Env) -> Result<Expr, Error> {
        let thunk = self.thunk(std::slice::from_ref(value), env, value.span)?;
        let list = self.global_ref("list", value.span);
        Ok(self.call_global("call-with-values", vec![thunk, list], value.span))
    }

    /// A function of no arguments with the given body.
    fn thunk(&mut self, body: &[Syntax], env: &Env, span: Span) -> Result<Expr, Error> {
        self.lambda(&[], None, body, env, None, span)
    }

    // Macros

    /// The special form a list is, if its head names one here.
    fn special_form<'s>(&self, form: &'s Syntax, env: &Env) -> Option<(Special, &'s [Syntax])> {
        let items = form.list()?;
        let head = items.first()?.ident()?;
        match resolve(head, env) {
            Resolved::Global(name) if !self.core.macros.contains_key(&name) => {
                Some((*self.core.specials.get(&name)?, items))
            }
            _ => None,
        }
    }

    /// The macro a form is a use of, if it is one.
    fn macro_use(&self, form: &Syntax, env: &Env) -> Option<Rc<Macro>> {
        let head = form.list()?.first()?.ident()?;
        match resolve(head, env) {
            Resolved::Macro(found) => Some(found),
            Resolved::Global(name) => self.core.macros.get(&name).cloned(),
            Resolved::Var(_) => None,
        }
    }

    /// Expands the form for as long as it is a use of a macro.
    fn expand_head(&mut self, form: &Syntax, env: &Env) -> Result<Syntax, Error> {
        let mut form = form.clone();
        while let Some(found) = self.macro_use(&form, env) {
            self.expansions += 1;
            if self.expansions > 10_000 {
                return Err(error(
                    "this macro's expansion does not come to an end",
                    &form,
                ));
            }
            let mark = self.core.fresh_mark();
            let names = self.keywords.names;
            let is_bound = |ident: &Ident| matches!(resolve(ident, env), Resolved::Var(_));
            form = found.expand(&form, mark, &names, &is_bound)?;
        }
        Ok(form)
    }

    // Expressions

    pub fn expr(&mut self, form: &Syntax, env: &Env) -> Result<Expr, Error> {
        self.named_expr(form, env, None)
    }

    /// Compiles an expression that is about to be given a name, which a
    /// `lambda` then takes as its own.
    fn named_expr(
        &mut self,
        form: &Syntax,
        env: &Env,
        name: Option<Symbol>,
    ) -> Result<Expr, Error> {
        // Macros can nest code deeper than anyone can write it.
        if self.depth > 4000 {
            return Err(error("this code is nested too deeply", form));
        }
        self.depth += 1;
        let expr = crate::deep(|| self.expr_here(form, env, name));
        self.depth -= 1;
        expr
    }

    fn expr_here(&mut self, form: &Syntax, env: &Env, name: Option<Symbol>) -> Result<Expr, Error> {
        match &form.datum {
            Datum::Bool(value) => Ok(Expr::Const(Value::Bool(*value))),
            Datum::Int(value) => Ok(Expr::Const(Value::Int(*value))),
            Datum::Float(value) => Ok(Expr::Const(Value::Float(*value))),
            Datum::Char(value) => Ok(Expr::Const(Value::Char(*value))),
            Datum::Str(_) | Datum::Keyword(_) | Datum::Vector(_) => {
                Ok(Expr::Const(self.core.datum_value(form)))
            }
            Datum::Ident(ident) => match resolve(ident, env) {
                Resolved::Var(var) => {
                    self.refer(&var);
                    Ok(if var.recursive {
                        Expr::LocalAt(var, form.span)
                    } else {
                        Expr::Local(var)
                    })
                }
                Resolved::Macro(_) => Err(error("a macro's name is used as a variable", form)),
                Resolved::Global(name) => {
                    if self.core.macros.contains_key(&name)
                        || self.core.specials.contains_key(&name)
                    {
                        return Err(error("a syntactic keyword is used as a variable", form));
                    }
                    Ok(Expr::Global(self.core.global_slot(name), form.span))
                }
            },
            Datum::Dotted(..) => Err(error("a call cannot have a dot in it", form)),
            Datum::List(items) if items.is_empty() => Err(error(
                "an empty list is not an expression; write '() for the empty list",
                form,
            )),
            Datum::List(_) => {
                let form = self.expand_head(form, env)?;
                if form.list().is_none_or(<[Syntax]>::is_empty) {
                    return self.named_expr(&form, env, name);
                }
                match self.special_form(&form, env) {
                    Some((special, items)) => self.special(special, &form, items, env, name),
                    None => {
                        let items = form.list().expect("checked to be a list");
                        let callee = self.expr(&items[0], env)?;
                        let args = items[1..]
                            .iter()
                            .map(|arg| self.expr(arg, env))
                            .collect::<Result<Vec<_>, _>>()?;
                        if args.len() > usize::from(u8::MAX) {
                            return Err(error("a call can have at most 255 arguments", &form));
                        }
                        Ok(Expr::Call(Box::new(callee), args, form.span))
                    }
                }
            }
        }
    }

    fn sequence(&mut self, forms: &[Syntax], env: &Env) -> Result<Expr, Error> {
        let mut exprs = forms
            .iter()
            .map(|form| self.expr(form, env))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(match exprs.len() {
            0 => Expr::Const(Value::Void),
            1 => exprs.pop().expect("one expression"),
            _ => Expr::Begin(exprs),
        })
    }

    #[allow(clippy::too_many_lines)]
    fn special(
        &mut self,
        special: Special,
        form: &Syntax,
        items: &[Syntax],
        env: &Env,
        name: Option<Symbol>,
    ) -> Result<Expr, Error> {
        match special {
            Special::Quote => match items {
                [_, datum] => Ok(Expr::Const(self.core.datum_value(datum))),
                _ => Err(error("quote takes one datum", form)),
            },
            Special::Quasiquote => match items {
                [_, template] => self.quasi(template, 1, env),
                _ => Err(error("quasiquote takes one template", form)),
            },
            Special::Lambda => match items {
                [_, formals, body @ ..] if !body.is_empty() => {
                    let (params, rest) = self.parse_formals(formals)?;
                    self.lambda(&params, rest.as_ref(), body, env, name, form.span)
                }
                _ => Err(error("lambda takes parameters and a body", form)),
            },
            Special::Define | Special::DefineValues | Special::DefineRecordType => Err(error(
                "a definition is only allowed at the top level or at the start of a body",
                form,
            )),
            Special::DefineSyntax => Err(error(
                "define-syntax is only allowed at the top level or at the start of a body",
                form,
            )),
            Special::Import => Ok(Expr::Const(Value::Void)),
            Special::Set => {
                let [_, target, value] = items else {
                    return Err(error("set! takes a variable and a value", form));
                };
                let ident = target
                    .ident()
                    .ok_or_else(|| error("set! needs a variable to assign to", target))?;
                match resolve(ident, env) {
                    Resolved::Var(var) => {
                        self.refer(&var);
                        var.mutated.set(true);
                        let value = self.named_expr(value, env, Some(var.name))?;
                        Ok(Expr::SetLocal(var, Box::new(value)))
                    }
                    Resolved::Macro(_) => Err(error("a macro cannot be assigned to", target)),
                    Resolved::Global(global) => {
                        let slot = self.core.global_slot(global);
                        let value = self.named_expr(value, env, Some(global))?;
                        Ok(Expr::SetGlobal(slot, Box::new(value), form.span))
                    }
                }
            }
            Special::If => match items {
                [_, test, then] => Ok(Expr::If(
                    Box::new(self.expr(test, env)?),
                    Box::new(self.expr(then, env)?),
                    Box::new(Expr::Const(Value::Void)),
                )),
                [_, test, then, otherwise] => Ok(Expr::If(
                    Box::new(self.expr(test, env)?),
                    Box::new(self.expr(then, env)?),
                    Box::new(self.expr(otherwise, env)?),
                )),
                _ => Err(error(
                    "if takes a test, a consequence and at most one alternative",
                    form,
                )),
            },
            Special::Begin => self.sequence(&items[1..], env),
            Special::And => {
                let exprs = items[1..]
                    .iter()
                    .map(|item| self.expr(item, env))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(match exprs.len() {
                    0 => Expr::Const(Value::Bool(true)),
                    _ => Expr::And(exprs),
                })
            }
            Special::Or => {
                let exprs = items[1..]
                    .iter()
                    .map(|item| self.expr(item, env))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(match exprs.len() {
                    0 => Expr::Const(Value::Bool(false)),
                    _ => Expr::Or(exprs),
                })
            }
            Special::When | Special::Unless => {
                let [_, test, body @ ..] = items else {
                    return Err(error("when and unless take a test and a body", form));
                };
                let test = self.expr(test, env)?;
                let body = self.sequence(body, env)?;
                let nothing = Expr::Const(Value::Void);
                Ok(if special == Special::When {
                    Expr::If(Box::new(test), Box::new(body), Box::new(nothing))
                } else {
                    Expr::If(Box::new(test), Box::new(nothing), Box::new(body))
                })
            }
            Special::Let => self.let_form(form, items, env),
            Special::LetStar => {
                let [_, bindings, body @ ..] = items else {
                    return Err(error("let* takes bindings and a body", form));
                };
                let bindings = self.parse_bindings(bindings)?;
                let mut scope = env.clone();
                let mut bound = Vec::new();
                for (ident, value) in bindings {
                    // Each value sees the variables before it.
                    let value = self.named_expr(&value, &scope, Some(ident.name))?;
                    let var = self.new_var(ident.name, false)?;
                    let inner = Scope::child(&scope);
                    inner.bind(ident, Binding::Var(var.clone()));
                    scope = Some(inner);
                    bound.push((var, value));
                }
                let body = self.body(body, &scope, form)?;
                Ok(Expr::Let(bound, Box::new(body)))
            }
            Special::Letrec => {
                let [_, bindings, body @ ..] = items else {
                    return Err(error("letrec takes bindings and a body", form));
                };
                let bindings = self.parse_bindings(bindings)?;
                let scope = Scope::child(env);
                let mut vars = Vec::new();
                for (ident, _) in &bindings {
                    let var = self.new_var(ident.name, true)?;
                    scope.bind(ident.clone(), Binding::Var(var.clone()));
                    vars.push(var);
                }
                let scope = Some(scope);
                let mut steps = Vec::new();
                for (var, (ident, value)) in vars.iter().zip(&bindings) {
                    let value = self.named_expr(value, &scope, Some(ident.name))?;
                    steps.push(init_local(var.clone(), value));
                }
                steps.push(self.body(body, &scope, form)?);
                Ok(Expr::Letrec(vars, Box::new(Expr::Begin(steps))))
            }
            Special::Cond => self.cond(&items[1..], env, form, None),
            Special::Case => self.case(form, items, env),
            Special::Do => self.do_loop(form, items, env),
            Special::LetSyntax => {
                let [_, bindings, body @ ..] = items else {
                    return Err(error("let-syntax takes macro bindings and a body", form));
                };
                let scope = Scope::child(env);
                let inner = Some(scope.clone());
                for (ident, spec) in self.parse_bindings(bindings)? {
                    let defined =
                        Macro::parse(ident.name, &spec, inner.clone(), &self.keywords.names)?;
                    scope.bind(ident, Binding::Macro(Rc::new(defined)));
                }
                self.body(body, &inner, form)
            }
            Special::Parameterize => {
                let [_, bindings, body @ ..] = items else {
                    return Err(error("parameterize takes bindings and a body", form));
                };
                let mut pairs = Vec::new();
                for binding in bindings
                    .list()
                    .ok_or_else(|| error("parameterize needs a list of bindings", bindings))?
                {
                    let Some([parameter, value]) = binding.list() else {
                        return Err(error("a binding is a parameter and a value", binding));
                    };
                    let parameter = self.expr(parameter, env)?;
                    let value = self.expr(value, env)?;
                    pairs.push(self.call_global("cons", vec![parameter, value], binding.span));
                }
                let pairs = self.call_global("list", pairs, form.span);
                let thunk = self.thunk(body, env, form.span)?;
                Ok(self.call_global("%parameterize", vec![pairs, thunk], form.span))
            }
            Special::Guard => self.guard(form, items, env),
            Special::LetValues | Special::LetStarValues => {
                let [_, bindings, body @ ..] = items else {
                    return Err(error("let-values takes bindings and a body", form));
                };
                let bindings = bindings
                    .list()
                    .ok_or_else(|| error("let-values needs a list of bindings", bindings))?;
                // Nested from the last binding inward. For let-values the
                // values should not see the earlier variables, which this
                // lets them; nothing depends on the difference.
                self.let_values(bindings, body, env, form)
            }
        }
    }

    fn let_values(
        &mut self,
        bindings: &[Syntax],
        body: &[Syntax],
        env: &Env,
        form: &Syntax,
    ) -> Result<Expr, Error> {
        let Some((first, rest)) = bindings.split_first() else {
            return self.body(body, env, form);
        };
        let Some([formals, value]) = first.list() else {
            return Err(error("a binding is the variables and an expression", first));
        };
        let producer = self.thunk(std::slice::from_ref(value), env, value.span)?;
        let (params, rest_param) = self.parse_formals(formals)?;
        // The consumer's body is the rest of the form.
        let consumer = self.lambda_with(
            &params,
            rest_param.as_ref(),
            env,
            None,
            first.span,
            |compiler, inner| compiler.let_values(rest, body, inner, form),
        )?;
        Ok(self.call_global("call-with-values", vec![producer, consumer], first.span))
    }

    /// `(name value)` pairs of a binding form.
    fn parse_bindings(&mut self, bindings: &Syntax) -> Result<Vec<(Ident, Syntax)>, Error> {
        bindings
            .list()
            .ok_or_else(|| error("expected a list of bindings", bindings))?
            .iter()
            .map(|binding| match binding.list() {
                Some([name, value]) => {
                    let ident = name.ident().ok_or_else(|| {
                        error("the name in a binding must be an identifier", name)
                    })?;
                    Ok((ident.clone(), value.clone()))
                }
                _ => Err(error("a binding is a name and a value", binding)),
            })
            .collect()
    }

    /// The parameters of a `lambda`: the required ones, and the one that
    /// takes the rest if there is one.
    fn parse_formals(&mut self, formals: &Syntax) -> Result<(Vec<Ident>, Option<Ident>), Error> {
        let ident = |syntax: &Syntax| {
            syntax
                .ident()
                .cloned()
                .ok_or_else(|| error("a parameter must be an identifier", syntax))
        };
        match &formals.datum {
            Datum::Ident(rest) => Ok((Vec::new(), Some(rest.clone()))),
            Datum::List(items) => Ok((items.iter().map(ident).collect::<Result<_, _>>()?, None)),
            Datum::Dotted(items, rest) => Ok((
                items.iter().map(ident).collect::<Result<_, _>>()?,
                Some(ident(rest)?),
            )),
            _ => Err(error("expected a list of parameters", formals)),
        }
    }

    /// The name and value of a `define`, with `(define (f . args) body)`
    /// turned into a `lambda`. Also returns the name, for the lambda.
    fn parse_define(
        &mut self,
        form: &Syntax,
        items: &[Syntax],
    ) -> Result<(Ident, Syntax, Option<Symbol>), Error> {
        match items {
            [_, target, value] if target.ident().is_some() => {
                let ident = target.ident().expect("checked").clone();
                Ok((ident.clone(), value.clone(), Some(ident.name)))
            }
            [_, target] if target.ident().is_some() => {
                let ident = target.ident().expect("checked").clone();
                let unbound = Syntax::new(Datum::List(Vec::new()), form.span);
                // No value: the variable exists and holds nothing useful.
                let quote = self.global_ident("quote", form.span);
                Ok((ident, form.list_like(vec![quote, unbound]), None))
            }
            [_, target, body @ ..] if !body.is_empty() => {
                // Curried definitions nest: (define ((f a) b) ...).
                let (head, formals) = match &target.datum {
                    Datum::List(parts) if !parts.is_empty() => (
                        parts[0].clone(),
                        Syntax::new(Datum::List(parts[1..].to_vec()), target.span),
                    ),
                    Datum::Dotted(parts, rest) if !parts.is_empty() => (
                        parts[0].clone(),
                        if parts.len() == 1 {
                            (**rest).clone()
                        } else {
                            Syntax::new(
                                Datum::Dotted(parts[1..].to_vec(), rest.clone()),
                                target.span,
                            )
                        },
                    ),
                    _ => {
                        return Err(error(
                            "define needs a name or a name with parameters",
                            target,
                        ));
                    }
                };
                let lambda = self.global_ident("lambda", form.span);
                let mut parts = vec![lambda, formals];
                parts.extend(body.iter().cloned());
                let value = form.list_like(parts);
                if head.ident().is_some() {
                    let ident = head.ident().expect("checked").clone();
                    Ok((ident.clone(), value, Some(ident.name)))
                } else {
                    let define = self.global_ident("define", form.span);
                    let inner = form.list_like(vec![define, head, value]);
                    let items = inner.list().expect("just built").to_vec();
                    self.parse_define(&inner, &items)
                }
            }
            _ => Err(error("define takes a name and a value", form)),
        }
    }

    // Functions

    fn lambda(
        &mut self,
        params: &[Ident],
        rest: Option<&Ident>,
        body: &[Syntax],
        env: &Env,
        name: Option<Symbol>,
        span: Span,
    ) -> Result<Expr, Error> {
        // A string that starts a longer body documents the function.
        let (doc, body) = match body {
            [first, more @ ..] if !more.is_empty() => match &first.datum {
                Datum::Str(text) => (Some(text.clone()), more),
                _ => (None, body),
            },
            _ => (None, body),
        };
        let whole = Syntax::new(Datum::List(Vec::new()), span);
        let mut lambda = self.lambda_with(params, rest, env, name, span, |compiler, inner| {
            compiler.body(body, inner, &whole)
        })?;
        if let Expr::Lambda(lambda) = &mut lambda {
            lambda.doc = doc;
        }
        Ok(lambda)
    }

    /// A function whose body `make_body` compiles in the scope of the
    /// parameters.
    fn lambda_with(
        &mut self,
        params: &[Ident],
        rest: Option<&Ident>,
        env: &Env,
        name: Option<Symbol>,
        span: Span,
        make_body: impl FnOnce(&mut Self, &Env) -> Result<Expr, Error>,
    ) -> Result<Expr, Error> {
        self.enter_function();
        let scope = Scope::child(env);
        let mut vars = Vec::new();
        for ident in params.iter().chain(rest) {
            if vars.len() >= usize::from(u8::MAX) {
                return Err(Error::new("a function can have at most 255 parameters").at(span));
            }
            let var = self.new_var(ident.name, false)?;
            scope.bind(ident.clone(), Binding::Var(var.clone()));
            vars.push(var);
        }
        let body = make_body(self, &Some(scope));
        let func = self.funcs.pop().expect("the function entered above");
        let body = body?;
        // What this function captured from further out, the function
        // around it has to have too; `refer` saw to that.
        Ok(Expr::Lambda(Box::new(Lambda {
            func: func.id,
            params: vars,
            required: params.len(),
            rest: rest.is_some(),
            captures: func.captures,
            body,
            name,
            doc: None,
            span,
            locals: func.next_slot,
            toplevel: false,
            self_var: None,
            vars: func.vars,
        })))
    }

    /// Compiles a body: definitions, which are in scope throughout it, and
    /// expressions.
    fn body(&mut self, forms: &[Syntax], env: &Env, whole: &Syntax) -> Result<Expr, Error> {
        if forms.is_empty() {
            return Err(error("a body needs at least one expression", whole));
        }
        let scope = Scope::child(env);
        let inner = Some(scope.clone());
        let mut items = Vec::new();
        let mut pending: Vec<Syntax> = forms.iter().rev().cloned().collect();
        while let Some(form) = pending.pop() {
            let form = self.expand_head(&form, &inner)?;
            match self.special_form(&form, &inner) {
                Some((Special::Begin, parts)) => {
                    pending.extend(parts[1..].iter().rev().cloned());
                }
                Some((Special::Define, parts)) => {
                    let (ident, value, name) = self.parse_define(&form, parts)?;
                    let var = self.new_var(ident.name, true)?;
                    scope.bind(ident, Binding::Var(var.clone()));
                    items.push(BodyItem::Define(var, value, name));
                }
                Some((Special::DefineValues, parts)) => {
                    let [_, formals, value] = parts else {
                        return Err(error(
                            "define-values takes the variables and an expression",
                            &form,
                        ));
                    };
                    let (names, rest) = self.parse_formals(formals)?;
                    let mut vars = Vec::new();
                    for ident in names {
                        let var = self.new_var(ident.name, true)?;
                        scope.bind(ident, Binding::Var(var.clone()));
                        vars.push(var);
                    }
                    let rest = match rest {
                        Some(ident) => {
                            let var = self.new_var(ident.name, true)?;
                            scope.bind(ident, Binding::Var(var.clone()));
                            Some(var)
                        }
                        None => None,
                    };
                    items.push(BodyItem::DefineValues(vars, rest, value.clone()));
                }
                Some((Special::DefineSyntax, parts)) => {
                    let [_, name, spec] = parts else {
                        return Err(error("define-syntax takes a name and syntax-rules", &form));
                    };
                    let ident = name
                        .ident()
                        .ok_or_else(|| error("the name of a macro must be an identifier", name))?;
                    let defined =
                        Macro::parse(ident.name, spec, inner.clone(), &self.keywords.names)?;
                    scope.bind(ident.clone(), Binding::Macro(Rc::new(defined)));
                }
                Some((Special::DefineRecordType, parts)) => {
                    pending.push(self.record_type(&form, parts)?);
                }
                _ => items.push(BodyItem::Expr(form)),
            }
        }

        let mut vars = Vec::new();
        let mut steps = Vec::new();
        for item in items {
            match item {
                BodyItem::Define(var, value, name) => {
                    let value = self.named_expr(&value, &inner, name)?;
                    vars.push(var.clone());
                    steps.push(init_local(var, value));
                }
                BodyItem::DefineValues(targets, rest, value) => {
                    let list = self.values_as_list(&value, &inner)?;
                    let span = value.span;
                    let step = self.enter_scope_let(list, span, |compiler, temp| {
                        let mut inits = Vec::new();
                        let mut tail = Expr::Local(temp.clone());
                        for target in &targets {
                            let car = compiler.call_global("car", vec![tail_clone(&tail)], span);
                            inits.push(Expr::InitLocal(target.clone(), Box::new(car)));
                            tail = compiler.call_global("cdr", vec![tail], span);
                        }
                        if let Some(rest) = &rest {
                            inits.push(Expr::InitLocal(rest.clone(), Box::new(tail)));
                        }
                        Ok(Expr::Begin(inits))
                    })?;
                    vars.extend(targets_of(&step));
                    steps.push(step);
                }
                BodyItem::Expr(form) => steps.push(self.expr(&form, &inner)?),
            }
        }
        if matches!(steps.last(), Some(Expr::InitLocal(..)) | None) {
            return Err(error(
                "a body must end with an expression, not a definition",
                whole,
            ));
        }
        let body = if steps.len() == 1 {
            steps.pop().expect("one step")
        } else {
            Expr::Begin(steps)
        };
        Ok(if vars.is_empty() {
            body
        } else {
            Expr::Letrec(vars, Box::new(body))
        })
    }

    fn let_form(&mut self, form: &Syntax, items: &[Syntax], env: &Env) -> Result<Expr, Error> {
        match items {
            // Named let: a loop.
            [_, name, bindings, body @ ..] if name.ident().is_some() => {
                let name = name.ident().expect("checked").clone();
                let bindings = self.parse_bindings(bindings)?;
                let values = bindings
                    .iter()
                    .map(|(_, value)| self.expr(value, env))
                    .collect::<Result<Vec<_>, _>>()?;
                let scope = Scope::child(env);
                let var = self.new_var(name.name, true)?;
                scope.bind(name.clone(), Binding::Var(var.clone()));
                let inner = Some(scope);
                let params: Vec<Ident> = bindings.into_iter().map(|(ident, _)| ident).collect();
                let lambda =
                    self.lambda(&params, None, body, &inner, Some(name.name), form.span)?;
                self.refer(&var);
                Ok(self.make_loop(var, lambda, values, form.span))
            }
            [_, bindings, body @ ..] => {
                let bindings = self.parse_bindings(bindings)?;
                // Every value is evaluated outside the new variables.
                let mut bound = Vec::new();
                let scope = Scope::child(env);
                for (ident, value) in bindings {
                    let value = self.named_expr(&value, env, Some(ident.name))?;
                    let var = self.new_var(ident.name, false)?;
                    scope.bind(ident, Binding::Var(var.clone()));
                    bound.push((var, value));
                }
                let body = self.body(body, &Some(scope), form)?;
                Ok(Expr::Let(bound, Box::new(body)))
            }
            _ => Err(error("let takes bindings and a body", form)),
        }
    }

    /// `cond`. `fallback` is what the value is when no clause applies.
    fn cond(
        &mut self,
        clauses: &[Syntax],
        env: &Env,
        form: &Syntax,
        fallback: Option<Expr>,
    ) -> Result<Expr, Error> {
        // Each clause nests the rest one deeper.
        if self.depth > 4000 {
            return Err(error("this cond has too many clauses", form));
        }
        self.depth += 1;
        let expr = crate::deep(|| self.cond_here(clauses, env, form, fallback));
        self.depth -= 1;
        expr
    }

    fn cond_here(
        &mut self,
        clauses: &[Syntax],
        env: &Env,
        form: &Syntax,
        fallback: Option<Expr>,
    ) -> Result<Expr, Error> {
        let Some((clause, rest)) = clauses.split_first() else {
            return Ok(fallback.unwrap_or(Expr::Const(Value::Void)));
        };
        let parts = clause
            .list()
            .filter(|parts| !parts.is_empty())
            .ok_or_else(|| error("a cond clause is a test followed by expressions", clause))?;
        let is = |syntax: &Syntax, name: Symbol| {
            syntax.ident().is_some_and(|ident| {
                ident.name == name && !matches!(resolve(ident, env), Resolved::Var(_))
            })
        };
        if is(&parts[0], self.keywords.else_) {
            if !rest.is_empty() {
                return Err(error("else must be the last clause", clause));
            }
            return self.sequence(&parts[1..], env);
        }
        let test = self.expr(&parts[0], env)?;
        if parts.len() == 1 {
            // The test's value is the value.
            let otherwise = self.cond(rest, env, form, fallback)?;
            return Ok(Expr::Or(vec![test, otherwise]));
        }
        if is(&parts[1], self.keywords.arrow) {
            let [_, _, receiver] = parts else {
                return Err(error("=> is followed by one procedure", clause));
            };
            let receiver = self.expr(receiver, env)?;
            let span = clause.span;
            return self.enter_scope_let(test, span, |compiler, temp| {
                let otherwise = compiler.cond(rest, env, form, fallback)?;
                let call = Expr::Call(Box::new(receiver), vec![Expr::Local(temp.clone())], span);
                Ok(Expr::If(
                    Box::new(Expr::Local(temp.clone())),
                    Box::new(call),
                    Box::new(otherwise),
                ))
            });
        }
        let then = self.sequence(&parts[1..], env)?;
        let otherwise = self.cond(rest, env, form, fallback)?;
        Ok(Expr::If(
            Box::new(test),
            Box::new(then),
            Box::new(otherwise),
        ))
    }

    fn case(&mut self, form: &Syntax, items: &[Syntax], env: &Env) -> Result<Expr, Error> {
        let [_, key, clauses @ ..] = items else {
            return Err(error("case takes a key and clauses", form));
        };
        if clauses.len() > 4000 {
            return Err(error("this case has too many clauses", form));
        }
        let key = self.expr(key, env)?;
        let span = form.span;
        self.enter_scope_let(key, span, |compiler, temp| {
            let mut result = Expr::Const(Value::Void);
            for clause in clauses.iter().rev() {
                let parts = clause
                    .list()
                    .filter(|parts| parts.len() >= 2)
                    .ok_or_else(|| {
                        error("a case clause is a list of data and expressions", clause)
                    })?;
                // `=> procedure` calls the procedure with the key.
                let arrow = compiler.keywords.arrow;
                let body = match &parts[1..] {
                    [marker, receiver]
                        if marker.ident().is_some_and(|ident| ident.name == arrow) =>
                    {
                        let receiver = compiler.expr(receiver, env)?;
                        Expr::Call(
                            Box::new(receiver),
                            vec![Expr::Local(temp.clone())],
                            clause.span,
                        )
                    }
                    body => compiler.sequence(body, env)?,
                };
                let is_else = parts[0]
                    .ident()
                    .is_some_and(|ident| ident.name == compiler.keywords.else_);
                if is_else {
                    result = body;
                    continue;
                }
                let data = parts[0]
                    .list()
                    .ok_or_else(|| error("a case clause starts with a list of data", &parts[0]))?;
                let tests = data
                    .iter()
                    .map(|datum| {
                        let datum = Expr::Const(compiler.core.datum_value(datum));
                        compiler.call_global(
                            "eqv?",
                            vec![Expr::Local(temp.clone()), datum],
                            clause.span,
                        )
                    })
                    .collect::<Vec<_>>();
                let test = match tests.len() {
                    0 => Expr::Const(Value::Bool(false)),
                    _ => Expr::Or(tests),
                };
                result = Expr::If(Box::new(test), Box::new(body), Box::new(result));
            }
            Ok(result)
        })
    }

    /// `(do ((var init step) ...) (test result ...) body ...)`.
    fn do_loop(&mut self, form: &Syntax, items: &[Syntax], env: &Env) -> Result<Expr, Error> {
        let [_, specs, finish, body @ ..] = items else {
            return Err(error("do takes variables, a test and a body", form));
        };
        let specs = specs
            .list()
            .ok_or_else(|| error("do needs a list of variables", specs))?;
        let finish = finish
            .list()
            .filter(|parts| !parts.is_empty())
            .ok_or_else(|| error("do needs a test, with what to return", finish))?;
        let mut params = Vec::new();
        let mut inits = Vec::new();
        let mut steps = Vec::new();
        for spec in specs {
            let (ident, init, step) = match spec.list() {
                Some([name, init]) => (name, init, None),
                Some([name, init, step]) => (name, init, Some(step)),
                _ => {
                    return Err(error(
                        "a do variable is a name, a value and maybe a step",
                        spec,
                    ));
                }
            };
            let ident = ident
                .ident()
                .ok_or_else(|| error("a do variable must be an identifier", ident))?
                .clone();
            inits.push(self.expr(init, env)?);
            steps.push(step.cloned());
            params.push(ident);
        }
        let name = self.intern("do-loop");
        let loop_var = self.new_var(name, true)?;
        let span = form.span;
        let lambda =
            self.lambda_with(&params, None, env, Some(name), span, |compiler, inner| {
                let test = compiler.expr(&finish[0], inner)?;
                let result = compiler.sequence(&finish[1..], inner)?;
                let mut round = body
                    .iter()
                    .map(|part| compiler.expr(part, inner))
                    .collect::<Result<Vec<_>, _>>()?;
                // Go round again with the stepped values.
                let mut next = Vec::new();
                for (param, step) in params.iter().zip(&steps) {
                    let current = Syntax::new(Datum::Ident(param.clone()), span);
                    next.push(compiler.expr(step.as_ref().unwrap_or(&current), inner)?);
                }
                compiler.refer(&loop_var);
                round.push(Expr::Call(
                    Box::new(Expr::Local(loop_var.clone())),
                    next,
                    span,
                ));
                Ok(Expr::If(
                    Box::new(test),
                    Box::new(result),
                    Box::new(Expr::Begin(round)),
                ))
            })?;
        Ok(self.make_loop(loop_var, lambda, inits, span))
    }

    /// A loop: `var` is the name that the body, a function, calls to go
    /// round again, and `inits` are the values of the first turn. Where
    /// every use of the name is a call that the body ends with, the loop
    /// needs no function: its variables join those of the function around
    /// it, and going round is a jump.
    fn make_loop(&mut self, var: Rc<Var>, lambda: Expr, inits: Vec<Expr>, span: Span) -> Expr {
        let func = self.funcs.last_mut().expect("inside a function");
        let lambda = match lambda {
            Expr::Lambda(lambda)
                if !lambda.rest
                    && lambda.required == inits.len()
                    && inits.len() <= usize::from(u8::MAX)
                    && !var.mutated.get()
                    && usize::from(func.next_slot) + lambda.vars.len() < usize::from(u16::MAX)
                    && ends_with_every_call(&lambda.body, &var, inits.len(), true) =>
            {
                for moved in &lambda.vars {
                    moved.func.set(func.id);
                    moved.slot.set(func.next_slot);
                    func.next_slot += 1;
                }
                func.vars.extend(lambda.vars);
                return Expr::Loop {
                    var,
                    params: lambda.params,
                    inits,
                    body: Box::new(lambda.body),
                };
            }
            other => other,
        };
        let call = Expr::Call(Box::new(Expr::Local(var.clone())), inits, span);
        Expr::Letrec(
            vec![var.clone()],
            Box::new(Expr::Begin(vec![init_local(var, lambda), call])),
        )
    }

    /// `(guard (e clause ...) body ...)`: run the body, and if it raises
    /// something, the clauses with `e` bound to it. If no clause applies,
    /// it is raised again.
    fn guard(&mut self, form: &Syntax, items: &[Syntax], env: &Env) -> Result<Expr, Error> {
        let [_, spec, body @ ..] = items else {
            return Err(error(
                "guard takes a variable with clauses, and a body",
                form,
            ));
        };
        let Some([variable, clauses @ ..]) = spec.list() else {
            return Err(error("guard needs (variable clause ...)", spec));
        };
        let ident = variable
            .ident()
            .ok_or_else(|| error("guard needs a variable for what was raised", variable))?
            .clone();
        let span = form.span;
        let handler = self.lambda_with(
            std::slice::from_ref(&ident),
            None,
            env,
            None,
            span,
            |compiler, inner| {
                let variable = Syntax::new(Datum::Ident(ident.clone()), span);
                let raise_again = |compiler: &mut Self| -> Result<Expr, Error> {
                    let raised = compiler.expr(&variable, inner)?;
                    Ok(compiler.call_global("raise", vec![raised], span))
                };
                let unhandled = raise_again(compiler)?;
                let passing = raise_again(compiler)?;
                let clauses = compiler.cond(clauses, inner, form, Some(unhandled))?;
                // An escape is on its way somewhere else: not for the clauses.
                let raised = compiler.expr(&variable, inner)?;
                let is_escape = compiler.call_global("%escape?", vec![raised], span);
                Ok(Expr::If(
                    Box::new(is_escape),
                    Box::new(passing),
                    Box::new(clauses),
                ))
            },
        )?;
        let thunk = self.thunk(body, env, span)?;
        Ok(self.call_global("%call-with-catch", vec![handler, thunk], span))
    }

    /// `define-record-type` as the definitions it stands for.
    fn record_type(&mut self, form: &Syntax, items: &[Syntax]) -> Result<Syntax, Error> {
        let [_, name, constructor, predicate, fields @ ..] = items else {
            return Err(error(
                "define-record-type takes a name, a constructor, a predicate and fields",
                form,
            ));
        };
        let span = form.span;
        let type_name = match &name.datum {
            Datum::Ident(_) => name.clone(),
            // `(name extra ...)`: only the name matters here.
            Datum::List(parts) if !parts.is_empty() => parts[0].clone(),
            _ => return Err(error("a record type needs a name", name)),
        };
        let field_names = fields
            .iter()
            .map(|field| match &field.datum {
                Datum::Ident(_) => Ok(field.clone()),
                Datum::List(parts) if !parts.is_empty() && parts[0].ident().is_some() => {
                    Ok(parts[0].clone())
                }
                _ => Err(error(
                    "a field is a name with its accessor and maybe a modifier",
                    field,
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let quote = |compiler: &mut Self, datum: Syntax| {
            let quote = compiler.global_ident("quote", span);
            form.list_like(vec![quote, datum])
        };
        let call = |compiler: &mut Self, name: &str, args: Vec<Syntax>| {
            let mut parts = vec![compiler.global_ident(name, span)];
            parts.extend(args);
            form.list_like(parts)
        };
        let define = |compiler: &mut Self, target: Syntax, value: Syntax| {
            let define = compiler.global_ident("define", span);
            form.list_like(vec![define, target, value])
        };

        // What the type defines says what it is, for `describe`.
        let type_text = type_name
            .ident()
            .map(|ident| {
                self.core
                    .symbols
                    .name(ident.name)
                    .trim_matches(['<', '>'])
                    .to_owned()
            })
            .unwrap_or_default();
        let field_text = |compiler: &Self, field: &Syntax| {
            field
                .ident()
                .map(|ident| compiler.core.symbols.name(ident.name).to_owned())
                .unwrap_or_default()
        };
        let documented = |compiler: &mut Self, target: Syntax, doc: String, value: Syntax| {
            let define = compiler.global_ident("define", span);
            let doc = Syntax::new(Datum::Str(doc.into()), span);
            form.list_like(vec![define, target, doc, value])
        };
        let mut out = vec![self.global_ident("begin", span)];
        let quoted_name = quote(self, type_name.clone());
        let quoted_fields = quote(self, form.list_like(field_names.clone()));
        let make_type = call(self, "%make-record-type", vec![quoted_name, quoted_fields]);
        out.push(define(self, type_name.clone(), make_type));

        // The constructor: its parameters in the order given, the fields
        // in their own order.
        match &constructor.datum {
            Datum::List(parts) if !parts.is_empty() => {
                let mut values = vec![type_name.clone()];
                for field in &field_names {
                    let given = parts[1..].iter().find(|param| {
                        param.ident().map(|ident| ident.name)
                            == field.ident().map(|ident| ident.name)
                    });
                    values.push(match given {
                        Some(param) => param.clone(),
                        None => Syntax::new(Datum::Bool(false), span),
                    });
                }
                let body = call(self, "%record", values);
                let doc = format!("Makes a {type_text}.");
                out.push(documented(self, constructor.clone(), doc, body));
            }
            Datum::Ident(_) => {
                let mut target = vec![constructor.clone()];
                target.extend(field_names.iter().cloned());
                let mut values = vec![type_name.clone()];
                values.extend(field_names.iter().cloned());
                let body = call(self, "%record", values);
                let doc = format!("Makes a {type_text}.");
                out.push(documented(self, form.list_like(target), doc, body));
            }
            Datum::Bool(false) => {}
            _ => {
                return Err(error(
                    "a constructor is a name with field names",
                    constructor,
                ));
            }
        }
        if predicate.ident().is_some() {
            let object = self.symbol_syntax("object", span);
            let body = call(self, "%record?", vec![type_name.clone(), object.clone()]);
            out.push(documented(
                self,
                form.list_like(vec![predicate.clone(), object]),
                format!("Whether something is a {type_text}."),
                body,
            ));
        }
        for (index, field) in fields.iter().enumerate() {
            let Some(parts) = field.list() else { continue };
            let index_syntax = Syntax::new(Datum::Int(index as i64), span);
            let record = self.symbol_syntax("record", span);
            let value = self.symbol_syntax("value", span);
            if let Some(accessor) = parts.get(1) {
                let who = quote(self, accessor.clone());
                let body = call(
                    self,
                    "%record-ref",
                    vec![type_name.clone(), record.clone(), index_syntax.clone(), who],
                );
                let doc = format!(
                    "The {} of a {type_text}.",
                    field_text(self, &field_names[index])
                );
                out.push(documented(
                    self,
                    form.list_like(vec![accessor.clone(), record.clone()]),
                    doc,
                    body,
                ));
            }
            if let Some(modifier) = parts.get(2) {
                let who = quote(self, modifier.clone());
                let body = call(
                    self,
                    "%record-set!",
                    vec![
                        type_name.clone(),
                        record.clone(),
                        index_syntax,
                        value.clone(),
                        who,
                    ],
                );
                let doc = format!(
                    "Changes the {} of a {type_text}.",
                    field_text(self, &field_names[index])
                );
                out.push(documented(
                    self,
                    form.list_like(vec![modifier.clone(), record, value]),
                    doc,
                    body,
                ));
            }
        }
        Ok(form.list_like(out))
    }

    /// An identifier of its own, which nothing outside can refer to.
    fn symbol_syntax(&mut self, name: &str, span: Span) -> Syntax {
        self.global_ident(name, span)
    }

    // Quasiquote

    /// Whether a template has an unquote at the level that counts.
    fn has_unquote(&self, template: &Syntax, depth: u32) -> bool {
        let head = |items: &[Syntax]| {
            items
                .first()
                .and_then(Syntax::ident)
                .map(|ident| ident.name)
        };
        match &template.datum {
            Datum::List(items) => {
                let name = head(items);
                if name == Some(self.keywords.unquote)
                    || name == Some(self.keywords.unquote_splicing)
                {
                    return depth == 1
                        || items[1..]
                            .iter()
                            .any(|item| self.has_unquote(item, depth - 1));
                }
                if name == Some(self.keywords.quasiquote) {
                    return items[1..]
                        .iter()
                        .any(|item| self.has_unquote(item, depth + 1));
                }
                // `(a . ,b)` reads as `(a unquote b)`.
                let dotted = items.len() >= 3
                    && items[items.len() - 2]
                        .ident()
                        .is_some_and(|ident| ident.name == self.keywords.unquote);
                if dotted && depth == 1 {
                    return true;
                }
                items.iter().any(|item| self.has_unquote(item, depth))
            }
            Datum::Dotted(items, tail) => {
                items.iter().any(|item| self.has_unquote(item, depth))
                    || self.has_unquote(tail, depth)
            }
            Datum::Vector(items) => items.iter().any(|item| self.has_unquote(item, depth)),
            _ => false,
        }
    }

    fn quasi(&mut self, template: &Syntax, depth: u32, env: &Env) -> Result<Expr, Error> {
        crate::deep(|| self.quasi_here(template, depth, env))
    }

    fn quasi_here(&mut self, template: &Syntax, depth: u32, env: &Env) -> Result<Expr, Error> {
        if !crate::deep(|| self.has_unquote(template, depth)) {
            return Ok(Expr::Const(self.core.datum_value(template)));
        }
        let span = template.span;
        let keyword = |items: &[Syntax]| {
            items
                .first()
                .and_then(Syntax::ident)
                .map(|ident| ident.name)
        };
        match &template.datum {
            Datum::List(items) => {
                let name = keyword(items);
                if name == Some(self.keywords.unquote) && items.len() == 2 {
                    if depth == 1 {
                        return self.expr(&items[1], env);
                    }
                    return self.quasi_wrapped("unquote", &items[1], depth - 1, env, span);
                }
                if name == Some(self.keywords.quasiquote) && items.len() == 2 {
                    return self.quasi_wrapped("quasiquote", &items[1], depth + 1, env, span);
                }
                self.quasi_list(items, None, depth, env, span)
            }
            Datum::Dotted(items, tail) => self.quasi_list(items, Some(tail), depth, env, span),
            Datum::Vector(items) => {
                let list = self.quasi_list(items, None, depth, env, span)?;
                Ok(self.call_global("list->vector", vec![list], span))
            }
            _ => Ok(Expr::Const(self.core.datum_value(template))),
        }
    }

    /// `(keyword inner)` rebuilt with `inner` processed a level away.
    fn quasi_wrapped(
        &mut self,
        keyword: &str,
        inner: &Syntax,
        depth: u32,
        env: &Env,
        span: Span,
    ) -> Result<Expr, Error> {
        let symbol = self.intern(keyword);
        let inner = self.quasi(inner, depth, env)?;
        Ok(self.call_global("list", vec![Expr::Const(Value::Sym(symbol)), inner], span))
    }

    fn quasi_list(
        &mut self,
        items: &[Syntax],
        tail: Option<&Syntax>,
        depth: u32,
        env: &Env,
        span: Span,
    ) -> Result<Expr, Error> {
        let mut result = match tail {
            Some(tail) => self.quasi(tail, depth, env)?,
            None => Expr::Const(Value::Nil),
        };
        // The elements in front of `result`, last first. They go into one
        // call rather than a call each, which keeps long lists shallow.
        let mut pending: Vec<Expr> = Vec::new();
        let flush = |compiler: &mut Self, pending: &mut Vec<Expr>, result: Expr| {
            if pending.is_empty() {
                return result;
            }
            let mut args: Vec<Expr> = pending.drain(..).rev().collect();
            args.push(result);
            compiler.call_global("cons*", args, span)
        };
        for (index, item) in items.iter().enumerate().rev() {
            // `(a . ,b)` reads as `(a unquote b)`.
            let is_unquote_tail = index + 2 == items.len()
                && tail.is_none()
                && index > 0
                && item
                    .ident()
                    .is_some_and(|ident| ident.name == self.keywords.unquote);
            if is_unquote_tail && depth == 1 {
                pending.clear();
                result = self.expr(&items[index + 1], env)?;
                continue;
            }
            let splice = item.list().filter(|parts| {
                parts.len() == 2
                    && parts[0]
                        .ident()
                        .is_some_and(|ident| ident.name == self.keywords.unquote_splicing)
            });
            match splice {
                Some(parts) if depth == 1 => {
                    result = flush(self, &mut pending, result);
                    let spliced = self.expr(&parts[1], env)?;
                    result = self.call_global("append", vec![spliced, result], span);
                }
                Some(parts) => {
                    pending.push(self.quasi_wrapped(
                        "unquote-splicing",
                        &parts[1],
                        depth - 1,
                        env,
                        span,
                    )?);
                }
                None => pending.push(self.quasi(item, depth, env)?),
            }
            if pending.len() >= 200 {
                result = flush(self, &mut pending, result);
            }
        }
        Ok(flush(self, &mut pending, result))
    }
}

/// A second reference to a temporary. Only ever called on `Local`.
fn tail_clone(expr: &Expr) -> Expr {
    match expr {
        Expr::Local(var) => Expr::Local(var.clone()),
        Expr::Call(callee, args, span) => Expr::Call(
            Box::new(tail_clone(callee)),
            args.iter().map(tail_clone).collect(),
            *span,
        ),
        Expr::Global(slot, span) => Expr::Global(*slot, *span),
        _ => Expr::Const(Value::Void),
    }
}

/// The variables that a `define-values` step gives values to.
fn targets_of(step: &Expr) -> Vec<Rc<Var>> {
    let mut found = Vec::new();
    if let Expr::Let(_, body) = step
        && let Expr::Begin(inits) = &**body
    {
        for init in inits {
            if let Expr::InitLocal(var, _) = init {
                found.push(var.clone());
            }
        }
    }
    found
}

impl Core {
    /// The value that a piece of syntax is when quoted.
    pub fn datum_value(&mut self, syntax: &Syntax) -> Value {
        crate::deep(|| self.datum_value_here(syntax))
    }

    fn datum_value_here(&mut self, syntax: &Syntax) -> Value {
        match &syntax.datum {
            Datum::Bool(value) => Value::Bool(*value),
            Datum::Int(value) => Value::Int(*value),
            Datum::Float(value) => Value::Float(*value),
            Datum::Char(value) => Value::Char(*value),
            Datum::Str(text) => Value::Obj(self.heap.alloc(Object::Str(text.to_string()))),
            Datum::Ident(ident) => Value::Sym(ident.name),
            Datum::Keyword(name) => {
                // A keyword is a symbol whose name starts with `#:`.
                let name = format!("#:{}", self.symbols.name(*name));
                Value::Sym(self.symbols.intern(&name))
            }
            Datum::List(items) => {
                let mut list = Value::Nil;
                for item in items.iter().rev() {
                    let car = self.datum_value(item);
                    list = Value::Obj(self.heap.alloc(Object::Pair(car, list)));
                }
                list
            }
            Datum::Dotted(items, tail) => {
                let mut list = self.datum_value(tail);
                for item in items.iter().rev() {
                    let car = self.datum_value(item);
                    list = Value::Obj(self.heap.alloc(Object::Pair(car, list)));
                }
                list
            }
            Datum::Vector(items) => {
                let items = items.iter().map(|item| self.datum_value(item)).collect();
                Value::Obj(self.heap.alloc(Object::Vector(items)))
            }
        }
    }
}

impl Core {
    /// A value as source code, for `eval`.
    pub fn value_syntax(&self, value: Value) -> Result<Syntax, Error> {
        self.value_syntax_at(value, 0)
    }

    fn value_syntax_at(&self, value: Value, depth: usize) -> Result<Syntax, Error> {
        // The same limit as for code that is read.
        if depth > 1000 {
            return Err(Error::new("eval: this code is nested too deeply"));
        }
        let span = Span {
            file: crate::vm::NO_FILE,
            start: 0,
            end: 0,
        };
        let datum = match value {
            Value::Nil => Datum::List(Vec::new()),
            Value::Bool(value) => Datum::Bool(value),
            Value::Int(value) => Datum::Int(value),
            Value::Float(value) => Datum::Float(value),
            Value::Char(value) => Datum::Char(value),
            Value::Sym(name) => Datum::Ident(Ident::plain(name)),
            Value::Void | Value::Unbound | Value::Eof => {
                return Err(Error::new("eval: this value cannot be written as code")
                    .with_irritants(vec![value]));
            }
            Value::Obj(handle) => match self.heap.get(handle) {
                Object::Str(text) => Datum::Str(text.as_str().into()),
                Object::Vector(items) => Datum::Vector(
                    items
                        .iter()
                        .map(|item| crate::deep(|| self.value_syntax_at(*item, depth + 1)))
                        .collect::<Result<_, _>>()?,
                ),
                Object::Pair(..) => {
                    let mut items = Vec::new();
                    let mut rest = value;
                    while let Some((car, cdr)) = self.as_pair(rest) {
                        items.push(crate::deep(|| self.value_syntax_at(car, depth + 1))?);
                        rest = cdr;
                        if items.len() > self.heap.len() {
                            return Err(Error::new("eval: the code is a list that never ends"));
                        }
                    }
                    match rest {
                        Value::Nil => Datum::List(items),
                        tail => {
                            Datum::Dotted(items, Box::new(self.value_syntax_at(tail, depth + 1)?))
                        }
                    }
                }
                _ => {
                    return Err(Error::new("eval: this value cannot be written as code")
                        .with_irritants(vec![value]));
                }
            },
        };
        Ok(Syntax::new(datum, span))
    }
}
