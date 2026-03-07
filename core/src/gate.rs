use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::config::ProjectLayout;
use crate::document::{as_object, load_document, resolve_path};
use crate::error::{EvelinError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRequirementItem {
    pub name: String,
    pub skill_path: Option<String>,
    pub required_snippets: Vec<String>,
    pub config_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateLintResultRow {
    pub skill: String,
    pub skill_path: String,
    pub config_path: Option<String>,
    pub pass: bool,
    pub missing_file: bool,
    pub missing_snippets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub verdict: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateLintReport {
    pub generated_at_utc: DateTime<Utc>,
    pub duration_ms: u128,
    pub summary: ReportSummary,
    pub results: Vec<GateLintResultRow>,
}

pub fn load_requirements_from_file(path: &Path) -> Result<Vec<GateRequirementItem>> {
    let value = load_document(path)?;
    let object = as_object(&value, "requirements document")?;

    if object.contains_key("gate_requirements") {
        return load_requirements_from_suite(path, &value);
    }
    if let Some(skills) = object.get("skills") {
        let rows = skills.as_array().ok_or_else(|| {
            EvelinError::message(format!("Invalid skills list in {}", path.display()))
        })?;
        let mut items = Vec::new();
        for row in rows {
            items.push(parse_requirement_item(row, Some(path))?);
        }
        return Ok(items);
    }
    if object.contains_key("name") && object.contains_key("required_snippets") {
        return Ok(vec![parse_requirement_item(&value, Some(path))?]);
    }

    Err(EvelinError::message(format!(
        "Unsupported requirements format in {}",
        path.display()
    )))
}

pub fn discover_requirements(discover_dir: &Path) -> Result<Vec<GateRequirementItem>> {
    let mut rows = Vec::new();
    for entry in WalkDir::new(discover_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file() && entry.file_name() == "gate_requirements.json" {
            rows.extend(load_requirements_from_file(entry.path())?);
        }
    }
    Ok(rows)
}

pub fn lint_items(
    layout: &ProjectLayout,
    items: &[GateRequirementItem],
    duration_ms: u128,
) -> Result<GateLintReport> {
    let mut results = Vec::new();
    let mut passed = 0usize;

    for item in items {
        let skill_file = item
            .skill_path
            .as_ref()
            .map(|value| resolve_path(&layout.root, value))
            .unwrap_or_else(|| layout.skills_dir.join(&item.name).join("SKILL.md"));
        if !skill_file.exists() {
            results.push(GateLintResultRow {
                skill: item.name.clone(),
                skill_path: skill_file.display().to_string(),
                config_path: item.config_path.clone(),
                pass: false,
                missing_file: true,
                missing_snippets: item.required_snippets.clone(),
            });
            continue;
        }
        let text = fs::read_to_string(&skill_file).map_err(|source| EvelinError::Io {
            path: skill_file.clone(),
            source,
        })?;
        let missing_snippets = item
            .required_snippets
            .iter()
            .filter(|snippet| !text.contains(snippet.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let pass = missing_snippets.is_empty();
        if pass {
            passed += 1;
        }
        results.push(GateLintResultRow {
            skill: item.name.clone(),
            skill_path: skill_file.display().to_string(),
            config_path: item.config_path.clone(),
            pass,
            missing_file: false,
            missing_snippets,
        });
    }

    let total = results.len();
    Ok(GateLintReport {
        generated_at_utc: Utc::now(),
        duration_ms,
        summary: ReportSummary {
            total,
            passed,
            failed: total.saturating_sub(passed),
            verdict: if total > 0 && passed == total {
                "pass".to_owned()
            } else {
                "fail".to_owned()
            },
            duration_ms,
        },
        results,
    })
}

fn load_requirements_from_suite(path: &Path, value: &Value) -> Result<Vec<GateRequirementItem>> {
    let object = as_object(value, "suite document")?;
    let gate = object.get("gate_requirements").ok_or_else(|| {
        EvelinError::message(format!(
            "Missing or invalid gate_requirements in {}",
            path.display()
        ))
    })?;
    let gate_object = as_object(gate, "gate_requirements")?;
    let required_snippets = gate_object
        .get("required_snippets")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            EvelinError::message(format!(
                "Invalid required_snippets list in {}",
                path.display()
            ))
        })?
        .iter()
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| {
                EvelinError::message(format!(
                    "Invalid required_snippets list in {}",
                    path.display()
                ))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![GateRequirementItem {
        name: object
            .get("skill")
            .and_then(Value::as_str)
            .ok_or_else(|| EvelinError::message(format!("Missing skill in {}", path.display())))?
            .to_owned(),
        skill_path: object
            .get("skill_path")
            .and_then(Value::as_str)
            .map(str::to_owned),
        required_snippets,
        config_path: Some(path.display().to_string()),
    }])
}

fn parse_requirement_item(
    value: &Value,
    config_path: Option<&Path>,
) -> Result<GateRequirementItem> {
    let object = as_object(value, "gate requirement item")?;
    let required_snippets = object
        .get("required_snippets")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            EvelinError::message(match config_path {
                Some(path) => format!("Invalid required_snippets list in {}", path.display()),
                None => "Invalid required_snippets list".to_owned(),
            })
        })?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| EvelinError::message("Invalid required_snippets list"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(GateRequirementItem {
        name: object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| EvelinError::message("Missing skill name"))?
            .to_owned(),
        skill_path: object
            .get("skill_path")
            .and_then(Value::as_str)
            .map(str::to_owned),
        required_snippets,
        config_path: config_path.map(|path| path.display().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::config::ProjectLayout;

    use super::{lint_items, GateRequirementItem};

    #[test]
    fn lints_required_snippets() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        let skill_dir = root.join("skills").join("hello-world");
        fs::create_dir_all(&skill_dir).expect("dir");
        fs::write(skill_dir.join("SKILL.md"), "## Workflow\nHello").expect("write");

        let report = lint_items(
            &ProjectLayout::discover(root),
            &[GateRequirementItem {
                name: "hello-world".to_owned(),
                skill_path: None,
                required_snippets: vec!["## Workflow".to_owned()],
                config_path: None,
            }],
            12,
        )
        .expect("report");

        assert_eq!(report.summary.verdict, "pass");
    }
}
