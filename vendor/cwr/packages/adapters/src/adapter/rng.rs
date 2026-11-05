const RNG_ROLL_ACTION: &str = "roll--dice";
const RESPOND_TO_MESSAGE_ACTION: &str = crate::action_contract::RESPOND_TO_MESSAGE_ACTION_NAME;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollDiceInput {
    #[serde(default = "default_roll_count")]
    count: u64,
    #[serde(default = "default_die_sides")]
    sides: u64,
    #[serde(default)]
    modifier: i64,
}
#[derive(Debug)]
struct DiceExpression {
    count: u64,
    sides: u64,
    modifier: i64,
}
pub fn run_rng_adapter(mut input: impl BufRead, mut output: impl Write) -> CwrResult<()> {
    let message: AdapterHostMessage = read_adapter_message(&mut input)?
        .ok_or_else(|| protocol_error("RNG adapter received no invocation"))?;
    let (protocol, invocation) = match message {
        AdapterHostMessage::Invoke {
            protocol,
            invocation,
        } => (protocol, invocation),
        AdapterHostMessage::CallResult { .. } => {
            return protocol_failure("RNG adapter received a call result before invocation")
        }
        AdapterHostMessage::ActivityResult { .. } => {
            return protocol_failure("RNG adapter received an activity result before invocation")
        }
    };
    if protocol != PROCESS_ADAPTER_PROTOCOL_REF {
        return protocol_failure(format!(
            "RNG adapter supports {PROCESS_ADAPTER_PROTOCOL_REF}, not {protocol}"
        ));
    }
    validate_adapter_invocation(&invocation)?;
    write_adapter_message(
        &mut output,
        &AdapterEvent::Progress {
                progress: None,
                published: false,
            invocation_ref: invocation.invocation_ref.clone(),
            sequence: 1,
            status: 102,
            summary: "Rolling dice.".into(),
        },
    )?;
    let result = match invocation.action_name.as_str() {
        RNG_ROLL_ACTION => {
            let requested: RollDiceInput = serde_json::from_value(invocation.input.clone())
                .map_err(|error| {
                    protocol_error(format!("RNG roll input is invalid: {error}"))
                })?;
            let rolled = roll_dice(DiceExpression {
                count: requested.count,
                sides: requested.sides,
                modifier: requested.modifier,
            })?;
            let text = roll_text(&rolled);
            json!({
                "notation": notation(rolled.expression.count, rolled.expression.sides, rolled.expression.modifier),
                "rolls": rolled.rolls,
                "modifier": rolled.expression.modifier,
                "total": rolled.total,
                "text": text,
            })
        }
        RESPOND_TO_MESSAGE_ACTION => respond_to_message(&invocation)?,
        action => {
            return write_adapter_message(
                &mut output,
                &AdapterEvent::Result {
                    invocation_ref: invocation.invocation_ref,
                    sequence: 2,
                    status: 422,
                    summary: format!("RNG does not support action {action}."),
                    result: None,
                    error: Some(AdapterError {
                        code: "unsupported_action".into(),
                        message: Some(format!("RNG does not support action {action}")),
                        details: None,
                    }),
                    continuation: invocation.continuation,
                },
            )
        }
    };
    write_adapter_message(
        &mut output,
        &AdapterEvent::Result {
            invocation_ref: invocation.invocation_ref,
            sequence: 2,
            status: 200,
            summary: "Rolled dice.".into(),
            result: Some(result),
            error: None,
            continuation: invocation.continuation,
        },
    )
}
fn respond_to_message(invocation: &AdapterInvocation) -> CwrResult<Value> {
    let source_message_ref = invocation
        .input
        .get("source_message_ref")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("RNG message invocation has no source_message_ref"))?;
    let source_text = invocation
        .thread
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages.iter().find(|message| {
                message.get("message_ref").and_then(Value::as_str) == Some(source_message_ref)
            })
        })
        .and_then(|message| message.get("text"))
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error("RNG source message is missing from the thread"))?;
    let Some(expression) = parse_dice_expression(source_text) else {
        return Ok(json!({"text": "Tell me what to roll using dice notation, like `d20`, `2d6`, or `2d6+3`."}));
    };
    let rolled = match roll_dice(expression) {
        Ok(rolled) => rolled,
        Err(_) => {
            return Ok(json!({"text": "I can roll 1–100 dice with 2–1,000,000 sides each."}));
        }
    };
    Ok(json!({"text": roll_text(&rolled)}))
}
fn roll_text(rolled: &DiceRoll) -> String {
    let notation = notation(
        rolled.expression.count,
        rolled.expression.sides,
        rolled.expression.modifier,
    );
    let rolls = rolled
        .rolls
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let modifier = match rolled.expression.modifier {
        0 => String::new(),
        value if value > 0 => format!(" + {value}"),
        value => format!(" - {}", value.unsigned_abs()),
    };
    format!(
        "🎲 `{notation}` → [{rolls}]{modifier} = *{}*",
        rolled.total
    )
}
struct DiceRoll {
    expression: DiceExpression,
    rolls: Vec<u64>,
    total: i64,
}
fn roll_dice(expression: DiceExpression) -> CwrResult<DiceRoll> {
    if !(1..=100).contains(&expression.count) {
        return protocol_failure("RNG roll count must be between 1 and 100");
    }
    if !(2..=1_000_000).contains(&expression.sides) {
        return protocol_failure("RNG die sides must be between 2 and 1,000,000");
    }
    let mut random = std::fs::File::open("/dev/urandom")?;
    let mut rolls = Vec::with_capacity(expression.count as usize);
    for _ in 0..expression.count {
        rolls.push(uniform_below(&mut random, expression.sides)? + 1);
    }
    let rolled_total = rolls.iter().try_fold(0_i64, |total, value| {
        total
            .checked_add(*value as i64)
            .ok_or_else(|| protocol_error("RNG roll total overflowed"))
    })?;
    let total = rolled_total
        .checked_add(expression.modifier)
        .ok_or_else(|| protocol_error("RNG modified total overflowed"))?;
    Ok(DiceRoll {
        expression,
        rolls,
        total,
    })
}
fn uniform_below(random: &mut impl std::io::Read, upper: u64) -> CwrResult<u64> {
    let threshold = upper.wrapping_neg() % upper;
    loop {
        let mut bytes = [0_u8; 8];
        random.read_exact(&mut bytes)?;
        let value = u64::from_le_bytes(bytes);
        if value >= threshold {
            return Ok(value % upper);
        }
    }
}
fn parse_dice_expression(text: &str) -> Option<DiceExpression> {
    let bytes = text.as_bytes();
    for die_index in 0..bytes.len() {
        if !matches!(bytes[die_index], b'd' | b'D') {
            continue;
        }
        let mut count_start = die_index;
        while count_start > 0 && bytes[count_start - 1].is_ascii_digit() {
            count_start -= 1;
        }
        if count_start > 0 && is_dice_token_character(bytes[count_start - 1]) {
            continue;
        }
        let mut sides_end = die_index + 1;
        while sides_end < bytes.len() && bytes[sides_end].is_ascii_digit() {
            sides_end += 1;
        }
        if sides_end == die_index + 1 {
            continue;
        }
        let count = if count_start == die_index {
            1
        } else {
            text[count_start..die_index].parse().ok()?
        };
        let sides = text[die_index + 1..sides_end].parse().ok()?;
        let mut modifier = 0;
        let mut expression_end = sides_end;
        if sides_end < bytes.len() && matches!(bytes[sides_end], b'+' | b'-') {
            let sign = if bytes[sides_end] == b'-' { -1 } else { 1 };
            let modifier_start = sides_end + 1;
            let mut modifier_end = modifier_start;
            while modifier_end < bytes.len() && bytes[modifier_end].is_ascii_digit() {
                modifier_end += 1;
            }
            if modifier_end == modifier_start {
                return None;
            }
            modifier = text[modifier_start..modifier_end]
                .parse::<i64>()
                .ok()?
                .checked_mul(sign)?;
            expression_end = modifier_end;
        }
        if expression_end < bytes.len() && is_dice_token_character(bytes[expression_end]) {
            continue;
        }
        return Some(DiceExpression {
            count,
            sides,
            modifier,
        });
    }
    None
}
fn is_dice_token_character(value: u8) -> bool {
    value.is_ascii_alphanumeric() || value == b'_'
}
fn notation(count: u64, sides: u64, modifier: i64) -> String {
    match modifier {
        0 => format!("{count}d{sides}"),
        value if value > 0 => format!("{count}d{sides}+{value}"),
        value => format!("{count}d{sides}-{}", value.unsigned_abs()),
    }
}
fn default_roll_count() -> u64 {
    1
}
fn default_die_sides() -> u64 {
    20
}
#[cfg(test)]
mod rng_tests {
    use super::*;
    #[test]
    fn finds_common_dice_notation_in_a_message() {
        let expression = parse_dice_expression("please roll 2d6+3 for stealth").unwrap();
        assert_eq!(expression.count, 2);
        assert_eq!(expression.sides, 6);
        assert_eq!(expression.modifier, 3);
        let expression = parse_dice_expression("d20").unwrap();
        assert_eq!(expression.count, 1);
        assert_eq!(expression.sides, 20);
        assert_eq!(expression.modifier, 0);
    }
    #[test]
    fn rejects_incomplete_dice_modifiers() {
        assert!(parse_dice_expression("d20+").is_none());
        assert!(parse_dice_expression("2d6-").is_none());
    }
    #[test]
    fn requires_boundaries_around_dice_notation() {
        assert!(parse_dice_expression("we updated20 yesterday").is_none());
        assert!(parse_dice_expression("I had COVID19").is_none());
        assert!(parse_dice_expression("abc2d6").is_none());
        assert!(parse_dice_expression("2d6foo").is_none());
        assert_eq!(
            parse_dice_expression("roll (2d6), please")
                .expect("bounded dice notation")
                .sides,
            6
        );
    }
}
