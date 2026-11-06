use std::{
    cell::{Ref, RefCell, RefMut},
    fmt,
    rc::Rc,
};

use crate::{
    Error,
    limits::{MAX_LIST_TRAVERSAL_STEPS, MAX_OUTPUT_BYTES, MAX_RENDER_STEPS, MAX_STRUCTURE_DEPTH},
};

enum RenderPart {
    Value(Value, usize),
    Tail(Value, usize),
    Text(&'static str),
}

#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Symbol(Rc<str>),
    Char(char),
    Pair(Rc<PairCell>),
}

/// Mutable storage for a Bel pair.
///
/// This type is public only because it appears in `Value`'s public
/// representation. Its contents remain private to the bootstrap host.
#[doc(hidden)]
#[derive(Debug)]
pub struct PairCell {
    parts: RefCell<Option<(Value, Value)>>,
}

impl PairCell {
    fn new(car: Value, cdr: Value) -> Self {
        Self {
            parts: RefCell::new(Some((car, cdr))),
        }
    }

    /// Immutably borrow this pair's `(car, cdr)` payload.
    pub fn borrow(&self) -> Ref<'_, (Value, Value)> {
        Ref::map(self.parts.borrow(), |parts| {
            parts.as_ref().expect("live pair has parts")
        })
    }

    /// Mutably borrow this pair's `(car, cdr)` payload.
    pub fn borrow_mut(&self) -> RefMut<'_, (Value, Value)> {
        RefMut::map(self.parts.borrow_mut(), |parts| {
            parts.as_mut().expect("live pair has parts")
        })
    }

    fn take_parts(&self) -> Option<(Value, Value)> {
        self.parts.borrow_mut().take()
    }
}

impl Drop for PairCell {
    fn drop(&mut self) {
        let Some((car, cdr)) = self.parts.get_mut().take() else {
            return;
        };
        let mut pending = vec![car, cdr];
        while let Some(value) = pending.pop() {
            let Value::Pair(pair) = value else {
                continue;
            };
            let Ok(pair) = Rc::try_unwrap(pair) else {
                continue;
            };
            if let Some((car, cdr)) = pair.take_parts() {
                pending.push(car);
                pending.push(cdr);
            }
        }
    }
}

impl Value {
    pub fn symbol(name: &str) -> Self {
        if name == "nil" {
            Self::Nil
        } else {
            Self::Symbol(Rc::from(name))
        }
    }

    pub fn pair(car: Self, cdr: Self) -> Self {
        Self::Pair(Rc::new(PairCell::new(car, cdr)))
    }

    pub fn list(values: impl IntoIterator<Item = Self>) -> Self {
        let values: Vec<_> = values.into_iter().collect();
        values
            .into_iter()
            .rev()
            .fold(Self::Nil, |tail, head| Self::pair(head, tail))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Self::Nil)
    }

    pub(crate) fn is_string(&self) -> bool {
        self.elements()
            .is_ok_and(|values| values.iter().all(|value| matches!(value, Self::Char(_))))
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Nil => Some("nil"),
            Self::Symbol(name) => Some(name),
            Self::Char(_) => None,
            Self::Pair(_) => None,
        }
    }

    pub fn identical(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nil, Self::Nil) => true,
            (Self::Symbol(a), Self::Symbol(b)) => a == b,
            (Self::Char(a), Self::Char(b)) => a == b,
            (Self::Pair(a), Self::Pair(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// Render without expanding shared pairs beyond the experiment's output limits.
    /// A failed rendering returns no partial output.
    pub fn render(&self) -> Result<String, Error> {
        let mut output = String::new();
        let mut pending = vec![RenderPart::Value(self.clone(), 0)];
        let mut steps = 0;
        while let Some(part) = pending.pop() {
            steps += 1;
            if steps > MAX_RENDER_STEPS {
                return Err(output_limit());
            }
            match part {
                RenderPart::Text(text) => append(&mut output, text)?,
                RenderPart::Value(value, depth) => {
                    render_value(value, depth, &mut output, &mut pending)?;
                }
                RenderPart::Tail(value, depth) => {
                    render_tail(value, depth, &mut output, &mut pending)?;
                }
            }
        }
        Ok(output)
    }

    pub(crate) fn elements(&self) -> Result<Vec<Self>, Error> {
        let mut result = Vec::new();
        let mut current = self.clone();
        while let Self::Pair(pair) = current {
            if result.len() >= MAX_LIST_TRAVERSAL_STEPS {
                return Err(Error("expected a finite proper list".into()));
            }
            let (head, tail) = pair.borrow().clone();
            result.push(head);
            current = tail;
        }
        if !current.is_nil() {
            return Err(Error("expected a proper list".into()));
        }
        Ok(result)
    }

    pub(crate) fn pair_parts(&self) -> Option<(Self, Self)> {
        let Self::Pair(pair) = self else {
            return None;
        };
        Some(pair.borrow().clone())
    }

    pub(crate) fn replace_car(&self, value: Self) -> Result<Self, Error> {
        let Self::Pair(pair) = self else {
            return Err(Error("xar requires a pair".into()));
        };
        pair.borrow_mut().0 = value.clone();
        Ok(value)
    }

    pub(crate) fn replace_cdr(&self, value: Self) -> Result<Self, Error> {
        let Self::Pair(pair) = self else {
            return Err(Error("xdr requires a pair".into()));
        };
        pair.borrow_mut().1 = value.clone();
        Ok(value)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.render() {
            Ok(text) => formatter.write_str(&text),
            Err(_) => formatter.write_str("#<value exceeds output limit>"),
        }
    }
}

fn output_limit() -> Error {
    Error("output limit reached; value is too large to print (16 KiB, 8192 rendering steps, or 128 nesting levels)".into())
}

fn append(output: &mut String, text: &str) -> Result<(), Error> {
    if text.len() > MAX_OUTPUT_BYTES - output.len() {
        return Err(output_limit());
    }
    output.push_str(text);
    Ok(())
}

fn render_value(
    value: Value,
    depth: usize,
    output: &mut String,
    pending: &mut Vec<RenderPart>,
) -> Result<(), Error> {
    match &value {
        Value::Nil => append(output, "nil"),
        Value::Symbol(name) => append(output, name),
        Value::Char(character) => append(output, &render_character(*character)),
        Value::Pair(pair) => {
            if depth >= MAX_STRUCTURE_DEPTH {
                return Err(output_limit());
            }
            append(output, "(")?;
            let (head, tail) = pair.borrow().clone();
            pending.push(RenderPart::Text(")"));
            pending.push(RenderPart::Tail(tail, depth + 1));
            pending.push(RenderPart::Value(head, depth + 1));
            Ok(())
        }
    }
}

fn render_character(character: char) -> String {
    let name = match character {
        '\u{7}' => "bel",
        '\t' => "tab",
        '\n' => "lf",
        '\r' => "cr",
        ' ' => "sp",
        _ => return format!("\\{character}"),
    };
    format!("\\{name}")
}

fn render_tail(
    value: Value,
    depth: usize,
    output: &mut String,
    pending: &mut Vec<RenderPart>,
) -> Result<(), Error> {
    match &value {
        Value::Nil => Ok(()),
        Value::Pair(pair) => {
            append(output, " ")?;
            let (head, tail) = pair.borrow().clone();
            pending.push(RenderPart::Tail(tail, depth));
            pending.push(RenderPart::Value(head, depth));
            Ok(())
        }
        _ => {
            append(output, " . ")?;
            pending.push(RenderPart::Value(value, depth));
            Ok(())
        }
    }
}
