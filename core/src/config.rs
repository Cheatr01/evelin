use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::document::{as_object, load_document, load_jsonl_or_stream, resolve_path};
use crate::error::{EvelinError, Result};
use crate::schema;

pub const DEFAULT_CODEX_HOME_BASE_ENV: &str = "EVAL_CODEX_HOME_BASE_DIR";
pub const DEFAULT_EVAL_CONFIG_TOML: &str = include_str!("eval-config.toml");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalType {
    Skill,
}

impl EvalType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Skill => "skill",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerExpectations {
    pub must_include: Vec<String>,
    pub must_not_include: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptCase {
    pub id: String,
    pub prompt: String,
    pub expected: MarkerExpectations,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRequirements {
    pub required_snippets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvalConfig {
    pub eval_type: EvalType,
    pub skill: String,
    pub skill_path: String,
    pub gate_requirements: Option<GateRequirements>,
    pub eval_name: Option<String>,
    pub grader: String,
    pub rate: f64,
    pub max_concurrency: Option<usize>,
    pub codex_timeout_seconds: Option<u64>,
    pub codex_isolation: Option<bool>,
    pub codex_home_base_dir: Option<String>,
    pub codex_sandbox: Option<String>,
    pub codex_model: Option<String>,
    pub codex_reasoning_effort: Option<String>,
    pub codex_extra_args: Option<Vec<String>>,
    pub cases: Vec<PromptCase>,
    pub prompts_file: Option<String>,
}

impl EvalConfig {
    pub fn from_path(path: &Path, root: &Path) -> Result<Self> {
        let value = load_document(path)?;
        let errors = schema::validate_suite_document(path, &value);
        if !errors.is_empty() {
            return Err(EvelinError::message(format!(
                "Schema validation failed: {}",
                errors.join(" | ")
            )));
        }
        Self::from_value(&value, root)
    }

    pub fn from_value(value: &Value, root: &Path) -> Result<Self> {
        let object = as_object(value, "eval config")?;
        let eval_type = match required_string(object, "eval_type")?.as_str() {
            "skill" => EvalType::Skill,
            other => {
                return Err(EvelinError::message(format!(
                    "unsupported eval_type '{other}'"
                )))
            }
        };
        let gate_requirements = object
            .get("gate_requirements")
            .map(parse_gate_requirements)
            .transpose()?;
        let prompts_file = optional_string(object, "prompts_file")?;
        let cases = if let Some(path) = &prompts_file {
            load_cases_from_prompts_file(&resolve_path(root, path), root)?
        } else {
            parse_cases(object.get("cases").ok_or_else(|| {
                EvelinError::message("Eval config requires 'cases' or 'prompts_file'")
            })?)?
        };
        if cases.is_empty() && prompts_file.is_none() {
            return Err(EvelinError::message(
                "cases must not be empty when prompts_file is absent",
            ));
        }

        Ok(Self {
            eval_type,
            skill: required_string(object, "skill")?,
            skill_path: required_string(object, "skill_path")?,
            gate_requirements,
            eval_name: optional_string(object, "eval_name")?,
            grader: required_string(object, "grader")?,
            rate: required_f64(object, "rate")?,
            max_concurrency: optional_usize(object, "max_concurrency")?,
            codex_timeout_seconds: optional_u64(object, "codex_timeout_seconds")?,
            codex_isolation: optional_bool(object, "codex_isolation")?,
            codex_home_base_dir: optional_string(object, "codex_home_base_dir")?,
            codex_sandbox: optional_string(object, "codex_sandbox")?,
            codex_model: optional_string(object, "codex_model")?,
            codex_reasoning_effort: optional_string(object, "codex_reasoning_effort")?,
            codex_extra_args: optional_string_list(object, "codex_extra_args")?,
            cases,
            prompts_file,
        })
    }

    pub fn effective_eval_name(&self, config_path: &Path) -> String {
        if let Some(name) = &self.eval_name {
            if !name.trim().is_empty() {
                return name.trim().to_owned();
            }
        }
        let file_name = config_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("eval");
        if is_default_eval_config(file_name) {
            return "eval".to_owned();
        }
        strip_eval_suffix(file_name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunnerDefaults {
    pub max_concurrency: usize,
    pub codex_timeout_seconds: u64,
    pub codex_sandbox: String,
    pub codex_model: Option<String>,
    pub codex_reasoning_effort: Option<String>,
    pub codex_extra_args: Vec<String>,
    pub codex_isolation: bool,
    pub codex_home_base_dir: Option<String>,
}

impl Default for RunnerDefaults {
    fn default() -> Self {
        Self {
            max_concurrency: 3,
            codex_timeout_seconds: 180,
            codex_sandbox: "read-only".to_owned(),
            codex_model: None,
            codex_reasoning_effort: None,
            codex_extra_args: Vec::new(),
            codex_isolation: false,
            codex_home_base_dir: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GlobalEvalConfig {
    pub defaults: RunnerDefaultsPatch,
    pub eval_type: BTreeMap<String, RunnerDefaultsPatch>,
}

impl GlobalEvalConfig {
    pub fn from_toml_str(text: &str, path: &Path) -> Result<Self> {
        let toml = toml::from_str::<toml::Value>(text).map_err(|source| EvelinError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
        let value = serde_json::to_value(toml).map_err(|source| {
            EvelinError::message(format!(
                "toml conversion error at {}: {source}",
                path.display()
            ))
        })?;
        ProjectLayout::parse_global_eval_config_value(&value)
    }

    pub fn apply_overlay(&mut self, overlay: Self) {
        self.defaults.merge_from(&overlay.defaults);
        for (eval_type, patch) in overlay.eval_type {
            self.eval_type
                .entry(eval_type)
                .or_default()
                .merge_from(&patch);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RunnerDefaultsPatch {
    pub max_concurrency: Option<usize>,
    pub codex_timeout_seconds: Option<u64>,
    pub codex_sandbox: Option<String>,
    pub codex_model: Option<String>,
    pub codex_reasoning_effort: Option<String>,
    pub codex_extra_args: Option<Vec<String>>,
    pub codex_isolation: Option<bool>,
    pub codex_home_base_dir: Option<String>,
}

impl RunnerDefaultsPatch {
    pub fn apply_to(&self, defaults: &mut RunnerDefaults) {
        if let Some(value) = self.max_concurrency {
            defaults.max_concurrency = value;
        }
        if let Some(value) = self.codex_timeout_seconds {
            defaults.codex_timeout_seconds = value;
        }
        if let Some(value) = &self.codex_sandbox {
            defaults.codex_sandbox = value.clone();
        }
        if let Some(value) = &self.codex_model {
            defaults.codex_model = if value.trim().is_empty() {
                None
            } else {
                Some(value.clone())
            };
        }
        if let Some(value) = &self.codex_reasoning_effort {
            defaults.codex_reasoning_effort = if value.trim().is_empty() {
                None
            } else {
                Some(value.clone())
            };
        }
        if let Some(value) = &self.codex_extra_args {
            defaults.codex_extra_args = value.clone();
        }
        if let Some(value) = self.codex_isolation {
            defaults.codex_isolation = value;
        }
        if let Some(value) = &self.codex_home_base_dir {
            defaults.codex_home_base_dir = if value.trim().is_empty() {
                None
            } else {
                Some(value.clone())
            };
        }
    }

    pub fn merge_from(&mut self, other: &Self) {
        if other.max_concurrency.is_some() {
            self.max_concurrency = other.max_concurrency;
        }
        if other.codex_timeout_seconds.is_some() {
            self.codex_timeout_seconds = other.codex_timeout_seconds;
        }
        if let Some(value) = &other.codex_sandbox {
            self.codex_sandbox = Some(value.clone());
        }
        if let Some(value) = &other.codex_model {
            self.codex_model = Some(value.clone());
        }
        if let Some(value) = &other.codex_reasoning_effort {
            self.codex_reasoning_effort = Some(value.clone());
        }
        if let Some(value) = &other.codex_extra_args {
            self.codex_extra_args = Some(value.clone());
        }
        if other.codex_isolation.is_some() {
            self.codex_isolation = other.codex_isolation;
        }
        if let Some(value) = &other.codex_home_base_dir {
            self.codex_home_base_dir = Some(value.clone());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerConfig {
    pub max_concurrency: usize,
    pub codex_timeout_seconds: u64,
    pub codex_sandbox: String,
    pub codex_model: Option<String>,
    pub codex_reasoning_effort: Option<String>,
    pub codex_extra_args: Vec<String>,
    pub codex_isolation: bool,
    pub codex_home_base_dir: Option<String>,
}

impl RunnerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.max_concurrency == 0 {
            return Err(EvelinError::message(
                "Effective runtime config field 'max_concurrency' must be >= 1",
            ));
        }
        if self.codex_timeout_seconds == 0 {
            return Err(EvelinError::message(
                "Effective runtime config field 'codex_timeout_seconds' must be >= 1",
            ));
        }
        if !matches!(
            self.codex_sandbox.as_str(),
            "read-only" | "workspace-write" | "danger-full-access"
        ) {
            return Err(EvelinError::message(
                "Effective runtime config field 'codex_sandbox' is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLayout {
    pub root: PathBuf,
    pub skills_dir: PathBuf,
    pub tests_skills_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub global_eval_config_path: PathBuf,
}

impl ProjectLayout {
    pub fn discover(root: impl Into<PathBuf>) -> Self {
        let root = normalize_root_path(root.into());
        Self {
            skills_dir: root.join("skills"),
            tests_skills_dir: root.join("tests").join("src").join("skills"),
            assets_dir: root.join(".evelin"),
            global_eval_config_path: root.join("eval-config.toml"),
            root,
        }
    }

    pub fn resolve(&self, value: impl AsRef<str>) -> PathBuf {
        resolve_path(&self.root, value)
    }

    pub fn load_global_eval_config(&self) -> Result<GlobalEvalConfig> {
        let mut global = GlobalEvalConfig::from_toml_str(
            DEFAULT_EVAL_CONFIG_TOML,
            Path::new("core/src/eval-config.toml"),
        )?;
        if self.global_eval_config_path.exists() {
            let value = load_document(&self.global_eval_config_path)?;
            global.apply_overlay(Self::parse_global_eval_config_value(&value)?);
        }
        Ok(global)
    }

    fn parse_global_eval_config_value(value: &Value) -> Result<GlobalEvalConfig> {
        let object = as_object(value, "global eval config")?;
        let defaults = object
            .get("defaults")
            .map(parse_patch)
            .transpose()?
            .unwrap_or_default();
        let mut eval_type = BTreeMap::new();
        if let Some(per_type) = object.get("eval_type") {
            let per_type_object = as_object(per_type, "[eval_type]")?;
            for (key, patch_value) in per_type_object {
                eval_type.insert(key.clone(), parse_patch(patch_value)?);
            }
        }
        Ok(GlobalEvalConfig {
            defaults,
            eval_type,
        })
    }
}

fn normalize_root_path(root: PathBuf) -> PathBuf {
    let absolute = if root.is_absolute() {
        root
    } else {
        env::current_dir()
            .map(|cwd| cwd.join(&root))
            .unwrap_or(root)
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}

pub fn effective_runner_config(
    layout: &ProjectLayout,
    config: &EvalConfig,
) -> Result<RunnerConfig> {
    let mut defaults = RunnerDefaults::default();
    let global = layout.load_global_eval_config()?;
    global.defaults.apply_to(&mut defaults);
    if let Some(per_type) = global.eval_type.get(config.eval_type.as_str()) {
        per_type.apply_to(&mut defaults);
    }

    let inline_patch = RunnerDefaultsPatch {
        max_concurrency: config.max_concurrency,
        codex_timeout_seconds: config.codex_timeout_seconds,
        codex_sandbox: config.codex_sandbox.clone(),
        codex_model: config.codex_model.clone(),
        codex_reasoning_effort: config.codex_reasoning_effort.clone(),
        codex_extra_args: config.codex_extra_args.clone(),
        codex_isolation: config.codex_isolation,
        codex_home_base_dir: config.codex_home_base_dir.clone(),
    };
    inline_patch.apply_to(&mut defaults);

    if let Ok(home_base) = env::var(DEFAULT_CODEX_HOME_BASE_ENV) {
        if !home_base.trim().is_empty() {
            defaults.codex_home_base_dir = Some(home_base);
        }
    }

    let runner = RunnerConfig {
        max_concurrency: defaults.max_concurrency,
        codex_timeout_seconds: defaults.codex_timeout_seconds,
        codex_sandbox: defaults.codex_sandbox,
        codex_model: defaults.codex_model,
        codex_reasoning_effort: defaults.codex_reasoning_effort,
        codex_extra_args: defaults.codex_extra_args,
        codex_isolation: defaults.codex_isolation,
        codex_home_base_dir: defaults.codex_home_base_dir,
    };
    runner.validate()?;
    Ok(runner)
}

pub fn sanitize_name(value: &str) -> String {
    Regex::new(r"[^A-Za-z0-9._-]+")
        .expect("regex")
        .replace_all(value.trim(), "-")
        .trim_matches('-')
        .to_string()
        .chars()
        .collect::<String>()
        .if_empty("eval")
}

trait StringExt {
    fn if_empty(self, fallback: &str) -> String;
}

impl StringExt for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

pub fn strip_eval_suffix(file_name: &str) -> String {
    [".eval_config.json", ".eval.yaml", ".eval.yml"]
        .iter()
        .find_map(|suffix| file_name.strip_suffix(suffix))
        .unwrap_or(file_name)
        .to_owned()
}

pub fn is_default_eval_config(file_name: &str) -> bool {
    matches!(file_name, "eval_config.json" | "eval.yaml" | "eval.yml")
}

fn load_cases_from_prompts_file(path: &Path, root: &Path) -> Result<Vec<PromptCase>> {
    let rows = load_jsonl_or_stream(path)?;
    let mut cases = Vec::new();
    for (index, value) in rows.iter().enumerate() {
        cases.push(parse_case(
            value,
            index + 1,
            &format!("prompts_file:{}", path.display()),
            root,
        )?);
    }
    if cases.is_empty() {
        return Err(EvelinError::message(format!(
            "prompts_file is empty: {}",
            path.display()
        )));
    }
    Ok(cases)
}

fn parse_cases(value: &Value) -> Result<Vec<PromptCase>> {
    let rows = value
        .as_array()
        .ok_or_else(|| EvelinError::message("cases must be an array"))?;
    let mut cases = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        cases.push(parse_case(row, index + 1, "cases", Path::new("."))?);
    }
    Ok(cases)
}

fn parse_case(value: &Value, index: usize, source: &str, _root: &Path) -> Result<PromptCase> {
    let object = as_object(value, &format!("{source} case #{index}"))?;
    let id = required_string(object, "id")?;
    let prompt = required_string(object, "prompt")?;
    let expected = object
        .get("expected")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let expected_object = as_object(&expected, &format!("{source} case '{id}' field 'expected'"))?;

    Ok(PromptCase {
        id: id.clone(),
        prompt,
        expected: MarkerExpectations {
            must_include: optional_string_list(expected_object, "must_include")?
                .unwrap_or_default(),
            must_not_include: optional_string_list(expected_object, "must_not_include")?
                .unwrap_or_default(),
        },
    })
}

fn parse_gate_requirements(value: &Value) -> Result<GateRequirements> {
    let object = as_object(value, "gate_requirements")?;
    Ok(GateRequirements {
        required_snippets: optional_string_list(object, "required_snippets")?.unwrap_or_default(),
    })
}

fn parse_patch(value: &Value) -> Result<RunnerDefaultsPatch> {
    let object = as_object(value, "runner patch")?;
    Ok(RunnerDefaultsPatch {
        max_concurrency: optional_usize(object, "max_concurrency")?,
        codex_timeout_seconds: optional_u64(object, "codex_timeout_seconds")?,
        codex_sandbox: optional_string(object, "codex_sandbox")?,
        codex_model: optional_string(object, "codex_model")?,
        codex_reasoning_effort: optional_string(object, "codex_reasoning_effort")?,
        codex_extra_args: optional_string_list(object, "codex_extra_args")?,
        codex_isolation: optional_bool(object, "codex_isolation")?,
        codex_home_base_dir: optional_string(object, "codex_home_base_dir")?,
    })
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String> {
    let value = object
        .get(key)
        .ok_or_else(|| EvelinError::message(format!("missing required field '{key}'")))?;
    let string = value
        .as_str()
        .ok_or_else(|| EvelinError::message(format!("field '{key}' must be a string")))?;
    if string.trim().is_empty() {
        return Err(EvelinError::message(format!(
            "field '{key}' must not be empty"
        )));
    }
    Ok(string.to_owned())
}

fn required_f64(object: &Map<String, Value>, key: &str) -> Result<f64> {
    object
        .get(key)
        .and_then(Value::as_f64)
        .ok_or_else(|| EvelinError::message(format!("field '{key}' must be a number")))
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match object.get(key) {
        Some(value) => {
            let string = value
                .as_str()
                .ok_or_else(|| EvelinError::message(format!("field '{key}' must be a string")))?;
            Ok(Some(string.to_owned()))
        }
        None => Ok(None),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match object.get(key) {
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| EvelinError::message(format!("field '{key}' must be a boolean"))),
        None => Ok(None),
    }
}

fn optional_u64(object: &Map<String, Value>, key: &str) -> Result<Option<u64>> {
    match object.get(key) {
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| EvelinError::message(format!("field '{key}' must be an integer"))),
        None => Ok(None),
    }
}

fn optional_usize(object: &Map<String, Value>, key: &str) -> Result<Option<usize>> {
    optional_u64(object, key).map(|value| value.map(|v| v as usize))
}

fn optional_string_list(object: &Map<String, Value>, key: &str) -> Result<Option<Vec<String>>> {
    match object.get(key) {
        Some(value) => {
            let rows = value
                .as_array()
                .ok_or_else(|| EvelinError::message(format!("field '{key}' must be an array")))?;
            let mut out = Vec::new();
            for item in rows {
                let string = item.as_str().ok_or_else(|| {
                    EvelinError::message(format!("field '{key}' must contain only strings"))
                })?;
                out.push(string.to_owned());
            }
            Ok(Some(out))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn computes_effective_eval_name() {
        let config = EvalConfig::from_value(
            &json!({
                "eval_type": "skill",
                "skill": "hello-world",
                "skill_path": "skills/hello-world/SKILL.md",
                "grader": "markers",
                "rate": 0.875,
                "cases": [{"id":"a","prompt":"b","expected":{"must_include":[],"must_not_include":[]}}]
            }),
            Path::new("."),
        )
        .expect("config");

        assert_eq!(
            config.effective_eval_name(Path::new("suite.eval.yaml")),
            "suite"
        );
        assert_eq!(config.effective_eval_name(Path::new("eval.yaml")), "eval");
    }

    #[test]
    fn merges_global_and_inline_runner_config() {
        let tmp = TempDir::new().expect("tmp");
        fs::write(
            tmp.path().join("eval-config.toml"),
            r#"
[defaults]
max_concurrency = 5
codex_timeout_seconds = 90
codex_sandbox = "workspace-write"
codex_extra_args = ["--foo"]
codex_isolation = false

[eval_type.skill]
codex_isolation = true
"#,
        )
        .expect("eval config");

        let config = EvalConfig::from_value(
            &json!({
                "eval_type": "skill",
                "skill": "hello-world",
                "skill_path": "skills/hello-world/SKILL.md",
                "grader": "markers",
                "rate": 0.875,
                "max_concurrency": 2,
                "cases": [{"id":"a","prompt":"b","expected":{"must_include":[],"must_not_include":[]}}]
            }),
            tmp.path(),
        )
        .expect("config");

        let runner =
            effective_runner_config(&ProjectLayout::discover(tmp.path()), &config).expect("runner");
        assert_eq!(runner.max_concurrency, 2);
        assert_eq!(runner.codex_timeout_seconds, 90);
        assert_eq!(runner.codex_sandbox, "workspace-write");
        assert_eq!(runner.codex_extra_args, vec!["--foo".to_owned()]);
        assert!(runner.codex_isolation);
    }

    #[test]
    fn uses_bundled_skill_eval_defaults_without_project_eval_config() {
        let tmp = TempDir::new().expect("tmp");
        let config = EvalConfig::from_value(
            &json!({
                "eval_type": "skill",
                "skill": "hello-world",
                "skill_path": "skills/hello-world/SKILL.md",
                "grader": "markers",
                "rate": 0.875,
                "cases": [{"id":"a","prompt":"b","expected":{"must_include":[],"must_not_include":[]}}]
            }),
            tmp.path(),
        )
        .expect("config");

        let runner =
            effective_runner_config(&ProjectLayout::discover(tmp.path()), &config).expect("runner");
        assert_eq!(runner.max_concurrency, 3);
        assert_eq!(runner.codex_timeout_seconds, 180);
        assert_eq!(runner.codex_sandbox, "read-only");
        assert!(runner.codex_isolation);
    }

    #[test]
    fn discovers_relative_root_as_absolute_path() {
        let cwd = env::current_dir().expect("cwd");
        let tmp = tempfile::tempdir_in(&cwd).expect("tmp in cwd");
        let relative_root = tmp
            .path()
            .strip_prefix(&cwd)
            .expect("relative root")
            .to_path_buf();

        let layout = ProjectLayout::discover(relative_root);

        assert!(layout.root.is_absolute());
        assert_eq!(
            fs::canonicalize(&layout.root).expect("layout root"),
            fs::canonicalize(tmp.path()).expect("tmp root"),
        );
    }
}
