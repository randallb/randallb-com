use crate::{
    Value,
    limits::{
        MAX_ABBREVIATION_PARTS, MAX_LINEAR_WORK, MAX_NUMERIC_COMPONENT, MAX_READER_NODES,
        MAX_SOURCE_BYTES, MAX_STRUCTURE_DEPTH,
    },
};
const STRING_TOKEN_PREFIX: &str = "\0string:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub message: String,
    pub incomplete: bool,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ReadError {}

struct Reader {
    tokens: Vec<String>,
    position: usize,
    nodes: usize,
}

fn failure(message: &str, incomplete: bool) -> ReadError {
    ReadError {
        message: message.into(),
        incomplete,
    }
}

/// Read all forms, distinguishing incomplete input from malformed input.
pub fn read(source: &str) -> Result<Vec<Value>, ReadError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(failure(
            "source exceeds the experiment's 64 KiB input limit",
            false,
        ));
    }
    let mut reader = Reader {
        tokens: tokenize(source)?,
        position: 0,
        nodes: 0,
    };
    let mut forms = Vec::new();
    while reader.position < reader.tokens.len() {
        forms.push(reader.form(0)?);
    }
    Ok(forms)
}

fn tokenize(source: &str) -> Result<Vec<String>, ReadError> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut comment = false;
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        if comment {
            comment = character != '\n';
            continue;
        }
        if character == '"' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            let mut value = String::new();
            let mut closed = false;
            while let Some(next) = characters.next() {
                match next {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => {
                        let Some(escaped) = characters.next() else {
                            return Err(failure("string ends after an escape", true));
                        };
                        value.push(escaped);
                    }
                    character if character.is_control() && character != '\n' => {
                        return Err(failure("string contains a control character", false));
                    }
                    character => value.push(character),
                }
            }
            if !closed {
                return Err(failure(
                    "unclosed string; enter more input or :cancel",
                    true,
                ));
            }
            tokens.push(format!("{STRING_TOKEN_PREFIX}{value}"));
        } else if character == '\\' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            let Some(first) = characters.next() else {
                return Err(failure("character literal is missing a character", true));
            };
            let mut name = first.to_string();
            if first.is_alphabetic() {
                while characters
                    .peek()
                    .is_some_and(|next| !next.is_whitespace() && !"()[]'`,;\"\\|#".contains(*next))
                {
                    name.push(characters.next().expect("peeked"));
                }
            }
            tokens.push(format!("\\{name}"));
        } else if character == ';' || character.is_whitespace() || "()[]'`,".contains(character) {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            match character {
                ';' => comment = true,
                ',' if characters.peek() == Some(&'@') => {
                    characters.next();
                    tokens.push(",@".into());
                }
                '(' | ')' | '[' | ']' | '\'' | '`' | ',' => tokens.push(character.to_string()),
                _ => {}
            }
        } else if character == '#' || character.is_control() {
            return Err(failure(
                "unsupported reader syntax (only symbols, lists, dots, quote, and backquote forms are supported)",
                false,
            ));
        } else {
            word.push(character);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    Ok(tokens)
}

impl Reader {
    fn next(&mut self) -> Result<String, ReadError> {
        let token =
            self.tokens.get(self.position).cloned().ok_or_else(|| {
                failure("incomplete expression; enter more input or :cancel", true)
            })?;
        self.position += 1;
        Ok(token)
    }

    fn form(&mut self, depth: usize) -> Result<Value, ReadError> {
        self.nodes += 1;
        if depth > MAX_STRUCTURE_DEPTH || self.nodes > MAX_READER_NODES {
            return Err(failure(
                "expression exceeds the experiment's reader limit",
                false,
            ));
        }
        match self.next()?.as_str() {
            "(" => self.list(depth + 1, ")"),
            "[" => self.bracket(depth + 1),
            ")" => Err(failure("unexpected closing parenthesis", false)),
            "]" => Err(failure("unexpected closing bracket", false)),
            "." => Err(failure(
                "dot is only supported between a pair's head and tail",
                false,
            )),
            "'" => Ok(Value::list([Value::symbol("quote"), self.form(depth + 1)?])),
            "`" => Ok(Value::list([
                Value::symbol("bquote"),
                self.form(depth + 1)?,
            ])),
            "," => Ok(Value::list([Value::symbol("comma"), self.form(depth + 1)?])),
            ",@" => Ok(Value::list([
                Value::symbol("comma-at"),
                self.form(depth + 1)?,
            ])),
            token => token
                .strip_prefix(STRING_TOKEN_PREFIX)
                .map(|string| {
                    let characters = string.chars().count();
                    if characters > MAX_LINEAR_WORK {
                        return Err(failure(
                            "string literal exceeds the experiment's reader allocation limit",
                            false,
                        ));
                    }
                    charge_nodes(
                        &mut self.nodes,
                        characters,
                        "string literal exceeds the experiment's reader allocation limit",
                    )?;
                    Ok(Value::list(string.chars().map(Value::Char)))
                })
                .unwrap_or_else(|| abbreviation(token, &mut self.nodes)),
        }
    }

    fn bracket(&mut self, depth: usize) -> Result<Value, ReadError> {
        let body = self.list(depth, "]")?;
        Ok(Value::list([
            Value::symbol("fn"),
            Value::list([Value::symbol("_")]),
            body,
        ]))
    }

    fn list(&mut self, depth: usize, terminator: &str) -> Result<Value, ReadError> {
        let mut heads = Vec::new();
        loop {
            match self.tokens.get(self.position).map(String::as_str) {
                Some(token) if token == terminator => {
                    self.position += 1;
                    return Ok(Value::list(heads));
                }
                Some(".") => return self.dotted_tail(heads, depth, terminator),
                None => {
                    let kind = if terminator == "]" { "bracket" } else { "list" };
                    return Err(failure(
                        &format!("unclosed {kind}; enter more input or :cancel"),
                        true,
                    ));
                }
                _ => heads.push(self.form(depth)?),
            }
        }
    }

    fn dotted_tail(
        &mut self,
        heads: Vec<Value>,
        depth: usize,
        terminator: &str,
    ) -> Result<Value, ReadError> {
        if heads.is_empty() {
            return Err(failure("a dotted pair needs a head before the dot", false));
        }
        self.position += 1;
        let tail = self.form(depth)?;
        if self.next()? != terminator {
            return Err(failure(
                "a dotted pair needs exactly one tail expression",
                false,
            ));
        }
        Ok(heads
            .into_iter()
            .rev()
            .fold(tail, |tail, head| Value::pair(head, tail)))
    }
}

fn abbreviation(token: &str, nodes: &mut usize) -> Result<Value, ReadError> {
    abbreviation_at(token, 0, nodes)
}

fn charge_nodes(nodes: &mut usize, additional: usize, message: &str) -> Result<(), ReadError> {
    if additional > MAX_READER_NODES.saturating_sub(*nodes) {
        return Err(failure(message, false));
    }
    *nodes += additional;
    Ok(())
}

fn abbreviation_at(token: &str, depth: usize, nodes: &mut usize) -> Result<Value, ReadError> {
    if depth > MAX_STRUCTURE_DEPTH
        || token
            .bytes()
            .filter(|character| matches!(character, b'.' | b'!' | b':'))
            .count()
            > MAX_ABBREVIATION_PARTS
    {
        return Err(failure(
            "reader abbreviation exceeds the experiment's expansion limit",
            false,
        ));
    }
    if token.starts_with('\\') {
        return complement(token, nodes);
    }
    let unsigned = token.strip_prefix(['+', '-']).unwrap_or(token);
    if let Some(number) = number(token, nodes) {
        return number;
    }
    if unsigned.starts_with(|character: char| character.is_ascii_digit())
        && token.contains(['.', '!'])
    {
        return intrasymbol(token, depth + 1, nodes);
    }
    if token.contains('|') {
        let mut parts = token.split('|');
        let parameter = parts.next().expect("split always has one item");
        let predicate = parts.next().expect("contains a separator");
        if parts.next().is_some() || parameter.is_empty() || predicate.is_empty() {
            return Err(failure("typed parameter shorthand is malformed", false));
        }
        return Ok(Value::list([
            Value::symbol("t"),
            abbreviation_at(parameter, depth + 1, nodes)?,
            abbreviation_at(predicate, depth + 1, nodes)?,
        ]));
    }
    if token.contains(['.', '!']) {
        return intrasymbol(token, depth + 1, nodes);
    }
    if token.contains(':') {
        let parts = token
            .split(':')
            .map(|part| complement(part, nodes))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.iter().any(|part| part.name() == Some("")) {
            return Err(failure("composition contains an empty term", false));
        }
        return Ok(Value::list(
            std::iter::once(Value::symbol("compose")).chain(parts),
        ));
    }
    complement(token, nodes)
}

fn intrasymbol(token: &str, depth: usize, nodes: &mut usize) -> Result<Value, ReadError> {
    let mut characters = token.chars().peekable();
    let mut terms = Vec::new();
    if characters
        .peek()
        .is_some_and(|character| ".!".contains(*character))
    {
        terms.push(Value::symbol("upon"));
    } else {
        let head = take_intrasymbol_term(&mut characters);
        terms.push(abbreviation_at(&head, depth, nodes)?);
    }
    while let Some(operator) = characters.next() {
        if !".!".contains(operator) {
            return Err(failure("malformed intrasymbol abbreviation", false));
        }
        if characters.peek().is_none() {
            return Err(failure("intrasymbol abbreviation has no final term", false));
        }
        if characters
            .peek()
            .is_some_and(|character| ".!".contains(*character))
        {
            return Err(failure(
                "intrasymbol abbreviation has adjacent operators",
                false,
            ));
        }
        let term = abbreviation_at(&take_intrasymbol_term(&mut characters), depth, nodes)?;
        terms.push(if operator == '!' {
            Value::list([Value::symbol("quote"), term])
        } else {
            term
        });
    }
    Ok(Value::list(terms))
}

fn take_intrasymbol_term(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut term = String::new();
    while characters
        .peek()
        .is_some_and(|character| !".!".contains(*character))
    {
        term.push(characters.next().expect("peeked"));
    }
    term
}

fn complement(token: &str, nodes: &mut usize) -> Result<Value, ReadError> {
    let complements = token
        .bytes()
        .take_while(|character| *character == b'~')
        .count();
    if complements > MAX_STRUCTURE_DEPTH {
        return Err(failure(
            "reader abbreviation exceeds the experiment's expansion limit",
            false,
        ));
    }
    let rest = &token[complements..];
    let (mut value, wrappers) = if rest.is_empty() && complements > 0 {
        (Value::symbol("no"), complements.saturating_sub(1))
    } else {
        let value = if let Some(number) = number(rest, nodes) {
            number?
        } else if let Some(character) = rest.strip_prefix('\\') {
            read_character(character)?
        } else {
            symbol(rest)?
        };
        (value, complements)
    };
    for _ in 0..wrappers {
        value = Value::list([Value::symbol("compose"), Value::symbol("no"), value]);
    }
    Ok(value)
}

fn read_character(name: &str) -> Result<Value, ReadError> {
    let character = match name {
        "bel" => '\u{7}',
        "tab" => '\t',
        "lf" => '\n',
        "cr" => '\r',
        "sp" => ' ',
        _ => {
            let mut characters = name.chars();
            let Some(character) = characters.next() else {
                return Err(failure("character literal is missing a character", false));
            };
            if characters.next().is_some() {
                return Err(failure("unknown named character", false));
            }
            character
        }
    };
    Ok(Value::Char(character))
}

fn number(token: &str, nodes: &mut usize) -> Option<Result<Value, ReadError>> {
    let (sign, unsigned) = match token.as_bytes().first() {
        Some(b'+') => ("+", &token[1..]),
        Some(b'-') => ("-", &token[1..]),
        _ => ("+", token),
    };
    if !unsigned.starts_with(|character: char| character.is_ascii_digit() || character == '.') {
        return None;
    }
    if unsigned.contains('!')
        || (unsigned.contains('.')
            && unsigned
                .chars()
                .any(|character| !character.is_ascii_digit() && !matches!(character, '.' | '/')))
    {
        return None;
    }
    let mut pieces = unsigned.split('/');
    let numerator_source = pieces.next().expect("split always has one item");
    let denominator_source = pieces.next();
    if pieces.next().is_some() || numerator_source.is_empty() || denominator_source == Some("") {
        return Some(Err(failure("malformed numeric literal", false)));
    }
    let (left_numerator, left_denominator) = match decimal_component(numerator_source) {
        Ok(Some(value)) => value,
        Ok(None) if unsigned.contains('.') => return None,
        Ok(None) => return Some(Err(failure("malformed numeric literal", false))),
        Err(error) => return Some(Err(error)),
    };
    let (right_numerator, right_denominator) = match denominator_source {
        Some(source) => match decimal_component(source) {
            Ok(Some((0, _))) => {
                return Some(Err(failure("numeric denominator cannot be zero", false)));
            }
            Ok(Some(value)) => value,
            Ok(None) if unsigned.contains('.') => return None,
            Ok(None) => return Some(Err(failure("malformed numeric literal", false))),
            Err(error) => return Some(Err(error)),
        },
        None => (1, 1),
    };
    let Some(mut numerator) = left_numerator.checked_mul(right_denominator) else {
        return Some(Err(failure(
            "numeric literal exceeds the reader limit",
            false,
        )));
    };
    let Some(mut denominator) = left_denominator.checked_mul(right_numerator) else {
        return Some(Err(failure(
            "numeric literal exceeds the reader limit",
            false,
        )));
    };
    let divisor = greatest_common_divisor(numerator, denominator);
    numerator /= divisor;
    denominator /= divisor;
    if numerator > MAX_NUMERIC_COMPONENT || denominator > MAX_NUMERIC_COMPONENT {
        return Some(Err(failure(
            "numeric literal exceeds the reader limit",
            false,
        )));
    }
    let expanded_pairs = 11 + numerator + denominator;
    if let Err(error) = charge_nodes(
        nodes,
        expanded_pairs,
        "numeric expansion exceeds the experiment's reader allocation limit",
    ) {
        return Some(Err(error));
    }
    let one = unary(1);
    let zero = Value::list([Value::symbol("+"), Value::Nil, one.clone()]);
    let real = Value::list([
        Value::symbol(if numerator == 0 { "+" } else { sign }),
        unary(numerator),
        unary(denominator),
    ]);
    Some(Ok(Value::list([
        Value::symbol("lit"),
        Value::symbol("num"),
        real,
        zero,
    ])))
}

fn decimal_component(source: &str) -> Result<Option<(usize, usize)>, ReadError> {
    let mut parts = source.split('.');
    let whole = parts.next().expect("split always has one item");
    let fraction = parts.next();
    if parts.next().is_some()
        || (whole.is_empty() && fraction.is_none_or(str::is_empty))
        || !whole.chars().all(|character| character.is_ascii_digit())
        || fraction
            .is_some_and(|digits| !digits.chars().all(|character| character.is_ascii_digit()))
    {
        return Ok(None);
    }
    let fractional_digits = fraction.unwrap_or("");
    let power = u32::try_from(fractional_digits.len())
        .ok()
        .and_then(|power| 10_usize.checked_pow(power))
        .ok_or_else(|| failure("numeric literal exceeds the reader limit", false))?;
    let whole = decimal_digits(whole)?;
    let fraction = decimal_digits(fractional_digits)?;
    let numerator = whole
        .checked_mul(power)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| failure("numeric literal exceeds the reader limit", false))?;
    Ok(Some((numerator, power)))
}

fn decimal_digits(source: &str) -> Result<usize, ReadError> {
    source.bytes().try_fold(0_usize, |value, digit| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(usize::from(digit - b'0')))
            .ok_or_else(|| failure("numeric literal exceeds the reader limit", false))
    })
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left.max(1)
}

fn unary(value: usize) -> Value {
    Value::list((0..value).map(|_| Value::symbol("t")))
}

fn symbol(token: &str) -> Result<Value, ReadError> {
    if token.is_empty()
        || token.chars().any(|character| character == '.')
        || token
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    {
        return Err(failure(
            "unsupported number or malformed reader abbreviation",
            false,
        ));
    }
    Ok(Value::symbol(token))
}
