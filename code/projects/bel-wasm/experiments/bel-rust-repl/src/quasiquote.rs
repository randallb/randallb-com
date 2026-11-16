use crate::{Error, Interpreter, Value, limits::MAX_STRUCTURE_DEPTH};

#[derive(Clone, Copy)]
struct ExpansionPosition {
    level: usize,
    eval_depth: usize,
    structural_depth: usize,
}

struct Expansion {
    value: Value,
    changed: bool,
}

impl Expansion {
    fn unchanged(value: &Value) -> Self {
        Self {
            value: value.clone(),
            changed: false,
        }
    }
}

impl ExpansionPosition {
    fn nested(self, level: usize) -> Self {
        Self {
            level,
            eval_depth: self.eval_depth + 1,
            structural_depth: self.structural_depth + 1,
        }
    }
}

pub(crate) fn expand(
    interpreter: &mut Interpreter,
    value: &Value,
    env: &Value,
    level: usize,
    depth: usize,
    fuel: &mut usize,
) -> Result<Value, Error> {
    Ok(expand_inner(
        interpreter,
        value,
        env,
        ExpansionPosition {
            level,
            eval_depth: depth,
            structural_depth: 0,
        },
        fuel,
        &mut Vec::new(),
    )?
    .value)
}

fn expand_inner(
    interpreter: &mut Interpreter,
    value: &Value,
    env: &Value,
    position: ExpansionPosition,
    fuel: &mut usize,
    active_pairs: &mut Vec<Value>,
) -> Result<Expansion, Error> {
    if position.eval_depth > interpreter.eval_depth_limit()
        || position.structural_depth > MAX_STRUCTURE_DEPTH
    {
        return Err(Error(
            "quasiquote expansion depth limit reached; the prompt is still usable".into(),
        ));
    }
    if *fuel == 0 {
        return Err(Error(
            "evaluation step limit reached; the prompt is still usable".into(),
        ));
    }
    *fuel -= 1;
    if let Some(inner) = tagged_argument(value, "bquote")? {
        let expanded = expand_inner(
            interpreter,
            &inner,
            env,
            position.nested(position.level + 1),
            fuel,
            active_pairs,
        )?;
        return if expanded.changed {
            Ok(Expansion {
                value: Value::list([Value::symbol("bquote"), expanded.value]),
                changed: true,
            })
        } else {
            Ok(Expansion::unchanged(value))
        };
    }
    if let Some(inner) = tagged_argument(value, "comma")? {
        return if position.level == 1 {
            Ok(Expansion {
                value: interpreter.eval(&inner, env, position.eval_depth + 1, fuel)?,
                changed: true,
            })
        } else if position.level == 2
            && let Some(splice) = tagged_argument(&inner, "comma-at")?
        {
            Ok(Expansion {
                value: Value::pair(
                    Value::symbol("comma"),
                    interpreter.eval(&splice, env, position.eval_depth + 1, fuel)?,
                ),
                changed: true,
            })
        } else {
            let expanded = expand_inner(
                interpreter,
                &inner,
                env,
                position.nested(position.level - 1),
                fuel,
                active_pairs,
            )?;
            if expanded.changed {
                Ok(Expansion {
                    value: Value::list([Value::symbol("comma"), expanded.value]),
                    changed: true,
                })
            } else {
                Ok(Expansion::unchanged(value))
            }
        };
    }
    if let Some(inner) = tagged_argument(value, "comma-at")? {
        return if position.level == 1 {
            Err(Error(
                "comma-at is only valid within a backquoted list".into(),
            ))
        } else if position.level == 2
            && let Some(splice) = tagged_argument(&inner, "comma-at")?
        {
            Ok(Expansion {
                value: Value::pair(
                    Value::symbol("comma-at"),
                    interpreter.eval(&splice, env, position.eval_depth + 1, fuel)?,
                ),
                changed: true,
            })
        } else {
            let expanded = expand_inner(
                interpreter,
                &inner,
                env,
                position.nested(position.level - 1),
                fuel,
                active_pairs,
            )?;
            if expanded.changed {
                Ok(Expansion {
                    value: Value::list([Value::symbol("comma-at"), expanded.value]),
                    changed: true,
                })
            } else {
                Ok(Expansion::unchanged(value))
            }
        };
    }
    if value.pair_parts().is_none() {
        return Ok(Expansion::unchanged(value));
    }
    if active_pairs.iter().any(|pair| pair.identical(value)) {
        return Err(Error(
            "quasiquote expansion depth limit reached; the prompt is still usable".into(),
        ));
    }
    active_pairs.push(value.clone());
    let result = expand_pair(interpreter, value, env, position, fuel, active_pairs);
    active_pairs.pop();
    result
}

fn expand_pair(
    interpreter: &mut Interpreter,
    original: &Value,
    env: &Value,
    position: ExpansionPosition,
    fuel: &mut usize,
    active_pairs: &mut Vec<Value>,
) -> Result<Expansion, Error> {
    let (head, tail_value) = original.pair_parts().expect("caller checked for a pair");
    let ExpansionPosition {
        level, eval_depth, ..
    } = position;
    if level == 1
        && let Some(head_splice) = tagged_argument(&head, "comma-at")?
        && let Some(tail_splice) = tagged_argument(&tail_value, "comma-at")?
    {
        let heads = interpreter.eval(&head_splice, env, eval_depth + 1, fuel)?;
        validate_head_splice(&heads)?;
        let tails = interpreter
            .eval(&tail_splice, env, eval_depth + 1, fuel)?
            .elements()
            .map_err(|_| Error("comma-at cdr requires a one-item proper list value".into()))?;
        let [tail] = tails.as_slice() else {
            return Err(Error(
                "comma-at cdr requires a one-item proper list value".into(),
            ));
        };
        return Ok(Expansion {
            value: heads
                .elements()
                .map_err(|_| Error("comma-at requires a proper list value".into()))?
                .into_iter()
                .rev()
                .fold(tail.clone(), |tail, value| Value::pair(value, tail)),
            changed: true,
        });
    }
    if level == 1
        && let Some(splice) = tagged_argument(&head, "comma-at")?
    {
        let values = interpreter.eval(&splice, env, eval_depth + 1, fuel)?;
        validate_head_splice(&values)?;
        let tail = expand_inner(
            interpreter,
            &tail_value,
            env,
            position.nested(level),
            fuel,
            active_pairs,
        )?
        .value;
        return Ok(Expansion {
            value: values
                .elements()
                .map_err(|_| Error("comma-at requires a proper list value".into()))?
                .into_iter()
                .rev()
                .fold(tail, |tail, value| Value::pair(value, tail)),
            changed: true,
        });
    }
    if level == 1
        && let Some(splice) = tagged_argument(&tail_value, "comma-at")?
    {
        let head = expand_inner(
            interpreter,
            &head,
            env,
            position.nested(level),
            fuel,
            active_pairs,
        )?
        .value;
        let values = interpreter
            .eval(&splice, env, eval_depth + 1, fuel)?
            .elements()
            .map_err(|_| Error("comma-at cdr requires a one-item proper list value".into()))?;
        let [tail] = values.as_slice() else {
            return Err(Error(
                "comma-at cdr requires a one-item proper list value".into(),
            ));
        };
        return Ok(Expansion {
            value: Value::pair(head, tail.clone()),
            changed: true,
        });
    }
    let head = expand_inner(
        interpreter,
        &head,
        env,
        position.nested(level),
        fuel,
        active_pairs,
    )?;
    let tail = expand_inner(
        interpreter,
        &tail_value,
        env,
        position.nested(level),
        fuel,
        active_pairs,
    )?;
    if head.changed || tail.changed {
        Ok(Expansion {
            value: Value::pair(head.value, tail.value),
            changed: true,
        })
    } else {
        Ok(Expansion::unchanged(original))
    }
}

fn validate_head_splice(value: &Value) -> Result<(), Error> {
    if value.is_nil() || value.pair_parts().is_some() {
        Ok(())
    } else {
        Err(Error("comma-at requires a proper list value".into()))
    }
}

fn tagged_argument(value: &Value, tag: &str) -> Result<Option<Value>, Error> {
    let Some((head, tail)) = value.pair_parts() else {
        return Ok(None);
    };
    if head.name() != Some(tag) {
        return Ok(None);
    }
    let arguments = tail.elements()?;
    if arguments.len() != 1 {
        return Err(Error(format!(
            "{tag} expects 1 argument, got {}",
            arguments.len()
        )));
    }
    Ok(arguments.into_iter().next())
}
