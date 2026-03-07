use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

pub const SKILL_SUITE_SCHEMA: &str = include_str!("skill-suite.schema.yaml");

const TOP_LEVEL_KEYS: &[&str] = &[
    "eval_type",
    "skill",
    "skill_path",
    "gate_requirements",
    "eval_name",
    "grader",
    "rate",
    "max_concurrency",
    "codex_timeout_seconds",
    "codex_isolation",
    "codex_home_base_dir",
    "codex_sandbox",
    "codex_model",
    "codex_reasoning_effort",
    "codex_extra_args",
    "cases",
    "prompts_file",
];

const CASE_KEYS: &[&str] = &["id", "prompt", "expected"];
const EXPECTED_KEYS: &[&str] = &["must_include", "must_not_include"];
const GATE_KEYS: &[&str] = &["required_snippets"];

pub fn validate_suite_document(config_path: &Path, data: &Value) -> Vec<String> {
    let is_yaml = matches!(
        config_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase())
            .as_deref(),
        Some("yaml" | "yml")
    );
    if !is_yaml {
        return Vec::new();
    }
    manual_validate(data)
}

fn manual_validate(data: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(object) = data.as_object() else {
        return vec!["$ must be an object".to_owned()];
    };

    push_unknown_keys(
        object.keys().map(String::as_str),
        TOP_LEVEL_KEYS,
        &[],
        &mut errors,
    );

    for key in ["eval_type", "skill", "skill_path", "grader", "rate"] {
        if !object.contains_key(key) {
            errors.push(format!("$ missing required property '{key}'"));
        }
    }
    if !object.contains_key("cases") && !object.contains_key("prompts_file") {
        errors.push("$ missing required property 'cases' or 'prompts_file'".to_owned());
    }

    validate_non_empty_string(object.get("eval_type"), &["eval_type"], &mut errors);
    if let Some(value) = object.get("eval_type").and_then(Value::as_str) {
        if value != "skill" {
            errors.push("$.eval_type must be one of ['skill']".to_owned());
        }
    }

    validate_non_empty_string(object.get("skill"), &["skill"], &mut errors);
    validate_non_empty_string(object.get("skill_path"), &["skill_path"], &mut errors);
    if let Some(value) = object.get("skill_path").and_then(Value::as_str) {
        if !value.ends_with("SKILL.md") {
            errors.push("$.skill_path must point to SKILL.md".to_owned());
        }
    }

    if let Some(value) = object.get("gate_requirements") {
        validate_gate_requirements(value, &mut errors);
    }

    if object.contains_key("eval_name") {
        validate_non_empty_string(object.get("eval_name"), &["eval_name"], &mut errors);
    }

    validate_non_empty_string(object.get("grader"), &["grader"], &mut errors);
    if let Some(value) = object.get("grader").and_then(Value::as_str) {
        if value != "markers" {
            errors.push("$.grader must be one of ['markers']".to_owned());
        }
    }

    match object.get("rate") {
        Some(Value::Number(value)) if value.as_f64().is_some() => {
            let value = value.as_f64().unwrap_or_default();
            if !(0.0..=1.0).contains(&value) {
                errors.push("$.rate must be between 0 and 1".to_owned());
            }
        }
        Some(_) => errors.push("$.rate must be a number".to_owned()),
        None => {}
    }

    validate_optional_int(
        object.get("max_concurrency"),
        &["max_concurrency"],
        &mut errors,
    );
    validate_optional_int(
        object.get("codex_timeout_seconds"),
        &["codex_timeout_seconds"],
        &mut errors,
    );
    validate_optional_bool(
        object.get("codex_isolation"),
        &["codex_isolation"],
        &mut errors,
    );
    if object.contains_key("codex_home_base_dir") {
        validate_non_empty_string(
            object.get("codex_home_base_dir"),
            &["codex_home_base_dir"],
            &mut errors,
        );
    }
    if object.contains_key("codex_sandbox") {
        validate_non_empty_string(object.get("codex_sandbox"), &["codex_sandbox"], &mut errors);
        if let Some(value) = object.get("codex_sandbox").and_then(Value::as_str) {
            if !matches!(
                value,
                "read-only" | "workspace-write" | "danger-full-access"
            ) {
                errors.push(
                    "$.codex_sandbox must be one of ['read-only', 'workspace-write', 'danger-full-access']"
                        .to_owned(),
                );
            }
        }
    }
    if let Some(value) = object.get("codex_model") {
        if !value.is_string() {
            errors.push("$.codex_model must be a string".to_owned());
        }
    }
    if object.contains_key("codex_reasoning_effort") {
        validate_non_empty_string(
            object.get("codex_reasoning_effort"),
            &["codex_reasoning_effort"],
            &mut errors,
        );
        if let Some(value) = object.get("codex_reasoning_effort").and_then(Value::as_str) {
            if !matches!(value, "none" | "low" | "medium" | "high" | "xhigh") {
                errors.push(
                    "$.codex_reasoning_effort must be one of ['none', 'low', 'medium', 'high', 'xhigh']"
                        .to_owned(),
                );
            }
        }
    }
    if let Some(value) = object.get("codex_extra_args") {
        validate_string_list(value, &["codex_extra_args"], &mut errors);
    }
    if let Some(value) = object.get("prompts_file") {
        validate_non_empty_string(Some(value), &["prompts_file"], &mut errors);
    }
    if let Some(value) = object.get("cases") {
        validate_cases(value, &mut errors);
    }

    errors
}

fn push_unknown_keys<'a, I>(keys: I, allowed: &[&str], path: &[&str], errors: &mut Vec<String>)
where
    I: Iterator<Item = &'a str>,
{
    let allowed: BTreeSet<&str> = allowed.iter().copied().collect();
    for key in keys {
        if !allowed.contains(key) {
            let mut parts = path.to_vec();
            parts.push(key);
            errors.push(format!("{} is not allowed", json_path(&parts)));
        }
    }
}

fn validate_cases(value: &Value, errors: &mut Vec<String>) {
    let Some(rows) = value.as_array() else {
        errors.push("$.cases must be an array".to_owned());
        return;
    };
    for (index, row) in rows.iter().enumerate() {
        let path = vec!["cases".to_owned(), format!("[{index}]")];
        let Some(object) = row.as_object() else {
            errors.push(format!("{} must be an object", json_path_owned(&path)));
            continue;
        };

        push_unknown_keys(
            object.keys().map(String::as_str),
            CASE_KEYS,
            &["cases", &format!("[{index}]")],
            errors,
        );
        for key in ["id", "prompt", "expected"] {
            if !object.contains_key(key) {
                errors.push(format!(
                    "{} missing required property '{key}'",
                    json_path_owned(&path)
                ));
            }
        }
        validate_non_empty_string(
            object.get("id"),
            &["cases", &format!("[{index}]"), "id"],
            errors,
        );
        validate_non_empty_string(
            object.get("prompt"),
            &["cases", &format!("[{index}]"), "prompt"],
            errors,
        );
        if let Some(expected) = object.get("expected") {
            validate_expected(expected, index, errors);
        }
    }
}

fn validate_expected(value: &Value, index: usize, errors: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        errors.push(format!(
            "{} must be an object",
            json_path(&["cases", &format!("[{index}]"), "expected"])
        ));
        return;
    };
    push_unknown_keys(
        object.keys().map(String::as_str),
        EXPECTED_KEYS,
        &["cases", &format!("[{index}]"), "expected"],
        errors,
    );
    for key in ["must_include", "must_not_include"] {
        if let Some(value) = object.get(key) {
            validate_string_list(
                value,
                &["cases", &format!("[{index}]"), "expected", key],
                errors,
            );
        }
    }
}

fn validate_gate_requirements(value: &Value, errors: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        errors.push("$.gate_requirements must be an object".to_owned());
        return;
    };
    push_unknown_keys(
        object.keys().map(String::as_str),
        GATE_KEYS,
        &["gate_requirements"],
        errors,
    );
    if !object.contains_key("required_snippets") {
        errors.push("$.gate_requirements missing required property 'required_snippets'".to_owned());
        return;
    }
    if let Some(value) = object.get("required_snippets") {
        validate_string_list(value, &["gate_requirements", "required_snippets"], errors);
    }
}

fn validate_non_empty_string(value: Option<&Value>, path: &[&str], errors: &mut Vec<String>) {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => {}
        Some(Value::String(_)) => errors.push(format!("{} must not be empty", json_path(path))),
        Some(_) => errors.push(format!("{} must be a string", json_path(path))),
        None => {}
    }
}

fn validate_optional_int(value: Option<&Value>, path: &[&str], errors: &mut Vec<String>) {
    let Some(value) = value else {
        return;
    };
    match value.as_i64() {
        Some(number) if number >= 1 => {}
        Some(_) => errors.push(format!("{} must be >= 1", json_path(path))),
        None => errors.push(format!("{} must be an integer", json_path(path))),
    }
}

fn validate_optional_bool(value: Option<&Value>, path: &[&str], errors: &mut Vec<String>) {
    let Some(value) = value else {
        return;
    };
    if !value.is_boolean() {
        errors.push(format!("{} must be a boolean", json_path(path)));
    }
}

fn validate_string_list(value: &Value, path: &[&str], errors: &mut Vec<String>) {
    let Some(rows) = value.as_array() else {
        errors.push(format!("{} must be an array of strings", json_path(path)));
        return;
    };
    for (index, row) in rows.iter().enumerate() {
        if !row.is_string() {
            errors.push(format!(
                "{} must be a string",
                json_path_owned(
                    &path
                        .iter()
                        .map(|part| part.to_string())
                        .chain([format!("[{index}]")])
                        .collect::<Vec<_>>()
                )
            ));
        }
    }
}

fn json_path(parts: &[&str]) -> String {
    if parts.is_empty() {
        return "$".to_owned();
    }
    let mut out = String::from("$");
    for part in parts {
        if part.starts_with('[') {
            out.push_str(part);
        } else {
            out.push('.');
            out.push_str(part);
        }
    }
    out
}

fn json_path_owned(parts: &[String]) -> String {
    if parts.is_empty() {
        return "$".to_owned();
    }
    let mut out = String::from("$");
    for part in parts {
        if part.starts_with('[') {
            out.push_str(part);
        } else {
            out.push('.');
            out.push_str(part);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::validate_suite_document;

    #[test]
    fn validates_yaml_suite() {
        let errors = validate_suite_document(
            Path::new("suite.eval.yaml"),
            &json!({
                "eval_type": "skill",
                "skill": "hello-world",
                "skill_path": "skills/hello-world/SKILL.md",
                "grader": "markers",
                "rate": 0.875,
                "cases": [{"id":"a","prompt":"b","expected":{"must_include":[],"must_not_include":[]}}]
            }),
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn reports_invalid_fields() {
        let errors =
            validate_suite_document(Path::new("suite.eval.yaml"), &json!({"eval_type":"oops"}));
        assert!(errors
            .iter()
            .any(|item| item.contains("$.eval_type must be one of ['skill']")));
        assert!(errors
            .iter()
            .any(|item| item.contains("$ missing required property 'skill'")));
    }
}
