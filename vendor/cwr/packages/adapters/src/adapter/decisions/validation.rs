use serde_json::Value;
fn probability(v: &Value) -> bool {
    v.as_f64().is_some_and(|n| (0.0..=1.0).contains(&n))
}

// OpenRouter rounds probabilities and scores independently to two decimals.
// Check whether some normalized distribution within those rounding intervals
// can produce the reported score, rather than treating rounded values as exact.
fn rounded_expectation_bounds(probabilities: &[f64]) -> Option<(f64, f64)> {
    const HALF_UNIT: f64 = 0.005;
    const EPSILON: f64 = 1e-9;
    let bounds: Vec<_> = probabilities
        .iter()
        .map(|p| ((p - HALF_UNIT).max(0.0), (p + HALF_UNIT).min(1.0)))
        .collect();
    let lower_sum: f64 = bounds.iter().map(|(low, _)| low).sum();
    let upper_sum: f64 = bounds.iter().map(|(_, high)| high).sum();
    if lower_sum > 1.0 + EPSILON || upper_sum < 1.0 - EPSILON {
        return None;
    }
    let extreme = |reverse: bool| {
        let mut remaining = (1.0 - lower_sum).max(0.0);
        let mut expectation: f64 = bounds
            .iter()
            .enumerate()
            .map(|(i, (low, _))| i as f64 * low)
            .sum();
        for offset in 0..bounds.len() {
            let i = if reverse {
                bounds.len() - 1 - offset
            } else {
                offset
            };
            let extra = remaining.min(bounds[i].1 - bounds[i].0);
            expectation += i as f64 * extra;
            remaining -= extra;
        }
        expectation
    };
    Some((extreme(false), extreme(true)))
}
pub(super) fn questions(value: &Value) -> Result<(), ()> {
    let qs = value.as_object().ok_or(())?;
    if qs.is_empty() || qs.len() > 200 {
        return Err(());
    }
    for (name, q) in qs {
        if name.trim().is_empty()
            || q["instructions"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
        {
            return Err(());
        }
        match q["type"].as_str() {
            Some("noul") => {
                if let Some(c) = q.get("criteria") {
                    let c = c.as_object().ok_or(())?;
                    if c.len() != 2 || !c.contains_key("true") || !c.contains_key("false") {
                        return Err(());
                    }
                    if c.values()
                        .any(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
                    {
                        return Err(());
                    }
                }
            }
            Some("choice") => {
                let c = q["criteria"].as_object().ok_or(())?;
                if c.len() < 2 || c.len() > 255 {
                    return Err(());
                }
                if c.iter().any(|(k, v)| {
                    k.trim().is_empty() || v.as_str().is_none_or(|s| s.trim().is_empty())
                }) {
                    return Err(());
                }
            }
            Some("score") => {
                let c = q["criteria"].as_array().ok_or(())?;
                if !(2..=10).contains(&c.len()) {
                    return Err(());
                }
                if c.iter()
                    .any(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
                {
                    return Err(());
                }
            }
            _ => return Err(()),
        }
    }
    Ok(())
}
pub(super) fn answers(qs: &Value, response: &Value) -> Result<(), ()> {
    let qs = qs.as_object().ok_or(())?;
    let answers = response["answers"].as_object().ok_or(())?;
    if qs.len() != answers.len() || response["model"].as_str().is_none_or(|s| s.is_empty()) {
        return Err(());
    }
    let usage = &response["usage"];
    if usage["input_tokens"].as_u64().is_none()
        || usage["output_tokens"].as_u64().is_none()
        || usage
            .get("cost")
            .is_some_and(|c| !c.as_f64().is_some_and(|n| n >= 0.0))
    {
        return Err(());
    }
    for (name, q) in qs {
        let a = answers.get(name).ok_or(())?;
        if a["type"] != q["type"] {
            return Err(());
        }
        if q["type"] == "noul" {
            if !probability(&a["noul"]) {
                return Err(());
            }
            continue;
        }
        if !probability(&a["confidence"]) {
            return Err(());
        }
        let ps = a["probabilities"].as_object().ok_or(())?;
        let keys: Vec<String> = if q["type"] == "choice" {
            q["criteria"]
                .as_object()
                .ok_or(())?
                .keys()
                .cloned()
                .collect()
        } else {
            (0..q["criteria"].as_array().ok_or(())?.len())
                .map(|n| n.to_string())
                .collect()
        };
        if ps.len() != keys.len() || keys.iter().any(|k| !ps.get(k).is_some_and(probability)) {
            return Err(());
        }
        let probabilities: Vec<_> = keys.iter().map(|key| ps[key].as_f64().unwrap()).collect();
        let (minimum, maximum) = rounded_expectation_bounds(&probabilities).ok_or(())?;
        if q["type"] == "choice" {
            let choice = a["choice"].as_str().ok_or(())?;
            let selected = ps.get(choice).and_then(Value::as_f64).ok_or(())?;
            if ps
                .values()
                .filter_map(Value::as_f64)
                .any(|p| p > selected + 0.001)
            {
                return Err(());
            }
        } else {
            let criteria = q["criteria"].as_array().ok_or(())?;
            let legend = a["legend"].as_object().ok_or(())?;
            if legend.len() != criteria.len()
                || criteria
                    .iter()
                    .enumerate()
                    .any(|(i, label)| legend.get(&i.to_string()) != Some(label))
            {
                return Err(());
            }
            let score = a["score"].as_f64().ok_or(())?;
            if score < 0.0
                || score > (keys.len() - 1) as f64
                || score + 0.005 + 1e-9 < minimum
                || score - 0.005 - 1e-9 > maximum
            {
                return Err(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score_answer(probabilities: &[f64], score: f64) -> Result<(), ()> {
        let criteria: Vec<_> = (0..probabilities.len())
            .map(|i| format!("Level {i}"))
            .collect();
        let qs = serde_json::json!({"quality":{"type":"score","instructions":"Rate", "criteria":criteria}});
        let ps: serde_json::Map<String, Value> = probabilities
            .iter()
            .enumerate()
            .map(|(i, p)| (i.to_string(), serde_json::json!(p)))
            .collect();
        let legend: serde_json::Map<String, Value> = criteria
            .iter()
            .enumerate()
            .map(|(i, label)| (i.to_string(), serde_json::json!(label)))
            .collect();
        answers(
            &qs,
            &serde_json::json!({"model":"test/model", "usage":{"input_tokens":1,"output_tokens":1}, "answers":{"quality":{"type":"score","confidence":0.8,"score":score,"probabilities":ps,"legend":legend}}}),
        )
    }

    #[test]
    fn scores_allow_independent_two_decimal_rounding() {
        assert!(score_answer(&[0.01, 0.09, 0.9], 1.9).is_ok());
        assert!(score_answer(&[0.02, 0.08, 0.23, 0.33, 0.3, 0.01, 0.02], 2.92).is_ok());
        assert!(score_answer(&[0.33, 0.33, 0.33], 1.0).is_ok());
        assert!(score_answer(&[0.9, 0.1], 0.9).is_err());
        assert!(score_answer(&[0.2, 0.2, 0.2], 1.0).is_err());
        assert!(score_answer(&[0.34, 0.34, 0.34], 1.0).is_err());
    }
}
