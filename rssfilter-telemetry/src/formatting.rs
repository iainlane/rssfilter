use std::io::IsTerminal;
use std::str::FromStr;
use thiserror::Error;

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

#[derive(Debug, Error)]
pub enum TracingError {
    #[error("OTLP error: {0}")]
    OtlpError(String),

    #[cfg(not(target_arch = "wasm32"))]
    #[error("Failed to create OTLP exporter: {0}")]
    ExporterBuild(#[from] opentelemetry_otlp::ExporterBuildError),
}
