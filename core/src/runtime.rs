use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempDir};
use wait_timeout::ChildExt;

use crate::config::{sanitize_name, EvalConfig, ProjectLayout, RunnerConfig};
use crate::error::{EvelinError, Result};

pub const PARALLEL_START_STAGGER_MS: u64 = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecMetadata {
    pub return_code: i32,
    pub timed_out: bool,
    pub duration_ms: u128,
    pub error: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub retry: Option<RetryMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryMetadata {
    pub mode: String,
    pub previous_duration_ms: Option<u128>,
    pub previous_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub prompt_id: String,
    pub prompt_text: String,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub runner: RunnerConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResponse {
    pub response_text: String,
    pub metadata: ExecMetadata,
}

pub trait CodexExecutor: Send + Sync {
    fn execute(&self, request: ExecRequest) -> Result<ExecResponse>;
}

#[derive(Debug, Clone)]
pub struct SubprocessCodexExecutor;

impl CodexExecutor for SubprocessCodexExecutor {
    fn execute(&self, request: ExecRequest) -> Result<ExecResponse> {
        let codex_bin = which::which("codex").map_err(|_| {
            EvelinError::Process("codex CLI not found in PATH; cannot run live evals".to_owned())
        })?;

        let output_file = NamedTempFile::new()
            .map_err(|source| EvelinError::Process(format!("temp file error: {source}")))?;
        let mut cmd = Command::new(codex_bin);
        cmd.arg("exec")
            .arg("--ephemeral")
            .arg("--color")
            .arg("never")
            .arg("--sandbox")
            .arg(&request.runner.codex_sandbox)
            .arg("--output-last-message")
            .arg(output_file.path());
        if let Some(model) = &request.runner.codex_model {
            if !model.trim().is_empty() {
                cmd.arg("--model").arg(model);
            }
        }
        if let Some(effort) = &request.runner.codex_reasoning_effort {
            if !effort.trim().is_empty() {
                cmd.arg("-c")
                    .arg(format!("model_reasoning_effort=\"{}\"", effort));
            }
        }
        for arg in &request.runner.codex_extra_args {
            cmd.arg(arg);
        }
        cmd.arg(&request.prompt_text)
            .current_dir(&request.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(request.env.clone());

        let started = std::time::Instant::now();
        let mut child = cmd.spawn().map_err(|source| {
            EvelinError::Process(format!("failed to start codex exec: {source}"))
        })?;

        let timeout = Duration::from_secs(request.runner.codex_timeout_seconds);
        let mut timed_out = false;
        let status = match child.wait_timeout(timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                return read_exec_response(
                    output_file.path(),
                    ExecMetadata {
                        return_code: 124,
                        timed_out,
                        duration_ms: started.elapsed().as_millis(),
                        error: format!(
                            "codex exec timeout after {}s",
                            request.runner.codex_timeout_seconds
                        ),
                        stdout_tail: String::new(),
                        stderr_tail: String::new(),
                        retry: None,
                    },
                );
            }
            Err(error) => {
                return Err(EvelinError::Process(format!(
                    "failed while waiting for codex exec: {error}"
                )))
            }
        };

        let output = child.wait_with_output().map_err(|source| {
            EvelinError::Process(format!("failed to collect codex output: {source}"))
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let return_code = status
            .code()
            .unwrap_or(if status.success() { 0 } else { 1 });
        let mut metadata = ExecMetadata {
            return_code,
            timed_out,
            duration_ms: started.elapsed().as_millis(),
            error: String::new(),
            stdout_tail: tail(&stdout),
            stderr_tail: tail(&stderr),
            retry: None,
        };
        if return_code != 0 {
            metadata.error = if stderr.trim().is_empty() {
                format!("codex exec failed (rc={return_code})")
            } else {
                format!("codex exec failed (rc={return_code}): {}", squeeze(&stderr))
            };
        }
        read_exec_response(output_file.path(), metadata)
    }
}

#[derive(Debug)]
pub struct SuiteEnvironment {
    suite_root: Option<TempDir>,
    template_home: Option<PathBuf>,
    case_homes_dir: Option<PathBuf>,
    pub codex_isolation: bool,
    pub codex_home_base_dir: PathBuf,
    pub codex_home: Option<PathBuf>,
    base_env: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct CaseEnvironment {
    pub env: BTreeMap<String, String>,
    _tempdir: Option<TempDir>,
}

impl SuiteEnvironment {
    pub fn prepare(
        layout: &ProjectLayout,
        config: &EvalConfig,
        runner: &RunnerConfig,
    ) -> Result<Self> {
        let base_env = env::vars().collect::<BTreeMap<_, _>>();
        let codex_home_base_dir = resolve_codex_home_base_dir(runner);
        if !runner.codex_isolation {
            return Ok(Self {
                suite_root: None,
                template_home: None,
                case_homes_dir: None,
                codex_isolation: false,
                codex_home_base_dir,
                codex_home: env::var("CODEX_HOME").ok().map(PathBuf::from),
                base_env,
            });
        }

        fs::create_dir_all(&codex_home_base_dir).map_err(|source| EvelinError::Io {
            path: codex_home_base_dir.clone(),
            source,
        })?;

        let suite_root = tempfile::Builder::new()
            .prefix(&format!(
                "codex-eval-{}-{}-",
                sanitize_name(&config.skill),
                sanitize_name(config.eval_name.as_deref().unwrap_or("eval"))
            ))
            .tempdir_in(&codex_home_base_dir)
            .map_err(|source| {
                EvelinError::Process(format!("failed to create suite tempdir: {source}"))
            })?;
        let template_home = suite_root.path().join("template-home");
        let case_homes_dir = suite_root.path().join("case-homes");
        fs::create_dir_all(&case_homes_dir).map_err(|source| EvelinError::Io {
            path: case_homes_dir.clone(),
            source,
        })?;
        seed_isolated_codex_home(layout, config, &template_home)?;

        let mut base_env = base_env;
        base_env.insert("CODEX_HOME".to_owned(), template_home.display().to_string());

        Ok(Self {
            suite_root: Some(suite_root),
            template_home: Some(template_home.clone()),
            case_homes_dir: Some(case_homes_dir),
            codex_isolation: true,
            codex_home_base_dir,
            codex_home: Some(template_home),
            base_env,
        })
    }

    pub fn case_env(&self, prompt_id: &str) -> Result<CaseEnvironment> {
        if !self.codex_isolation {
            return Ok(CaseEnvironment {
                env: self.base_env.clone(),
                _tempdir: None,
            });
        }
        let case_homes_dir = self
            .case_homes_dir
            .clone()
            .ok_or_else(|| EvelinError::message("missing case_homes_dir"))?;
        let template_home = self
            .template_home
            .clone()
            .ok_or_else(|| EvelinError::message("missing template_home"))?;
        let tempdir = tempfile::Builder::new()
            .prefix(&format!("case-{}-", sanitize_name(prompt_id)))
            .tempdir_in(&case_homes_dir)
            .map_err(|source| {
                EvelinError::Process(format!("failed to create case tempdir: {source}"))
            })?;
        let case_home = tempdir.path().to_path_buf();

        for name in [
            "auth.json",
            ".codex-global-state.json",
            "version.json",
            "models_cache.json",
            "config.toml",
        ] {
            let source = template_home.join(name);
            if source.exists() {
                copy_path(&source, &case_home.join(name))?;
            }
        }
        let skills_src = template_home.join("skills");
        if skills_src.exists() {
            copy_path(&skills_src, &case_home.join("skills"))?;
        }

        let mut env = self.base_env.clone();
        env.insert("CODEX_HOME".to_owned(), case_home.display().to_string());
        Ok(CaseEnvironment {
            env,
            _tempdir: Some(tempdir),
        })
    }

    pub fn suite_root_path(&self) -> Option<&Path> {
        self.suite_root.as_ref().map(TempDir::path)
    }
}

pub fn is_retryable_codex_error(metadata: &ExecMetadata) -> bool {
    let haystack = format!(
        "{} {} {}",
        metadata.error, metadata.stderr_tail, metadata.stdout_tail
    )
    .to_ascii_lowercase();
    [
        "stream disconnected",
        "failed to refresh available models",
        "error sending request for url",
        "reconnecting...",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn resolve_codex_home_base_dir(runner: &RunnerConfig) -> PathBuf {
    if let Some(path) = &runner.codex_home_base_dir {
        return PathBuf::from(path);
    }
    env::temp_dir().join("codex-evals")
}

fn seed_isolated_codex_home(
    layout: &ProjectLayout,
    config: &EvalConfig,
    isolated_home: &Path,
) -> Result<()> {
    fs::create_dir_all(isolated_home.join("skills")).map_err(|source| EvelinError::Io {
        path: isolated_home.join("skills"),
        source,
    })?;
    for name in [
        "auth.json",
        ".codex-global-state.json",
        "version.json",
        "models_cache.json",
    ] {
        let source = default_codex_home().join(name);
        if source.exists() {
            copy_path(&source, &isolated_home.join(name))?;
        }
    }
    fs::write(isolated_home.join("config.toml"), "# Generated by evelin\n").map_err(|source| {
        EvelinError::Io {
            path: isolated_home.join("config.toml"),
            source,
        }
    })?;

    let skill_path = layout.resolve(&config.skill_path);
    let skill_dir = skill_path.parent().ok_or_else(|| {
        EvelinError::message(format!(
            "skill_path has no parent: {}",
            skill_path.display()
        ))
    })?;
    if !skill_path.exists() {
        return Err(EvelinError::message(format!(
            "Skill eval isolation requires existing skill_path: {}",
            skill_path.display()
        )));
    }
    copy_path(skill_dir, &isolated_home.join("skills").join(&config.skill))?;
    Ok(())
}

fn default_codex_home() -> PathBuf {
    if let Ok(path) = env::var("CODEX_HOME") {
        return PathBuf::from(path);
    }
    env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".codex")
}

fn read_exec_response(output_path: &Path, mut metadata: ExecMetadata) -> Result<ExecResponse> {
    let response_text = fs::read_to_string(output_path)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if response_text.is_empty() && metadata.error.is_empty() && metadata.return_code != 0 {
        metadata.error = format!("codex exec failed (rc={})", metadata.return_code);
    }
    Ok(ExecResponse {
        response_text,
        metadata,
    })
}

fn tail(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let len = chars.len();
    chars[len.saturating_sub(500)..].iter().collect()
}

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn copy_path(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        return copy_dir_recursive(source, destination);
    }
    copy_file(source, destination)
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|source_error| EvelinError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    for entry in fs::read_dir(source).map_err(|source_error| EvelinError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| EvelinError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(|source_error| EvelinError::Io {
            path: source_path.clone(),
            source: source_error,
        })?;
        if file_type.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else if file_type.is_symlink() && source_path.is_dir() {
            copy_dir_recursive(&source_path, &destination_path)?;
        } else {
            copy_file(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source_error| EvelinError::Io {
            path: parent.to_path_buf(),
            source: source_error,
        })?;
    }
    fs::copy(source, destination).map_err(|source_error| EvelinError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::config::{effective_runner_config, EvalConfig, ProjectLayout};

    use super::SuiteEnvironment;

    #[test]
    fn isolated_case_env_copies_fixture_skill_into_codex_home() {
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        let skill_path = root.join("skills").join("hello-world").join("SKILL.md");
        fs::create_dir_all(skill_path.parent().expect("skill dir")).expect("skills");
        fs::write(&skill_path, "Hello world!\n").expect("skill");

        let config = EvalConfig::from_value(
            &json!({
                "eval_type": "skill",
                "skill": "hello-world",
                "skill_path": "skills/hello-world/SKILL.md",
                "grader": "markers",
                "rate": 1.0,
                "cases": [{"id":"smoke","prompt":"Use $hello-world skill.","expected":{"must_include":["Hello world!"],"must_not_include":[]}}]
            }),
            root,
        )
        .expect("config");
        let layout = ProjectLayout::discover(root);
        let runner = effective_runner_config(&layout, &config).expect("runner");
        let suite_env = SuiteEnvironment::prepare(&layout, &config, &runner).expect("suite env");

        assert!(suite_env.codex_isolation);

        let case_env = suite_env.case_env("smoke").expect("case env");
        let case_home = case_env
            .env
            .get("CODEX_HOME")
            .map(std::path::PathBuf::from)
            .expect("case CODEX_HOME");
        let linked_skill = case_home
            .join("skills")
            .join("hello-world")
            .join("SKILL.md");

        assert!(linked_skill.exists());
        assert_eq!(
            fs::read_to_string(&linked_skill).expect("copied skill"),
            fs::read_to_string(&skill_path).expect("fixture skill"),
        );
    }
}
