use std::io::IsTerminal;
use std::{fmt, str::FromStr};

#[derive(Debug, Clone, PartialEq)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl Default for LogFormat {
    fn default() -> Self {
        if std::io::stdout().is_terminal() {
            LogFormat::Pretty
        } else {
            LogFormat::Json
        }
    }
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pretty" => Ok(LogFormat::Pretty),
            "json" => Ok(LogFormat::Json),
            _ => Err(format!(
                "Invalid log format: '{s}'. Valid options are 'pretty' or 'json'"
            )),
        }
    }
}

#[derive(Debug)]
pub enum TracingError {
    OtlpError(String),
}

impl fmt::Display for TracingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TracingError::OtlpError(msg) => write!(f, "OTLP error: {msg}"),
        }
    }
}

impl std::error::Error for TracingError {}
