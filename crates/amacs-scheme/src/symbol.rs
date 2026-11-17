//! Symbols are interned: each name is stored once and compared by number.

use std::collections::HashMap;
use std::rc::Rc;

use crate::value::Symbol;

#[derive(Default)]
pub struct Interner {
    names: Vec<Rc<str>>,
    numbers: HashMap<Rc<str>, u32>,
    /// Counts the symbols made by `gensym`.
    generated: u32,
}

impl Interner {
    pub fn intern(&mut self, name: &str) -> Symbol {
        if let Some(number) = self.numbers.get(name) {
            return Symbol(*number);
        }
        let name: Rc<str> = name.into();
        self.names.push(name.clone());
        let number = self.names.len() as u32 - 1;
        self.numbers.insert(name, number);
        Symbol(number)
    }

    pub fn name(&self, symbol: Symbol) -> &str {
        &self.names[symbol.0 as usize]
    }

    /// A symbol that no source text can spell.
    pub fn fresh(&mut self, base: &str) -> Symbol {
        loop {
            self.generated += 1;
            let name = format!("{base}%{}", self.generated);
            if !self.numbers.contains_key(name.as_str()) {
                return self.intern(&name);
            }
        }
    }

    /// Every symbol there is, for looking names up by pattern.
    pub fn all(&self) -> impl Iterator<Item = Symbol> + '_ {
        (0..self.names.len() as u32).map(Symbol)
    }
}
