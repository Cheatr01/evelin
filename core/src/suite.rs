use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{is_default_eval_config, strip_eval_suffix, EvalConfig, ProjectLayout};
use crate::error::{EvelinError, Result};
use crate::eval::{run_eval_with_executor, write_eval_report};
use crate::gate::lint_items;
use crate::gate::load_requirements_from_file;
use crate::runtime::CodexExecutor;
use crate::schema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteStep {
    pub step_type: String,
    pub name: String,
    pub status: String,
    pub config: Option<String>,
    pub output: Option<String>,
    pub detail: Option<String>,
    pub runtime_detail: Option<String>,
    pub duration_ms: Option<u128>,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteSummary {
    pub verdict: String,
    pub steps_total: usize,
    pub steps_passed: usize,
    pub steps_failed: usize,
    pub steps_skipped: usize,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSuiteReport {
    pub skill: String,
    pub generated_at_utc: DateTime<Utc>,
    pub skill_test_dir: String,
    pub out_dir: String,
    pub steps: Vec<SuiteStep>,
    pub summary: SuiteSummary,
}

pub fn discover_eval_configs(skill_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for default_name in ["eval_config.json", "eval.yaml", "eval.yml"] {
        let path = skill_dir.join(default_name);
        if path.exists() {
            candidates.push(path);
        }
    }
    for entry in fs::read_dir(skill_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(|entry| entry.ok()))
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if matches!(
            name,
            _ if name.ends_with(".eval_config.json")
                || name.ends_with(".eval.yaml")
                || name.ends_with(".eval.yml")
        ) && !candidates.contains(&path)
        {
            candidates.push(path);
        }
    }
    candidates.sort();
    candidates
}

pub fn run_skill_suite(
    layout: &ProjectLayout,
    skill: &str,
    out_dir: &Path,
) -> Result<SkillSuiteReport> {
    run_skill_suite_with_executor(
        layout,
        skill,
        out_dir,
        &crate::runtime::SubprocessCodexExecutor,
    )
}

pub fn run_skill_suite_with_executor<E: CodexExecutor>(
    layout: &ProjectLayout,
    skill: &str,
    out_dir: &Path,
    executor: &E,
) -> Result<SkillSuiteReport> {
    let started = Instant::now();
    let skill_dir = layout.tests_skills_dir.join(skill);
    let mut steps = Vec::new();

    if !skill_dir.exists() {
        return Ok(SkillSuiteReport {
            skill: skill.to_owned(),
            generated_at_utc: Utc::now(),
            skill_test_dir: skill_dir.display().to_string(),
            out_dir: out_dir.display().to_string(),
            steps,
            summary: SuiteSummary {
                verdict: "fail".to_owned(),
                steps_total: 0,
                steps_passed: 0,
                steps_failed: 1,
                steps_skipped: 0,
                duration_ms: started.elapsed().as_millis(),
            },
        });
    }

    fs::create_dir_all(out_dir).map_err(|source| EvelinError::Io {
        path: out_dir.to_path_buf(),
        source,
    })?;

    let eval_configs = discover_eval_configs(&skill_dir);
    let mut schema_validity = Vec::new();

    for config_path in &eval_configs {
        let step_started = Instant::now();
        let value = crate::document::load_document(config_path)?;
        let errors = schema::validate_suite_document(config_path, &value);
        let fallback_name = fallback_eval_output_name(config_path);
        let schema_out = out_dir.join(format!("{fallback_name}.schema.json"));
        let step = SuiteStep {
            step_type: "schema_lint".to_owned(),
            name: format!("schema:{fallback_name}"),
            status: if errors.is_empty() { "pass" } else { "fail" }.to_owned(),
            config: Some(config_path.display().to_string()),
            output: Some(schema_out.display().to_string()),
            detail: Some(if errors.is_empty() {
                format!("schema ok  t={}ms", step_started.elapsed().as_millis())
            } else {
                format!(
                    "{} error(s)  t={}ms",
                    errors.len(),
                    step_started.elapsed().as_millis()
                )
            }),
            runtime_detail: None,
            duration_ms: Some(step_started.elapsed().as_millis()),
            exit_code: Some(if errors.is_empty() { 0 } else { 1 }),
            reason: None,
        };
        fs::write(
            &schema_out,
            serde_json::to_string_pretty(&serde_json::json!({
                "config_path": config_path.display().to_string(),
                "schema": "core/src/skill-suite.schema.yaml",
                "errors": errors,
            }))
            .map_err(|error| {
                EvelinError::message(format!("failed to serialize schema report: {error}"))
            })? + "\n",
        )
        .map_err(|source| EvelinError::Io {
            path: schema_out.clone(),
            source,
        })?;
        schema_validity.push((config_path.clone(), step.status == "pass"));
        steps.push(step);
    }

    if let Some(gate_config) = find_gate_config(&eval_configs, &schema_validity, &skill_dir)? {
        let gate_started = Instant::now();
        let items = load_requirements_from_file(&gate_config)?;
        let report = lint_items(layout, &items, gate_started.elapsed().as_millis())?;
        let gate_out = out_dir.join("gate-lint.json");
        write_json(&gate_out, &report)?;
        steps.push(SuiteStep {
            step_type: "gate_lint".to_owned(),
            name: "gate-lint".to_owned(),
            status: report.summary.verdict.clone(),
            config: Some(gate_config.display().to_string()),
            output: Some(gate_out.display().to_string()),
            detail: Some(format!(
                "{}/{} checks  t={}ms",
                report.summary.passed, report.summary.total, report.duration_ms
            )),
            runtime_detail: None,
            duration_ms: Some(report.duration_ms),
            exit_code: Some(if report.summary.verdict == "pass" {
                0
            } else {
                1
            }),
            reason: None,
        });
    } else {
        steps.push(SuiteStep {
            step_type: "gate_lint".to_owned(),
            name: "gate-lint".to_owned(),
            status: "skipped".to_owned(),
            config: None,
            output: None,
            detail: None,
            runtime_detail: None,
            duration_ms: None,
            exit_code: None,
            reason: Some("missing valid embedded or legacy gate requirements".to_owned()),
        });
    }

    if eval_configs.is_empty() {
        steps.push(SuiteStep {
            step_type: "eval".to_owned(),
            name: "evals".to_owned(),
            status: "skipped".to_owned(),
            config: None,
            output: None,
            detail: None,
            runtime_detail: None,
            duration_ms: None,
            exit_code: None,
            reason: Some("no eval config".to_owned()),
        });
    } else {
        for config_path in &eval_configs {
            let fallback_name = fallback_eval_output_name(config_path);
            if !schema_validity
                .iter()
                .find(|(path, _)| path == config_path)
                .map(|(_, valid)| *valid)
                .unwrap_or(false)
            {
                steps.push(SuiteStep {
                    step_type: "eval".to_owned(),
                    name: fallback_name.clone(),
                    status: "skipped".to_owned(),
                    config: Some(config_path.display().to_string()),
                    output: None,
                    detail: None,
                    runtime_detail: None,
                    duration_ms: None,
                    exit_code: None,
                    reason: Some("schema validation failed".to_owned()),
                });
                continue;
            }
            let step_started = Instant::now();
            let config = EvalConfig::from_path(config_path, &layout.root)?;
            let eval_name = config.effective_eval_name(config_path);
            let output = out_dir.join(format!("{eval_name}.eval.json"));
            let report = run_eval_with_executor(layout, config_path, executor)?;
            write_eval_report(&output, &report)?;
            steps.push(SuiteStep {
                step_type: "eval".to_owned(),
                name: eval_name.clone(),
                status: report.summary.verdict.clone(),
                config: Some(config_path.display().to_string()),
                output: Some(output.display().to_string()),
                detail: Some(format!(
                    "{}/{} pass  rate={:.3}  t={}ms",
                    report.summary.passed,
                    report.summary.total,
                    report.summary.pass_rate,
                    step_started.elapsed().as_millis()
                )),
                runtime_detail: Some(format!(
                    "isolation={}  concurrency={}/{}  retries={}  sandbox={}  model={}  effort={}",
                    if report.runner.codex_isolation {
                        "on"
                    } else {
                        "off"
                    },
                    report.runner.max_concurrency,
                    report.runner.effective_concurrency,
                    report.runner.serial_retry_count,
                    report.runner.codex_sandbox,
                    report
                        .runner
                        .codex_model
                        .clone()
                        .unwrap_or_else(|| "default".to_owned()),
                    report
                        .runner
                        .codex_reasoning_effort
                        .clone()
                        .unwrap_or_else(|| "default".to_owned())
                )),
                duration_ms: Some(step_started.elapsed().as_millis()),
                exit_code: Some(if report.summary.verdict == "fail" {
                    1
                } else {
                    0
                }),
                reason: None,
            });
        }
    }

    let steps_total = steps.len();
    let steps_passed = steps.iter().filter(|step| step.status == "pass").count();
    let steps_failed = steps.iter().filter(|step| step.status == "fail").count();
    let steps_skipped = steps.iter().filter(|step| step.status == "skipped").count();
    let verdict = if steps_failed > 0 {
        "fail"
    } else if steps_passed == 0 {
        "skipped"
    } else {
        "pass"
    };

    Ok(SkillSuiteReport {
        skill: skill.to_owned(),
        generated_at_utc: Utc::now(),
        skill_test_dir: skill_dir.display().to_string(),
        out_dir: out_dir.display().to_string(),
        steps,
        summary: SuiteSummary {
            verdict: verdict.to_owned(),
            steps_total,
            steps_passed,
            steps_failed,
            steps_skipped,
            duration_ms: started.elapsed().as_millis(),
        },
    })
}

pub fn write_suite_report(path: &Path, report: &SkillSuiteReport) -> Result<()> {
    write_json(path, report)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EvelinError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(value).map_err(|error| {
            EvelinError::message(format!("failed to serialize json report: {error}"))
        })? + "\n",
    )
    .map_err(|source| EvelinError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn fallback_eval_output_name(config_path: &Path) -> String {
    let file_name = config_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("eval");
    if is_default_eval_config(file_name) {
        "eval".to_owned()
    } else {
        strip_eval_suffix(file_name)
    }
}

fn find_gate_config(
    eval_configs: &[PathBuf],
    schema_validity: &[(PathBuf, bool)],
    skill_dir: &Path,
) -> Result<Option<PathBuf>> {
    for config_path in eval_configs {
        if !schema_validity
            .iter()
            .find(|(path, _)| path == config_path)
            .map(|(_, valid)| *valid)
            .unwrap_or(false)
        {
            continue;
        }
        let root = skill_dir
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .unwrap_or(skill_dir);
        let config = EvalConfig::from_path(config_path, root)?;
        if config.gate_requirements.is_some() {
            return Ok(Some(config_path.clone()));
        }
    }
    let legacy = skill_dir.join("gate_requirements.json");
    if legacy.exists() {
        Ok(Some(legacy))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::runtime::{CodexExecutor, ExecMetadata, ExecRequest, ExecResponse};

    use super::{run_skill_suite_with_executor, ProjectLayout};

    struct MockExecutor;

    impl CodexExecutor for MockExecutor {
        fn execute(&self, _request: ExecRequest) -> crate::error::Result<ExecResponse> {
            Ok(ExecResponse {
                response_text: "Outcome Statement:\nScope (in):\nNon-goals (out):\nAcceptance Criteria:\nDependencies:\nReady for Subtask Breakdown: yes".to_owned(),
                metadata: ExecMetadata {
                    return_code: 0,
                    timed_out: false,
                    duration_ms: 4,
                    error: String::new(),
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    retry: None,
                },
            })
        }
    }

    #[test]
    fn runs_skill_suite_end_to_end() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        let skill_doc = root.join("skills").join("scope-to-acceptance");
        let skill_tests = root
            .join("tests")
            .join("src")
            .join("skills")
            .join("scope-to-acceptance");
        fs::create_dir_all(&skill_doc).expect("skill doc dir");
        fs::create_dir_all(&skill_tests).expect("skill tests dir");
        fs::write(
            skill_doc.join("SKILL.md"),
            "## Workflow\n## Output Contract\n## Gate Rules",
        )
        .expect("skill");
        fs::write(
            skill_tests.join("suite.eval.yaml"),
            r###"
eval_type: skill
skill: scope-to-acceptance
skill_path: skills/scope-to-acceptance/SKILL.md
gate_requirements:
  required_snippets:
    - "## Workflow"
    - "## Output Contract"
    - "## Gate Rules"
grader: markers
rate: 0.5
cases:
  - id: explicit
    prompt: use the skill
    expected:
      must_include: ["Outcome Statement:", "Scope (in):"]
      must_not_include: []
"###,
        )
        .expect("suite");

        let report = run_skill_suite_with_executor(
            &ProjectLayout::discover(root),
            "scope-to-acceptance",
            &root.join("out"),
            &MockExecutor,
        )
        .expect("report");

        assert_eq!(report.summary.verdict, "pass");
        assert!(report
            .steps
            .iter()
            .any(|step| step.step_type == "gate_lint" && step.status == "pass"));
        assert!(report
            .steps
            .iter()
            .any(|step| step.step_type == "eval" && step.status == "pass"));
    }
}
