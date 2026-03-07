use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{EvelinError, Result};

pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| EvelinError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load_document(path: &Path) -> Result<Value> {
    let text = read_text(path)?;
    match extension(path).as_deref() {
        Some("json") => serde_json::from_str::<Value>(&text).map_err(|source| EvelinError::Json {
            path: path.to_path_buf(),
            source,
        }),
        Some("yaml" | "yml") => {
            let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text).map_err(|source| {
                EvelinError::Yaml {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
            serde_json::to_value(yaml).map_err(|source| {
                EvelinError::message(format!(
                    "yaml conversion error at {}: {source}",
                    path.display()
                ))
            })
        }
        Some("toml") => {
            let toml =
                toml::from_str::<toml::Value>(&text).map_err(|source| EvelinError::Toml {
                    path: path.to_path_buf(),
                    source,
                })?;
            serde_json::to_value(toml).map_err(|source| {
                EvelinError::message(format!(
                    "toml conversion error at {}: {source}",
                    path.display()
                ))
            })
        }
        Some(other) => Err(EvelinError::message(format!(
            "unsupported config extension for {}: .{}",
            path.display(),
            other
        ))),
        None => Err(EvelinError::message(format!(
            "missing config extension for {}",
            path.display()
        ))),
    }
}

pub fn load_jsonl_or_stream(path: &Path) -> Result<Vec<Value>> {
    let text = read_text(path)?;
    let mut line_rows = Vec::new();
    let mut line_mode_ok = true;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) => line_rows.push(value),
            Err(_) => {
                line_mode_ok = false;
                break;
            }
        }
    }

    if line_mode_ok {
        return Ok(line_rows);
    }

    let mut rows = Vec::new();
    let stream = serde_json::Deserializer::from_str(&text).into_iter::<Value>();
    for item in stream {
        rows.push(item.map_err(|source| EvelinError::Json {
            path: path.to_path_buf(),
            source,
        })?);
    }
    Ok(rows)
}

pub fn as_object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| EvelinError::message(format!("{context} must be an object")))
}

pub fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
}

pub fn resolve_path(root: &Path, value: impl AsRef<str>) -> PathBuf {
    let path = PathBuf::from(value.as_ref());
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}
