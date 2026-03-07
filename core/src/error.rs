use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EvelinError {
    #[error("{0}")]
    Message(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json parse error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("yaml parse error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("toml parse error at {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("process error: {0}")]
    Process(String),
}

pub type Result<T> = std::result::Result<T, EvelinError>;

impl EvelinError {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}
