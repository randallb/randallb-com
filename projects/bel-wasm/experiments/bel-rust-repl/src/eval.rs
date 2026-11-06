use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use crate::{
    Error, Value,
    limits::{
        MAX_CLEANUP_STEPS, MAX_EVAL_DEPTH, MAX_EVAL_STEPS, MAX_LINEAR_WORK, MAX_STRUCTURE_DEPTH,
    },
    read,
};
const ESCAPE_SENTINEL: &str = "internal continuation escape";

enum ParameterTask {
    List(Value, usize),
    One(Value, usize),
    Leave(Value),
}

#[derive(Clone, Copy)]
struct BindingDepths {
    eval: usize,
    pattern: usize,
}

impl BindingDepths {
    fn nested(self) -> Self {
        Self {
            eval: self.eval + 1,
            pattern: self.pattern + 1,
        }
    }
}

struct BindingBudget<'a> {
    steps: usize,
    fuel: &'a mut usize,
}

// These optimizations are valid only while the exact published closures and
// every binding they can observe remain unchanged.
struct NativeSourceShortcuts {
    whole: Value,
    pint: Value,
    charn: Value,
    binary_less: Value,
    character_positions: HashMap<char, usize>,
    bindings: HashMap<String, Value>,
    protected_pairs: HashSet<usize>,
}

pub struct Interpreter {
    batch_eval_depth_limit: Option<usize>,
    // The comparison oracle runs the published interpreter on a larger test
    // thread stack. Production and host-under-test limits remain unchanged.
    #[cfg(test)]
    pub(crate) test_eval_depth_limit: Option<usize>,
    globals: RefCell<HashMap<String, Value>>,
    globe: Value,
    globe_entry: Value,
    globe_insert_after: Value,
    dynamic: Vec<Value>,
    active_continuations: Vec<u64>,
    pending_escape: Option<(u64, Value)>,
    next_continuation: u64,
    bquote_expansion_depth: usize,
    macro_expansion_depth: usize,
    canonical_bquote: Option<Value>,
    deferring_bquote: bool,
    pending_bquote: Option<Value>,
    native_source_shortcuts: Option<NativeSourceShortcuts>,
}

impl Default for Interpreter {
    fn default() -> Self {
        let globe_entry = Value::pair(Value::symbol("globe"), Value::Nil);
        let globe = Value::pair(globe_entry.clone(), Value::Nil);
        globe_entry
            .replace_cdr(globe.clone())
            .expect("globe entry is a pair");
        let mut globe_insert_after = globe.clone();
        let mut globals = HashMap::from([("globe".into(), globe_entry.clone())]);
        for name in [
            "car", "cdr", "join", "id", "type", "xar", "xdr", "err", "sym", "nom",
        ] {
            let value = Value::list([
                Value::symbol("lit"),
                Value::symbol("prim"),
                Value::symbol(name),
            ]);
            let entry = Value::pair(Value::symbol(name), value);
            let next = Value::pair(entry.clone(), Value::Nil);
            globe_insert_after
                .replace_cdr(next.clone())
                .expect("globe tail is a pair");
            globe_insert_after = next;
            globals.insert(name.into(), entry);
        }
        let chars = character_table();
        let entry = Value::pair(Value::symbol("chars"), chars);
        let next = Value::pair(entry.clone(), Value::Nil);
        globe_insert_after
            .replace_cdr(next.clone())
            .expect("globe tail is a pair");
        globe_insert_after = next;
        globals.insert("chars".into(), entry);
        Self {
            batch_eval_depth_limit: None,
            #[cfg(test)]
            test_eval_depth_limit: None,
            globals: RefCell::new(globals),
            globe,
            globe_entry,
            globe_insert_after,
            dynamic: Vec::new(),
            active_continuations: Vec::new(),
            pending_escape: None,
            next_continuation: 0,
            bquote_expansion_depth: 0,
            macro_expansion_depth: 0,
            canonical_bquote: None,
            deferring_bquote: false,
            pending_bquote: None,
            native_source_shortcuts: None,
        }
    }
}

impl Drop for Interpreter {
    fn drop(&mut self) {
        let _ = self.globe_entry.replace_cdr(Value::Nil);
    }
}

impl Interpreter {
    pub(crate) fn eval_depth_limit(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.test_eval_depth_limit {
            return limit;
        }
        self.batch_eval_depth_limit.unwrap_or(MAX_EVAL_DEPTH)
    }

    /// Opt in to a bounded depth allowance for batch evaluation on a larger stack.
    pub fn set_batch_eval_depth_limit(&mut self, limit: usize) -> Result<(), Error> {
        if !(1..=4096).contains(&limit) {
            return Err(Error(
                "batch evaluation depth must be between 1 and 4096".into(),
            ));
        }
        self.batch_eval_depth_limit = Some(limit);
        Ok(())
    }

    /// Evaluate complete source forms in order, preserving definitions between calls.
    /// Parsing happens first; evaluation errors can leave preceding definitions installed.
    pub fn eval_source(&mut self, source: &str) -> Result<Vec<Value>, Error> {
        let forms = read(source).map_err(|error| Error(error.message))?;
        self.eval_forms(forms)
    }

    pub fn eval_forms(&mut self, forms: Vec<Value>) -> Result<Vec<Value>, Error> {
        self.eval_forms_with_bquote_deferral(forms, false)
    }

    pub(crate) fn eval_bootstrap_bquote_source(
        &mut self,
        source: &str,
    ) -> Result<Vec<Value>, Error> {
        let forms = read(source).map_err(|error| Error(error.message))?;
        self.eval_forms_with_bquote_deferral(forms, true)
    }

    pub(crate) fn register_native_source_shortcuts(&mut self) {
        self.native_source_shortcuts = self.global_bindings().ok().and_then(|bindings| {
            let protected_pairs = binding_pair_ids(&bindings);
            let mut character_positions = HashMap::new();
            for (number, entry) in bindings
                .get("chars")?
                .elements()
                .ok()?
                .into_iter()
                .enumerate()
            {
                let (Value::Char(character), _) = entry.pair_parts()? else {
                    return None;
                };
                character_positions.entry(character).or_insert(number);
            }
            Some(NativeSourceShortcuts {
                whole: bindings.get("whole")?.clone(),
                pint: bindings.get("pint")?.clone(),
                charn: bindings.get("charn")?.clone(),
                binary_less: bindings.get("bin<")?.clone(),
                character_positions,
                bindings,
                protected_pairs,
            })
        });
    }

    fn eval_forms_with_bquote_deferral(
        &mut self,
        forms: Vec<Value>,
        defer_bquote: bool,
    ) -> Result<Vec<Value>, Error> {
        self.deferring_bquote = defer_bquote;
        self.pending_bquote = None;
        let mut results = Vec::with_capacity(forms.len());
        for form in forms {
            let mut fuel = MAX_EVAL_STEPS;
            match self.eval(&form, &Value::Nil, 0, &mut fuel) {
                Ok(result) => results.push(result),
                Err(error) => {
                    self.deferring_bquote = false;
                    self.pending_bquote = None;
                    return Err(error);
                }
            }
        }
        self.deferring_bquote = false;
        if let Some(bquote) = self.pending_bquote.take() {
            self.canonical_bquote = Some(bquote.clone());
            self.store_global("bquote", bquote)?;
        }
        Ok(results)
    }

    pub(crate) fn eval(
        &mut self,
        form: &Value,
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        if depth > self.eval_depth_limit() {
            return Err(Error(
                "evaluation depth limit reached; the prompt is still usable".into(),
            ));
        }
        if *fuel == 0 {
            return Err(Error(
                "evaluation step limit reached; the prompt is still usable".into(),
            ));
        }
        *fuel -= 1;
        match form {
            Value::Nil => Ok(Value::Nil),
            Value::Char(_) => Ok(form.clone()),
            Value::Symbol(name) if name.as_ref() == "scope" => {
                let binding = match self.dynamic_binding(form) {
                    Some(value) => Some(value),
                    None => self.binding(form, env)?,
                };
                Ok(binding
                    .or_else(|| self.global("scope"))
                    .unwrap_or_else(|| env.clone()))
            }
            Value::Symbol(name) if matches!(name.as_ref(), "t" | "o" | "apply") => Ok(form.clone()),
            Value::Symbol(name) => self.lookup(name, env),
            Value::Pair(_) if self.is_unique_variable(form) => self.lookup_variable(form, env),
            Value::Pair(pair) if pair.borrow().0.name() == Some("lit") => Ok(form.clone()),
            Value::Pair(_) if form.is_string() => Ok(form.clone()),
            Value::Pair(pair) => {
                let (head, tail) = pair.borrow().clone();
                self.expression(&head, &tail, env, depth, fuel)
            }
        }
    }

    fn lookup(&self, name: &str, env: &Value) -> Result<Value, Error> {
        let symbol = Value::symbol(name);
        if let Some(value) = self.dynamic_binding(&symbol) {
            return Ok(value);
        }
        if let Some(value) = self.binding(&symbol, env)? {
            return Ok(value);
        }
        self.global(name)
            .ok_or_else(|| Error(format!("unbound symbol: {name}")))
    }

    fn global(&self, name: &str) -> Option<Value> {
        self.global_cell(name)
            .and_then(|cell| cell.pair_parts())
            .map(|(_, value)| value)
    }

    fn global_cell(&self, name: &str) -> Option<Value> {
        if let Some(cell) = self.globals.borrow().get(name).cloned() {
            return Some(cell);
        }
        let symbol = Value::symbol(name);
        let mut current = self.globe.clone();
        let mut visited = Vec::new();
        while let Ok(Some((entry, tail))) =
            bounded_list_pair(&current, &mut visited, "global environment")
        {
            if entry
                .pair_parts()
                .is_some_and(|(key, _)| key.identical(&symbol))
            {
                self.globals.borrow_mut().insert(name.into(), entry.clone());
                return Some(entry);
            }
            current = tail;
        }
        None
    }

    fn global_bindings(&self) -> Result<HashMap<String, Value>, Error> {
        let mut bindings = HashMap::new();
        let mut current = self.globe.clone();
        let mut visited = Vec::new();
        while let Some((entry, tail)) =
            bounded_list_pair(&current, &mut visited, "global environment")?
        {
            let Some((key, value)) = entry.pair_parts() else {
                return Err(Error("invalid global environment".into()));
            };
            let Some(name) = key.name() else {
                return Err(Error("invalid global environment".into()));
            };
            bindings.entry(name.into()).or_insert(value);
            current = tail;
        }
        Ok(bindings)
    }

    fn store_global(&mut self, name: &str, value: Value) -> Result<Value, Error> {
        if name == "bquote" && self.deferring_bquote {
            self.pending_bquote = Some(value.clone());
            return Ok(value);
        }
        if let Some(cell) = self.global_cell(name) {
            cell.replace_cdr(value.clone())?;
            self.globals.borrow_mut().entry(name.into()).or_insert(cell);
        } else {
            self.install_global(name, value.clone())?;
        }
        Ok(value)
    }

    fn install_global(&mut self, name: &str, value: Value) -> Result<Value, Error> {
        let insert_after = self.live_global_insertion_node()?;
        let entry = Value::pair(Value::symbol(name), value.clone());
        let (_, existing_tail) = insert_after
            .pair_parts()
            .ok_or_else(|| Error("invalid global environment".into()))?;
        let next = Value::pair(entry.clone(), existing_tail);
        insert_after.replace_cdr(next)?;
        self.globe_insert_after = insert_after;
        self.globals.borrow_mut().insert(name.into(), entry);
        Ok(value)
    }

    fn live_global_insertion_node(&self) -> Result<Value, Error> {
        let (anchor, _) = self
            .globe_insert_after
            .pair_parts()
            .ok_or_else(|| Error("invalid global insertion point".into()))?;
        let mut current = self.globe.clone();
        let mut visited = Vec::new();
        while let Some((entry, tail)) =
            bounded_list_pair(&current, &mut visited, "global environment")?
        {
            if entry.identical(&anchor) {
                return Ok(current);
            }
            current = tail;
        }
        Err(Error(
            "global insertion point is detached from the live environment".into(),
        ))
    }

    fn lookup_variable(&self, variable: &Value, env: &Value) -> Result<Value, Error> {
        if let Some(value) = self.dynamic_binding(variable) {
            return Ok(value);
        }
        self.binding(variable, env)?
            .ok_or_else(|| Error(format!("unbound variable: {variable}")))
    }

    fn dynamic_binding(&self, variable: &Value) -> Option<Value> {
        self.dynamic_cell(variable)
            .and_then(|cell| cell.pair_parts())
            .map(|(_, value)| value)
    }

    fn dynamic_cell(&self, variable: &Value) -> Option<Value> {
        self.dynamic.iter().rev().find_map(|cell| {
            let (name, _) = cell.pair_parts()?;
            name.identical(variable).then(|| cell.clone())
        })
    }

    fn binding(&self, variable: &Value, env: &Value) -> Result<Option<Value>, Error> {
        let mut current = env.clone();
        let mut visited = Vec::new();
        while let Some((entry, tail)) =
            bounded_list_pair(&current, &mut visited, "closure environment")?
        {
            let Some((name, value)) = entry.pair_parts() else {
                return Err(Error("invalid closure environment".into()));
            };
            if name.identical(variable) {
                return Ok(Some(value));
            }
            current = tail;
        }
        Ok(None)
    }

    fn is_unique_variable(&self, value: &Value) -> bool {
        let Some(vmark) = self.global("vmark") else {
            return false;
        };
        value.identical(&vmark)
            || matches!(value, Value::Pair(pair) if pair.borrow().0.identical(&vmark))
    }

    fn is_bindable_variable(&self, value: &Value) -> bool {
        value
            .name()
            .is_some_and(|name| !matches!(name, "nil" | "t" | "o" | "apply"))
            || self.is_unique_variable(value)
    }

    fn expression(
        &mut self,
        head: &Value,
        tail: &Value,
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        let arguments = tail.elements()?;
        if self.deferring_bquote
            && head.name() == Some("mac")
            && arguments.first().and_then(Value::name) == Some("bquote")
        {
            return self.define_macro(&arguments, env);
        }
        if let Some(name) = head
            .name()
            .filter(|name| matches!(*name, "bquote" | "fn" | "def" | "mac" | "set"))
            && let Ok(function) = self.lookup(name, env)
        {
            if macro_expander(&function).is_some() {
                let host_bquote = name == "bquote"
                    && self
                        .canonical_bquote
                        .as_ref()
                        .is_some_and(|canonical| canonical.identical(&function))
                    && (self.bquote_expansion_depth != 0 || self.macro_expansion_depth != 0);
                if !host_bquote {
                    if name == "bquote" {
                        return self.expand_bquote_macro(
                            &function,
                            &arguments,
                            env,
                            depth + 1,
                            fuel,
                        );
                    }
                    return self.expand_macro(&function, &arguments, env, depth + 1, fuel);
                }
            } else {
                let function = self.eval(head, env, depth + 1, fuel)?;
                return self.apply_arguments(&function, &arguments, env, depth + 1, fuel);
            }
        }
        match head.name() {
            Some("quote") => {
                exact(&arguments, 1, "quote")?;
                Ok(arguments[0].clone())
            }
            Some("if") => self.conditional(&arguments, env, depth + 1, fuel),
            Some("dyn") => self.dynamic_form(&arguments, env, depth + 1, fuel),
            Some("after") => self.after_form(&arguments, env, depth + 1, fuel),
            Some("ccc") => self.call_with_continuation(&arguments, env, depth + 1, fuel),
            Some("where") => self.where_form(&arguments, env, depth + 1, fuel),
            Some("bquote") => {
                exact(&arguments, 1, "bquote")?;
                crate::quasiquote::expand(self, &arguments[0], env, 1, depth + 1, fuel)
            }
            Some("fn") => self.closure(&arguments, env),
            Some("def") => self.define(&arguments, env),
            Some("mac") => self.define_macro(&arguments, env),
            Some("set") => self.set_global(&arguments, env, depth + 1, fuel),
            _ => {
                let function = self.eval(head, env, depth + 1, fuel)?;
                if macro_expander(&function).is_some() {
                    return self.expand_macro(&function, &arguments, env, depth + 1, fuel);
                }
                self.apply_arguments(&function, &arguments, env, depth + 1, fuel)
            }
        }
    }

    fn apply_arguments(
        &mut self,
        function: &Value,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        let values = arguments
            .iter()
            .map(|argument| self.eval(argument, env, depth, fuel))
            .collect::<Result<Vec<_>, _>>()?;
        self.apply(function, values, env, depth, fuel)
    }

    fn conditional(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        for pair in arguments.chunks(2) {
            let test = self.eval(&pair[0], env, depth, fuel)?;
            if pair.len() == 1 {
                return Ok(test);
            }
            if !test.is_nil() {
                return self.eval(&pair[1], env, depth, fuel);
            }
        }
        Ok(Value::Nil)
    }

    fn dynamic_form(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        exact(arguments, 3, "dyn")?;
        if !self.is_bindable_variable(&arguments[0]) {
            return Err(Error("dyn requires a variable name".into()));
        }
        let value = self.eval(&arguments[1], env, depth + 1, fuel)?;
        let cell = Value::pair(arguments[0].clone(), value);
        self.dynamic.push(cell.clone());
        let result = match self.eval(&arguments[2], env, depth + 1, fuel) {
            Err(error) if is_catchable_runtime_error(&error) => {
                let err = Value::symbol("err");
                if let Some(handler) = self.dynamic_binding(&err) {
                    self.apply(
                        &handler,
                        vec![Value::symbol(&error.0)],
                        env,
                        depth + 1,
                        fuel,
                    )
                } else {
                    Err(error)
                }
            }
            result => result,
        };
        self.dynamic.pop();
        result
    }

    fn after_form(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        exact(arguments, 2, "after")?;
        let result = self.eval(&arguments[0], env, depth + 1, fuel);
        let escape = if result
            .as_ref()
            .is_err_and(|error| error.0 == ESCAPE_SENTINEL)
        {
            self.pending_escape.take()
        } else {
            None
        };
        let exhausted = result.as_ref().is_err_and(|error| {
            error.0 == "evaluation step limit reached; the prompt is still usable"
        });
        let cleanup = if exhausted {
            let mut cleanup_fuel = MAX_CLEANUP_STEPS;
            self.eval(&arguments[1], env, depth + 1, &mut cleanup_fuel)
        } else {
            self.eval(&arguments[1], env, depth + 1, fuel)
        };
        match cleanup {
            Ok(_) => {
                if let Some(escape) = escape {
                    self.pending_escape = Some(escape);
                }
                result
            }
            Err(error) => Err(error),
        }
    }

    fn call_with_continuation(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        exact(arguments, 1, "ccc")?;
        let function = self.eval(&arguments[0], env, depth + 1, fuel)?;
        let id = self.next_continuation;
        self.next_continuation += 1;
        self.active_continuations.push(id);
        let continuation = Value::list([
            Value::symbol("lit"),
            Value::symbol("cont"),
            Value::symbol(&format!("continuation-{id}")),
        ]);
        let result = self.apply(&function, vec![continuation], env, depth + 1, fuel);
        self.active_continuations.pop();
        if result
            .as_ref()
            .is_err_and(|error| error.0 == ESCAPE_SENTINEL)
            && self
                .pending_escape
                .as_ref()
                .is_some_and(|(target, _)| *target == id)
        {
            return Ok(self.pending_escape.take().expect("checked").1);
        }
        result
    }

    fn where_form(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(Error(format!(
                "where expects a place and optional creation flag, got {} item(s)",
                arguments.len()
            )));
        }
        let place = &arguments[0];
        if self.is_bindable_variable(place) {
            if let Some(cell) = self.dynamic_cell(place) {
                return Ok(Value::list([cell, Value::symbol("d")]));
            }
            if let Some(cell) = self.binding_cell(place, env)? {
                return Ok(Value::list([cell, Value::symbol("d")]));
            }
            let Some(name) = place.name() else {
                return Err(Error("cannot create a global unique variable".into()));
            };
            let mut cell = self.global_cell(name);
            if cell.is_none() {
                let create = arguments
                    .get(1)
                    .map(|flag| self.eval(flag, env, depth + 1, fuel))
                    .transpose()?
                    .is_some_and(|value| !value.is_nil());
                if !create {
                    return Err(Error(format!("unbound place: {name}")));
                }
                self.install_global(name, Value::Nil)?;
                cell = self.global_cell(name);
            }
            return Ok(Value::list([
                cell.expect("inserted or present"),
                Value::symbol("d"),
            ]));
        }
        let Some((operator, tail)) = place.pair_parts() else {
            return Err(Error(
                "where requires a variable, car place, or cdr place".into(),
            ));
        };
        let place_arguments = tail.elements()?;
        let resolved_operator = self.eval(&operator, env, depth + 1, fuel)?;
        if macro_expander(&resolved_operator).is_some() {
            let expanded =
                self.macro_expansion(&resolved_operator, &place_arguments, env, depth + 1, fuel)?;
            let mut expanded_arguments = vec![expanded];
            expanded_arguments.extend(arguments.iter().skip(1).cloned());
            return self.where_form(&expanded_arguments, env, depth + 1, fuel);
        }
        let location = pair_place_location(&resolved_operator);
        if let Some(location) = location {
            exact(&place_arguments, 1, "place")?;
            let cell = self.eval(&place_arguments[0], env, depth + 1, fuel)?;
            if cell.pair_parts().is_none() {
                return Err(Error("car and cdr places require a pair".into()));
            }
            return Ok(Value::list([cell, Value::symbol(location)]));
        }
        exact(&place_arguments, 1, "table place")?;
        if !is_literal_kind(&resolved_operator, "tab") {
            return Err(Error("unsupported place expression".into()));
        }
        let key = self.eval(&place_arguments[0], env, depth + 1, fuel)?;
        let (table_tail, entries) = validated_table_entries(&resolved_operator)?;
        let get_symbol = Value::symbol("get");
        let get = self
            .dynamic_binding(&get_symbol)
            .or_else(|| self.global("get"))
            .ok_or_else(|| Error("unbound symbol: get".into()))?;
        let mut entry = self.apply(
            &get,
            vec![key.clone(), entries.clone()],
            env,
            depth + 1,
            fuel,
        )?;
        if entry.is_nil() {
            entry = Value::pair(key, Value::Nil);
            table_tail.replace_cdr(Value::pair(entry.clone(), entries))?;
        }
        if entry.pair_parts().is_none() {
            return Err(Error("table lookup did not return an entry pair".into()));
        }
        Ok(Value::list([entry, Value::symbol("d")]))
    }

    fn binding_cell(&self, variable: &Value, env: &Value) -> Result<Option<Value>, Error> {
        let mut current = env.clone();
        let mut visited = Vec::new();
        while let Some((entry, tail)) =
            bounded_list_pair(&current, &mut visited, "closure environment")?
        {
            let Some((name, _)) = entry.pair_parts() else {
                return Err(Error("invalid closure environment".into()));
            };
            if name.identical(variable) {
                return Ok(Some(entry));
            }
            current = tail;
        }
        Ok(None)
    }

    fn define(&mut self, arguments: &[Value], env: &Value) -> Result<Value, Error> {
        exact(arguments, 3, "def")?;
        let Some(name) = arguments[0].name().filter(|name| *name != "nil") else {
            return Err(Error("def requires a non-nil symbol name".into()));
        };
        let function = self.closure(&arguments[1..], env)?;
        self.store_global(name, function)
    }

    fn define_macro(&mut self, arguments: &[Value], env: &Value) -> Result<Value, Error> {
        exact(arguments, 3, "mac")?;
        let Some(name) = arguments[0].name().filter(|name| *name != "nil") else {
            return Err(Error("mac requires a non-nil symbol name".into()));
        };
        let macro_value = Value::list([
            Value::symbol("lit"),
            Value::symbol("mac"),
            self.closure(&arguments[1..], env)?,
        ]);
        self.store_global(name, macro_value)
    }

    fn set_global(
        &mut self,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        if arguments.is_empty() || !arguments.len().is_multiple_of(2) {
            return Err(Error(format!(
                "set expects one or more name/value pairs, got {} item(s)",
                arguments.len()
            )));
        }
        let mut result = Value::Nil;
        for pair in arguments.chunks_exact(2) {
            let Some(name) = pair[0].name().filter(|name| *name != "nil") else {
                return Err(Error("set requires non-nil symbol names".into()));
            };
            result = self.eval(&pair[1], env, depth, fuel)?;
            self.store_global(name, result.clone())?;
        }
        Ok(result)
    }

    fn expand_macro(
        &mut self,
        macro_value: &Value,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        let expansion = self.macro_expansion(macro_value, arguments, env, depth, fuel)?;
        self.eval(&expansion, env, depth + 1, fuel)
    }

    fn macro_expansion(
        &mut self,
        macro_value: &Value,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        let expander =
            macro_expander(macro_value).ok_or_else(|| Error("invalid macro literal".into()))?;
        self.macro_expansion_depth += 1;
        let expansion = self.apply(&expander, arguments.to_vec(), env, depth + 1, fuel);
        self.macro_expansion_depth -= 1;
        expansion
    }

    fn expand_bquote_macro(
        &mut self,
        macro_value: &Value,
        arguments: &[Value],
        env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        let expander =
            macro_expander(macro_value).ok_or_else(|| Error("invalid macro literal".into()))?;
        self.bquote_expansion_depth += 1;
        let result = self
            .apply(&expander, arguments.to_vec(), env, depth + 1, fuel)
            .and_then(|expansion| self.eval(&expansion, env, depth + 1, fuel));
        self.bquote_expansion_depth -= 1;
        result
    }

    fn closure(&self, arguments: &[Value], env: &Value) -> Result<Value, Error> {
        exact(arguments, 2, "fn")?;
        self.validate_parameters(&arguments[0])?;
        Ok(Value::list([
            Value::symbol("lit"),
            Value::symbol("clo"),
            env.clone(),
            arguments[0].clone(),
            arguments[1].clone(),
        ]))
    }

    fn apply(
        &mut self,
        function: &Value,
        arguments: Vec<Value>,
        caller_env: &Value,
        depth: usize,
        fuel: &mut usize,
    ) -> Result<Value, Error> {
        if depth > self.eval_depth_limit() {
            return Err(Error(
                "evaluation depth limit reached; the prompt is still usable".into(),
            ));
        }
        if *fuel == 0 {
            return Err(Error(
                "evaluation step limit reached; the prompt is still usable".into(),
            ));
        }
        *fuel -= 1;
        if function.name() == Some("apply") {
            if arguments.len() < 2 {
                return Err(Error(format!(
                    "apply expects at least 2 argument(s), got {}",
                    arguments.len()
                )));
            }
            let callee = arguments[0].clone();
            let mut applied = arguments[1..arguments.len() - 1].to_vec();
            applied.extend(
                arguments
                    .last()
                    .expect("length checked")
                    .elements()
                    .map_err(|_| {
                        Error("apply requires its final argument to be a proper list".into())
                    })?,
            );
            return self.apply(&callee, applied, caller_env, depth + 1, fuel);
        }
        if let Some(result) = self.native_binary_less(function, &arguments) {
            return Ok(result);
        }
        if let Some(result) = self.native_character_number(function, &arguments) {
            return Ok(result);
        }
        let literal = function
            .elements()
            .map_err(|_| Error(format!("cannot call {function}")))?;
        match literal.as_slice() {
            [lit, kind, primitive] if lit.name() == Some("lit") && kind.name() == Some("prim") => {
                let mutates_pair = matches!(primitive.name(), Some("xar" | "xdr"));
                let invalidates_source_shortcuts = mutates_pair
                    && arguments.first().is_some_and(|target| {
                        self.native_source_shortcuts.as_ref().is_some_and(|native| {
                            pair_id(target).is_some_and(|id| native.protected_pairs.contains(&id))
                        })
                    });
                let result = primitive_call(primitive, arguments);
                if result.is_ok() && mutates_pair {
                    self.globals.borrow_mut().clear();
                    if invalidates_source_shortcuts {
                        self.native_source_shortcuts = None;
                    }
                }
                result
            }
            [lit, kind, env, params, body]
                if lit.name() == Some("lit") && kind.name() == Some("clo") =>
            {
                self.validate_parameters(params)?;
                let mut budget = BindingBudget { steps: 0, fuel };
                let local = self.bind_pattern(
                    params,
                    &Value::list(arguments),
                    env.clone(),
                    BindingDepths {
                        eval: depth + 1,
                        pattern: 0,
                    },
                    &mut budget,
                )?;
                self.eval(body, &local, depth + 1, budget.fuel)
            }
            [lit, kind, _] if lit.name() == Some("lit") && kind.name() == Some("mac") => {
                let quoted_arguments = arguments
                    .into_iter()
                    .map(|argument| Value::list([Value::symbol("quote"), argument]))
                    .collect::<Vec<_>>();
                self.expand_macro(function, &quoted_arguments, caller_env, depth + 1, fuel)
            }
            [lit, kind, token] if lit.name() == Some("lit") && kind.name() == Some("cont") => {
                exact(&arguments, 1, "continuation")?;
                let id = continuation_id(token)?;
                if !self.active_continuations.contains(&id) {
                    return Err(Error("continuation is no longer active".into()));
                }
                self.pending_escape = Some((id, arguments[0].clone()));
                Err(Error(ESCAPE_SENTINEL.into()))
            }
            [lit, kind, ..] if lit.name() == Some("lit") => {
                let Some(expander) = self.virtual_expander(kind)? else {
                    return Err(Error(format!(
                        "cannot call {function}; no virtual function is registered for {kind}"
                    )));
                };
                let quoted_arguments = Value::list(
                    arguments
                        .into_iter()
                        .map(|argument| Value::list([Value::symbol("quote"), argument])),
                );
                let expansion = self.apply(
                    &expander,
                    vec![function.clone(), quoted_arguments],
                    caller_env,
                    depth + 1,
                    fuel,
                )?;
                self.eval(&expansion, caller_env, depth + 1, fuel)
            }
            _ => Err(Error(format!(
                "cannot call {function}; supported functions are primitive or closure literals"
            ))),
        }
    }

    fn virtual_expander(&self, tag: &Value) -> Result<Option<Value>, Error> {
        let registry = Value::symbol("virfns");
        let Some(mut bindings) = self
            .dynamic_binding(&registry)
            .or_else(|| self.global("virfns"))
        else {
            return Ok(None);
        };
        let mut visited = Vec::new();
        while let Some((entry, tail)) =
            bounded_list_pair(&bindings, &mut visited, "virtual function registry")?
        {
            let Some((key, value)) = entry.pair_parts() else {
                return Err(Error("invalid virtual function registry entry".into()));
            };
            if key.identical(tag) {
                return Ok(Some(value));
            }
            bindings = tail;
        }
        Ok(None)
    }

    fn validate_parameters(&self, parameters: &Value) -> Result<(), Error> {
        let mut tasks = vec![ParameterTask::List(parameters.clone(), 0)];
        let mut active_pairs: Vec<Value> = Vec::new();
        let mut seen_parameters: Vec<Value> = Vec::new();
        let mut steps = 0;
        while let Some(task) = tasks.pop() {
            steps += 1;
            if steps > MAX_LINEAR_WORK {
                return Err(Error(
                    "parameter validation step limit reached; the prompt is still usable".into(),
                ));
            }
            let (value, depth, is_list) = match task {
                ParameterTask::Leave(pair) => {
                    let active = active_pairs.pop().expect("leave follows enter");
                    debug_assert!(active.identical(&pair));
                    continue;
                }
                ParameterTask::List(value, depth) => (value, depth, true),
                ParameterTask::One(value, depth) => (value, depth, false),
            };
            if depth > MAX_STRUCTURE_DEPTH {
                return Err(Error(
                    "parameter pattern depth limit reached; the prompt is still usable".into(),
                ));
            }
            if self.is_bindable_variable(&value) {
                self.record_parameter(&value, &mut seen_parameters)?;
                continue;
            }
            if let Some((target, _)) = typed_spec(&value)? {
                self.enter_parameter_pair(
                    &value,
                    &mut active_pairs,
                    &mut tasks,
                    ParameterTask::One(target, depth + 1),
                )?;
                continue;
            }
            if value.is_nil() {
                continue;
            }
            if !is_list {
                if let Some((target, _)) = optional_spec(&value)? {
                    self.enter_parameter_pair(
                        &value,
                        &mut active_pairs,
                        &mut tasks,
                        ParameterTask::One(target, depth + 1),
                    )?;
                    continue;
                }
                if value
                    .pair_parts()
                    .is_some_and(|(head, _)| head.name() == Some("lit"))
                {
                    return Err(Error("literal values cannot be parameters".into()));
                }
            }
            let Some((head, tail)) = value.pair_parts() else {
                return Err(Error(if is_list {
                    "invalid parameter list".into()
                } else {
                    "parameters must be variables, optional forms, or pair patterns".into()
                }));
            };
            self.enter_parameter_pair(
                &value,
                &mut active_pairs,
                &mut tasks,
                ParameterTask::List(tail, depth),
            )?;
            tasks.push(ParameterTask::One(head, depth + 1));
        }
        Ok(())
    }

    fn enter_parameter_pair(
        &self,
        pair: &Value,
        active_pairs: &mut Vec<Value>,
        tasks: &mut Vec<ParameterTask>,
        continuation: ParameterTask,
    ) -> Result<(), Error> {
        if active_pairs.iter().any(|active| active.identical(pair)) {
            return Err(Error(
                "cyclic parameter pattern; the prompt is still usable".into(),
            ));
        }
        active_pairs.push(pair.clone());
        tasks.push(ParameterTask::Leave(pair.clone()));
        tasks.push(continuation);
        Ok(())
    }

    fn record_parameter(&self, parameter: &Value, seen: &mut Vec<Value>) -> Result<(), Error> {
        if seen.iter().any(|prior| prior.identical(parameter)) {
            return Err(Error(format!("duplicate parameter: {parameter}")));
        }
        seen.push(parameter.clone());
        Ok(())
    }

    fn bind_pattern(
        &mut self,
        pattern: &Value,
        argument: &Value,
        env: Value,
        depths: BindingDepths,
        budget: &mut BindingBudget<'_>,
    ) -> Result<Value, Error> {
        budget.steps += 1;
        if budget.steps > MAX_LINEAR_WORK {
            return Err(Error(
                "parameter binding step limit reached; the prompt is still usable".into(),
            ));
        }
        if depths.pattern > MAX_STRUCTURE_DEPTH {
            return Err(Error(
                "parameter binding depth limit reached; the prompt is still usable".into(),
            ));
        }
        if self.is_bindable_variable(pattern) {
            return Ok(Value::pair(
                Value::pair(pattern.clone(), argument.clone()),
                env,
            ));
        }
        if let Some((target, _)) = optional_spec(pattern)? {
            return self.bind_pattern(&target, argument, env, depths.nested(), budget);
        }
        if let Some((target, predicate)) = typed_spec(pattern)? {
            let accepted = match self.native_typed_predicate(&predicate, argument, &env)? {
                Some(accepted) => accepted,
                None => {
                    let check = Value::list([
                        predicate,
                        Value::list([Value::symbol("quote"), argument.clone()]),
                    ]);
                    !self
                        .eval(&check, &env, depths.eval + 1, budget.fuel)?
                        .is_nil()
                }
            };
            if !accepted {
                return Err(Error("typed parameter rejected its argument".into()));
            }
            return self.bind_pattern(&target, argument, env, depths.nested(), budget);
        }
        let Some((pattern_head, pattern_tail)) = pattern.pair_parts() else {
            return if argument.is_nil() {
                Ok(env)
            } else {
                Err(Error("function received too many arguments".into()))
            };
        };
        match argument {
            Value::Nil => {
                let Some((target, default)) = optional_spec(&pattern_head)? else {
                    return Err(Error("function received too few arguments".into()));
                };
                let value = self.eval(&default, &env, depths.eval + 1, budget.fuel)?;
                let env = self.bind_pattern(&target, &value, env, depths.nested(), budget)?;
                self.bind_pattern(&pattern_tail, &Value::Nil, env, depths.nested(), budget)
            }
            Value::Pair(_) => {
                let (argument_head, argument_tail) = argument.pair_parts().expect("pair matched");
                let env =
                    self.bind_pattern(&pattern_head, &argument_head, env, depths.nested(), budget)?;
                self.bind_pattern(&pattern_tail, &argument_tail, env, depths.nested(), budget)
            }
            _ => Err(Error("cannot destructure an atomic argument".into())),
        }
    }

    fn native_typed_predicate(
        &self,
        predicate: &Value,
        argument: &Value,
        env: &Value,
    ) -> Result<Option<bool>, Error> {
        let Some(native) = &self.native_source_shortcuts else {
            return Ok(None);
        };
        let Some((name, expected, positive)) = predicate.name().and_then(|name| match name {
            "whole" => Some((name, &native.whole, false)),
            "pint" => Some((name, &native.pint, true)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let resolved = self.lookup(name, env)?;
        if !resolved.identical(expected) || !self.native_bindings_match(native) {
            return Ok(None);
        }
        let Some(value) = natural_integer(argument) else {
            return Ok(None);
        };
        Ok(Some(!positive || value > 0))
    }

    fn native_character_number(&self, function: &Value, arguments: &[Value]) -> Option<Value> {
        let native = self.native_source_shortcuts.as_ref()?;
        if !function.identical(&native.charn)
            || arguments.len() != 1
            || !self.native_bindings_match(native)
        {
            return None;
        }
        let Value::Char(character) = &arguments[0] else {
            return None;
        };
        let number = native.character_positions.get(character)?;
        read(&number.to_string()).ok()?.into_iter().next()
    }

    fn native_binary_less(&self, function: &Value, arguments: &[Value]) -> Option<Value> {
        let native = self.native_source_shortcuts.as_ref()?;
        if !function.identical(&native.binary_less)
            || arguments.len() != 2
            || !self.native_bindings_match(native)
        {
            return None;
        }
        match (&arguments[0], &arguments[1]) {
            (Value::Char(left), Value::Char(right)) => {
                let left = native.character_positions.get(left)?;
                let right = native.character_positions.get(right)?;
                Some(if left < right {
                    Value::symbol("t")
                } else {
                    Value::Nil
                })
            }
            (left, right) => {
                source_symbol_less(left.name()?, right.name()?, &native.character_positions)
            }
        }
    }

    fn native_bindings_match(&self, native: &NativeSourceShortcuts) -> bool {
        if !self.dynamic.is_empty() {
            return false;
        }
        self.global_bindings().is_ok_and(|current_bindings| {
            native.bindings.iter().all(|(name, expected)| {
                current_bindings
                    .get(name)
                    .is_some_and(|current| current.identical(expected))
            })
        })
    }
}

fn character_table() -> Value {
    Value::list((u8::MIN..=u8::MAX).map(|byte| {
        let bits = Value::list((0..8).rev().map(|shift| {
            Value::Char(if byte & (1_u8 << shift) == 0 {
                '0'
            } else {
                '1'
            })
        }));
        Value::pair(Value::Char(char::from(byte)), bits)
    }))
}

fn source_symbol_less(
    left: &str,
    right: &str,
    character_positions: &HashMap<char, usize>,
) -> Option<Value> {
    let mut left = left.chars();
    let mut right = right.chars();
    loop {
        match (left.next(), right.next()) {
            (None, None) | (Some(_), None) => return Some(Value::Nil),
            (None, Some(character)) => {
                let tail = std::iter::once(character).chain(right).map(Value::Char);
                return Some(Value::list(tail));
            }
            (Some(left), Some(right)) if left == right => {
                character_positions.get(&left)?;
            }
            (Some(left), Some(right)) => {
                let left = character_positions.get(&left)?;
                let right = character_positions.get(&right)?;
                return Some(if left < right {
                    Value::symbol("t")
                } else {
                    Value::Nil
                });
            }
        }
    }
}

fn binding_pair_ids(bindings: &HashMap<String, Value>) -> HashSet<usize> {
    let mut ids = HashSet::new();
    let mut pending = bindings
        .iter()
        .filter(|(name, _)| name.as_str() != "globe")
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    while let Some(value) = pending.pop() {
        let Value::Pair(pair) = value else {
            continue;
        };
        if !ids.insert(Rc::as_ptr(&pair) as usize) {
            continue;
        }
        let (car, cdr) = pair.borrow().clone();
        pending.push(car);
        pending.push(cdr);
    }
    ids
}

fn pair_id(value: &Value) -> Option<usize> {
    let Value::Pair(pair) = value else {
        return None;
    };
    Some(Rc::as_ptr(pair) as usize)
}

fn macro_expander(value: &Value) -> Option<Value> {
    let literal = value.elements().ok()?;
    match literal.as_slice() {
        [lit, kind, expander] if lit.name() == Some("lit") && kind.name() == Some("mac") => {
            Some(expander.clone())
        }
        _ => None,
    }
}

fn pair_place_location(value: &Value) -> Option<&'static str> {
    let literal = value.elements().ok()?;
    let [lit, kind, primitive] = literal.as_slice() else {
        return None;
    };
    if lit.name() != Some("lit") || kind.name() != Some("prim") {
        return None;
    }
    match primitive.name() {
        Some("car") => Some("a"),
        Some("cdr") => Some("d"),
        _ => None,
    }
}

fn is_literal_kind(value: &Value, expected: &str) -> bool {
    let Some((lit, rest)) = value.pair_parts() else {
        return false;
    };
    let Some((kind, _)) = rest.pair_parts() else {
        return false;
    };
    lit.name() == Some("lit") && kind.name() == Some(expected)
}

fn validated_table_entries(table: &Value) -> Result<(Value, Value), Error> {
    let Some((_, rest)) = table.pair_parts() else {
        return Err(Error("invalid table literal".into()));
    };
    let Some((_, entries)) = rest.pair_parts() else {
        return Err(Error("invalid table literal".into()));
    };
    let mut current = entries.clone();
    let mut visited = Vec::new();
    while let Some((entry, tail)) = bounded_list_pair(&current, &mut visited, "table entries")? {
        if entry.pair_parts().is_none() {
            return Err(Error("invalid table entry".into()));
        }
        current = tail;
    }
    Ok((rest, entries))
}

fn continuation_id(token: &Value) -> Result<u64, Error> {
    token
        .name()
        .and_then(|name| name.strip_prefix("continuation-"))
        .and_then(|id| id.parse().ok())
        .ok_or_else(|| Error("invalid continuation literal".into()))
}

fn is_catchable_runtime_error(error: &Error) -> bool {
    error.0 != ESCAPE_SENTINEL
        && !error
            .0
            .contains("limit reached; the prompt is still usable")
}

fn exact(arguments: &[Value], count: usize, name: &str) -> Result<(), Error> {
    if arguments.len() != count {
        return Err(Error(format!(
            "{name} expects {count} argument(s), got {}",
            arguments.len()
        )));
    }
    Ok(())
}

fn bounded_list_pair(
    current: &Value,
    visited: &mut Vec<Value>,
    context: &str,
) -> Result<Option<(Value, Value)>, Error> {
    if current.is_nil() {
        return Ok(None);
    }
    let Some(parts) = current.pair_parts() else {
        return Err(Error(format!("invalid {context}")));
    };
    if visited.len() >= MAX_LINEAR_WORK {
        return Err(Error(format!(
            "{context} traversal limit reached; the prompt is still usable"
        )));
    }
    if visited.iter().any(|prior| prior.identical(current)) {
        return Err(Error(format!(
            "cyclic {context}; the prompt is still usable"
        )));
    }
    visited.push(current.clone());
    Ok(Some(parts))
}

fn optional_spec(value: &Value) -> Result<Option<(Value, Value)>, Error> {
    let Some((head, tail)) = value.pair_parts() else {
        return Ok(None);
    };
    if head.name() != Some("o") {
        return Ok(None);
    }
    let arguments = tail.elements()?;
    if !(1..=2).contains(&arguments.len()) {
        return Err(Error(format!(
            "optional parameter expects a target and optional default, got {} item(s)",
            arguments.len()
        )));
    }
    Ok(Some((
        arguments[0].clone(),
        arguments.get(1).cloned().unwrap_or(Value::Nil),
    )))
}

fn typed_spec(value: &Value) -> Result<Option<(Value, Value)>, Error> {
    let Some((head, tail)) = value.pair_parts() else {
        return Ok(None);
    };
    if head.name() != Some("t") {
        return Ok(None);
    }
    let arguments = tail.elements()?;
    if arguments.len() != 2 {
        return Err(Error(format!(
            "typed parameter expects a target and predicate, got {} item(s)",
            arguments.len()
        )));
    }
    Ok(Some((arguments[0].clone(), arguments[1].clone())))
}

fn natural_integer(value: &Value) -> Option<usize> {
    let literal = value.elements().ok()?;
    let [lit, kind, real, imaginary] = literal.as_slice() else {
        return None;
    };
    if lit.name() != Some("lit")
        || kind.name() != Some("num")
        || !signed_rational_is_canonical_zero(imaginary)
    {
        return None;
    }
    let real = real.elements().ok()?;
    let [sign, numerator, denominator] = real.as_slice() else {
        return None;
    };
    if unary_count(denominator) != Some(1) {
        return None;
    }
    let numerator = unary_count(numerator)?;
    match sign.name() {
        Some("+") => Some(numerator),
        Some("-") if numerator == 0 => Some(0),
        _ => None,
    }
}

fn signed_rational_is_canonical_zero(value: &Value) -> bool {
    let Ok(parts) = value.elements() else {
        return false;
    };
    let [sign, numerator, denominator] = parts.as_slice() else {
        return false;
    };
    sign.name() == Some("+")
        && unary_count(numerator) == Some(0)
        && unary_count(denominator) == Some(1)
}

fn unary_count(value: &Value) -> Option<usize> {
    let elements = value.elements().ok()?;
    elements
        .iter()
        .all(|element| element.name() == Some("t"))
        .then_some(elements.len())
}

fn primitive_call(primitive: &Value, mut arguments: Vec<Value>) -> Result<Value, Error> {
    let name = primitive
        .name()
        .ok_or_else(|| Error("invalid primitive name".into()))?;
    let count = match name {
        "car" | "cdr" | "type" | "err" | "sym" | "nom" => 1,
        "join" | "id" | "xar" | "xdr" => 2,
        _ => return Err(Error(format!("unsupported primitive: {name}"))),
    };
    if arguments.len() > count {
        return Err(Error(format!("{name} expects at most {count} argument(s)")));
    }
    arguments.resize(count, Value::Nil);
    match name {
        "car" | "cdr" => match &arguments[0] {
            Value::Nil => Ok(Value::Nil),
            Value::Pair(_) => {
                let (head, tail) = arguments[0].pair_parts().expect("pair matched");
                Ok(if name == "car" { head } else { tail })
            }
            _ => Err(Error(format!("{name} requires a pair or nil"))),
        },
        "join" => Ok(Value::pair(arguments[0].clone(), arguments[1].clone())),
        "xar" => arguments[0].replace_car(arguments[1].clone()),
        "xdr" => arguments[0].replace_cdr(arguments[1].clone()),
        "id" => Ok(if arguments[0].identical(&arguments[1]) {
            Value::symbol("t")
        } else {
            Value::Nil
        }),
        "type" => Ok(Value::symbol(match &arguments[0] {
            Value::Pair(_) => "pair",
            Value::Char(_) => "char",
            Value::Nil | Value::Symbol(_) => "symbol",
        })),
        "err" => Err(Error(format!("Bel error: {}", arguments[0]))),
        "nom" => {
            let name = arguments[0]
                .name()
                .ok_or_else(|| Error("nom requires a symbol".into()))?;
            Ok(Value::list(name.chars().map(Value::Char)))
        }
        "sym" => {
            let name = arguments[0]
                .elements()?
                .into_iter()
                .map(|value| match value {
                    Value::Char(character) => Ok(character),
                    _ => Err(Error("sym requires a proper list of characters".into())),
                })
                .collect::<Result<String, _>>()?;
            Ok(Value::symbol(&name))
        }
        _ => Err(Error(format!("unsupported primitive: {name}"))),
    }
}
