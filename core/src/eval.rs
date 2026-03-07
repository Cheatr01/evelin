use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Instant;

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::{effective_runner_config, EvalConfig, ProjectLayout, PromptCase};
use crate::error::{EvelinError, Result};
use crate::runtime::{
    is_retryable_codex_error, CodexExecutor, ExecMetadata, ExecRequest, SubprocessCodexExecutor,
    SuiteEnvironment, PARALLEL_START_STAGGER_MS,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalResultRow {
    pub id: String,
    pub prompt: String,
    pub pass: bool,
    pub found_response: bool,
    pub missing_required_markers: Vec<String>,
    pub forbidden_markers_present: Vec<String>,
    pub response: String,
    pub codex_exec: ExecMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalRunnerReport {
    pub mode: String,
    pub global_config_path: Option<String>,
    pub max_concurrency: usize,
    pub effective_concurrency: usize,
    pub serial_retry_count: usize,
    pub codex_timeout_seconds: u64,
    pub codex_sandbox: String,
    pub codex_model: Option<String>,
    pub codex_reasoning_effort: Option<String>,
    pub codex_extra_args: Vec<String>,
    pub codex_isolation: bool,
    pub codex_home_base_dir: String,
    pub codex_home: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pass_rate: f64,
    pub verdict: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    pub eval_type: String,
    pub skill: String,
    pub generated_at_utc: DateTime<Utc>,
    pub duration_ms: u128,
    pub rate: f64,
    pub summary: EvalSummary,
    pub runner: EvalRunnerReport,
    pub results: Vec<EvalResultRow>,
}

pub fn run_eval(layout: &ProjectLayout, config_path: &Path) -> Result<EvalReport> {
    run_eval_with_executor(layout, config_path, &SubprocessCodexExecutor)
}

pub fn run_eval_with_executor<E: CodexExecutor>(
    layout: &ProjectLayout,
    config_path: &Path,
    executor: &E,
) -> Result<EvalReport> {
    let started = Instant::now();
    let config = EvalConfig::from_path(config_path, &layout.root)?;
    let runner = effective_runner_config(layout, &config)?;
    let suite_env = SuiteEnvironment::prepare(layout, &config, &runner)?;
    let mut responses = HashMap::new();
    let mut execution_meta = HashMap::new();

    let effective_concurrency = runner.max_concurrency.max(1).min(config.cases.len().max(1));
    if effective_concurrency == 1 {
        for prompt in &config.cases {
            let (prompt_id, response_text, metadata) =
                run_prompt_case(layout, &runner, &suite_env, prompt, 0, executor)?;
            responses.insert(prompt_id.clone(), response_text);
            execution_meta.insert(prompt_id, metadata);
        }
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(effective_concurrency)
            .build()
            .map_err(|error| {
                EvelinError::Process(format!("failed to build thread pool: {error}"))
            })?;
        let parallel_results = pool.install(|| {
            config
                .cases
                .par_iter()
                .enumerate()
                .map(|(index, prompt)| {
                    run_prompt_case(
                        layout,
                        &runner,
                        &suite_env,
                        prompt,
                        PARALLEL_START_STAGGER_MS * index as u64,
                        executor,
                    )
                })
                .collect::<Vec<_>>()
        });
        for item in parallel_results {
            let (prompt_id, response_text, metadata) = item?;
            responses.insert(prompt_id.clone(), response_text);
            execution_meta.insert(prompt_id, metadata);
        }

        let retry_prompts = config
            .cases
            .iter()
            .filter(|prompt| {
                let metadata = execution_meta.get(&prompt.id);
                let response = responses.get(&prompt.id);
                response
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
                    && metadata.map(is_retryable_codex_error).unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();

        for prompt in retry_prompts {
            let previous = execution_meta.get(&prompt.id).cloned();
            let (prompt_id, response_text, mut metadata) =
                run_prompt_case(layout, &runner, &suite_env, &prompt, 0, executor)?;
            if let Some(previous) = previous {
                metadata.retry = Some(crate::runtime::RetryMetadata {
                    mode: "serial_after_parallel_failure".to_owned(),
                    previous_duration_ms: Some(previous.duration_ms),
                    previous_error: if previous.error.is_empty() {
                        None
                    } else {
                        Some(previous.error)
                    },
                });
            }
            responses.insert(prompt_id.clone(), response_text);
            execution_meta.insert(prompt_id, metadata);
        }
    }

    Ok(grade_eval(
        layout,
        &config,
        &runner,
        &suite_env,
        started.elapsed().as_millis(),
        effective_concurrency,
        &responses,
        &execution_meta,
    ))
}

pub fn write_eval_report(path: &Path, report: &EvalReport) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| EvelinError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(report).map_err(|error| {
            EvelinError::message(format!("failed to serialize eval report: {error}"))
        })? + "\n",
    )
    .map_err(|source| EvelinError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn run_prompt_case<E: CodexExecutor>(
    layout: &ProjectLayout,
    runner: &crate::config::RunnerConfig,
    suite_env: &SuiteEnvironment,
    prompt: &PromptCase,
    start_delay_ms: u64,
    executor: &E,
) -> Result<(String, String, ExecMetadata)> {
    if start_delay_ms > 0 {
        thread::sleep(std::time::Duration::from_millis(start_delay_ms));
    }
    let case_env = suite_env.case_env(&prompt.id)?;
    let response = executor.execute(ExecRequest {
        prompt_id: prompt.id.clone(),
        prompt_text: prompt.prompt.clone(),
        env: case_env.env,
        cwd: layout.root.clone(),
        runner: runner.clone(),
    })?;
    Ok((prompt.id.clone(), response.response_text, response.metadata))
}

fn grade_eval(
    layout: &ProjectLayout,
    config: &EvalConfig,
    runner: &crate::config::RunnerConfig,
    suite_env: &SuiteEnvironment,
    duration_ms: u128,
    effective_concurrency: usize,
    responses: &HashMap<String, String>,
    execution_meta: &HashMap<String, ExecMetadata>,
) -> EvalReport {
    let mut results = Vec::new();
    let mut passed = 0usize;

    for prompt in &config.cases {
        let response_text = responses.get(&prompt.id).cloned().unwrap_or_default();
        let response_lower = response_text.to_ascii_lowercase();
        let missing_required_markers = prompt
            .expected
            .must_include
            .iter()
            .filter(|marker| !response_lower.contains(&marker.to_ascii_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        let forbidden_markers_present = prompt
            .expected
            .must_not_include
            .iter()
            .filter(|marker| response_lower.contains(&marker.to_ascii_lowercase()))
            .cloned()
            .collect::<Vec<_>>();
        let found_response = !response_text.trim().is_empty();
        let pass = found_response
            && missing_required_markers.is_empty()
            && forbidden_markers_present.is_empty();
        if pass {
            passed += 1;
        }
        results.push(EvalResultRow {
            id: prompt.id.clone(),
            prompt: prompt.prompt.clone(),
            pass,
            found_response,
            missing_required_markers,
            forbidden_markers_present,
            response: response_text,
            codex_exec: execution_meta
                .get(&prompt.id)
                .cloned()
                .unwrap_or(ExecMetadata {
                    return_code: 1,
                    timed_out: false,
                    duration_ms: 0,
                    error: "missing execution metadata".to_owned(),
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    retry: None,
                }),
        });
    }

    let total = results.len();
    let pass_rate = if total == 0 {
        0.0
    } else {
        passed as f64 / total as f64
    };
    let verdict = if total == 0 {
        "skipped"
    } else if pass_rate >= config.rate {
        "pass"
    } else {
        "fail"
    };
    EvalReport {
        eval_type: config.eval_type.as_str().to_owned(),
        skill: config.skill.clone(),
        generated_at_utc: Utc::now(),
        duration_ms,
        rate: config.rate,
        summary: EvalSummary {
            total,
            passed,
            failed: total.saturating_sub(passed),
            pass_rate: (pass_rate * 10000.0).round() / 10000.0,
            verdict: verdict.to_owned(),
            duration_ms,
        },
        runner: EvalRunnerReport {
            mode: "live_codex_exec".to_owned(),
            global_config_path: layout
                .global_eval_config_path
                .exists()
                .then(|| layout.global_eval_config_path.display().to_string()),
            max_concurrency: runner.max_concurrency,
            effective_concurrency,
            serial_retry_count: results
                .iter()
                .filter(|row| row.codex_exec.retry.is_some())
                .count(),
            codex_timeout_seconds: runner.codex_timeout_seconds,
            codex_sandbox: runner.codex_sandbox.clone(),
            codex_model: runner.codex_model.clone(),
            codex_reasoning_effort: runner.codex_reasoning_effort.clone(),
            codex_extra_args: runner.codex_extra_args.clone(),
            codex_isolation: runner.codex_isolation,
            codex_home_base_dir: suite_env.codex_home_base_dir.display().to_string(),
            codex_home: suite_env
                .codex_home
                .as_ref()
                .map(|path| path.display().to_string()),
        },
        results,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::runtime::{CodexExecutor, ExecMetadata, ExecRequest, ExecResponse};

    use super::{run_eval_with_executor, ProjectLayout};

    struct MockExecutor;

    impl CodexExecutor for MockExecutor {
        fn execute(&self, request: ExecRequest) -> crate::error::Result<ExecResponse> {
            let response_text = if request.prompt_text.contains("Use") {
                "Hello world!".to_owned()
            } else {
                "Just a joke".to_owned()
            };
            Ok(ExecResponse {
                response_text,
                metadata: ExecMetadata {
                    return_code: 0,
                    timed_out: false,
                    duration_ms: 5,
                    error: String::new(),
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    retry: None,
                },
            })
        }
    }

    #[test]
    fn runs_eval_with_mock_executor() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("skills").join("hello-world")).expect("skills");
        fs::write(
            root.join("skills").join("hello-world").join("SKILL.md"),
            "Hello world!",
        )
        .expect("skill");
        fs::write(
            root.join("hello.eval.yaml"),
            r#"
eval_type: skill
skill: hello-world
skill_path: skills/hello-world/SKILL.md
grader: markers
rate: 0.5
cases:
  - id: explicit
    prompt: Use $hello-world skill.
    expected:
      must_include: ["Hello world!"]
      must_not_include: []
  - id: negative
    prompt: Tell me a joke.
    expected:
      must_include: []
      must_not_include: ["Hello world!"]
"#,
        )
        .expect("config");

        let report = run_eval_with_executor(
            &ProjectLayout::discover(root),
            &root.join("hello.eval.yaml"),
            &MockExecutor,
        )
        .expect("report");
        assert_eq!(report.summary.verdict, "pass");
        assert_eq!(report.summary.passed, 2);
    }
}
