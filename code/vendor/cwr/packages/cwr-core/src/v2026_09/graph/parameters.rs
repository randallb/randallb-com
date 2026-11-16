use super::*;

#[derive(Debug)]
pub(super) struct UnresolvedParameterConflict;

impl std::fmt::Display for UnresolvedParameterConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("shared parameter equality depends on unread inputs")
    }
}

impl std::error::Error for UnresolvedParameterConflict {}

/// Unread paths belong to the inspection, never to the authored JSON value.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(transparent)]
pub(super) struct Parameters {
    pub value: Value,
    #[serde(skip)]
    pub unresolved: BTreeMap<String, requirements::BindingExpression>,
    #[serde(skip)]
    pub sources: BTreeMap<String, Vec<requirements::InputSource>>,
}

impl From<Value> for Parameters {
    fn from(value: Value) -> Self {
        Self {
            value,
            unresolved: BTreeMap::new(),
            sources: BTreeMap::new(),
        }
    }
}

impl Default for Parameters {
    fn default() -> Self {
        empty_object().into()
    }
}

pub(super) fn child_pointer(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn within(path: &str, parent: &str) -> bool {
    path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|tail| tail.starts_with('/'))
}

impl Parameters {
    /// Compare concrete structure and unread source identities independently.
    /// Different unread sources may eventually agree, but cannot establish readiness.
    pub fn equivalent_value_at(
        &self,
        path: &str,
        value: &Value,
        other: &Self,
        other_value: &Value,
    ) -> requirements::InputEquivalence {
        self.equivalent_values_at(path, value, other, path, other_value)
    }

    fn equivalent_values_at(
        &self,
        path: &str,
        value: &Value,
        other: &Self,
        other_path: &str,
        other_value: &Value,
    ) -> requirements::InputEquivalence {
        use requirements::InputEquivalence::{Conflicting, Equal, Unresolved};
        let source_at = |params: &Self, path: &str| {
            params
                .unresolved
                .iter()
                .find(|(root, _)| within(path, root))
                .map(|(root, source)| (source.clone(), path[root.len()..].to_owned()))
        };
        match (source_at(self, path), source_at(other, other_path)) {
            (Some(left), Some(right)) if left == right => return Equal,
            (Some(_), _) | (_, Some(_)) => return Unresolved,
            _ => {}
        }
        let pairs = match (value, other_value) {
            (Value::Object(left), Value::Object(right)) if left.keys().eq(right.keys()) => left
                .iter()
                .map(|(key, value)| (key.clone(), value, &right[key]))
                .collect::<Vec<_>>(),
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => left
                .iter()
                .zip(right)
                .enumerate()
                .map(|(i, (a, b))| (i.to_string(), a, b))
                .collect(),
            _ => {
                return if value == other_value {
                    Equal
                } else {
                    Conflicting
                }
            }
        };
        let mut result = Equal;
        for (key, left, right) in pairs {
            match self.equivalent_values_at(
                &child_pointer(path, &key),
                left,
                other,
                &child_pointer(other_path, &key),
                right,
            ) {
                Conflicting => return Conflicting,
                Unresolved => result = Unresolved,
                Equal => {}
            }
        }
        result
    }

    pub fn unknown_at(&self, path: &str) -> bool {
        self.unresolved.keys().any(|unknown| within(path, unknown))
    }

    pub fn known_string_at(&self, path: &str) -> bool {
        self.unresolved
            .get(path)
            .is_some_and(requirements::BindingExpression::is_known_string)
    }

    pub fn can_be_object(&self) -> bool {
        if self.unknown_at("") {
            !self.known_string_at("")
        } else {
            self.value.is_object()
        }
    }
}
