use std::{env, str::FromStr};

use opentelemetry::global;
use opentelemetry_resource_detectors::{HostResourceDetector, OsResourceDetector};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    resource::{Resource, ResourceBuilder},
    trace::SdkTracerProvider,
};
use tracing::Level;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

mod formatting;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub use formatting::{LogFormat, TracingError};

const DEFAULT_LOG_LEVEL: Level = Level::INFO;

pub struct LogConfig {
    pub log_format: LogFormat,
    pub log_level: Level,
}

#[derive(Clone, Debug, Default)]
pub struct WorkerConfig {
    pub log_format: Option<String>,
    pub rust_log: Option<String>,
}

impl LogConfig {
    fn resolve_log_format(config: &WorkerConfig) -> LogFormat {
        [
            config.log_format.as_deref(),
            env::var("LOG_FORMAT").ok().as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|s| LogFormat::from_str(s).ok())
        .unwrap_or_default()
    }

    fn resolve_log_level(config: &WorkerConfig) -> Level {
        [
            config.rust_log.as_deref(),
            env::var("RUST_LOG").ok().as_deref(),
        ]
        .into_iter()
        .flatten()
        .find_map(|s| Level::from_str(s).ok())
        .unwrap_or(DEFAULT_LOG_LEVEL)
    }

    pub fn new(worker_config: WorkerConfig) -> Self {
        Self {
            log_format: Self::resolve_log_format(&worker_config),
            log_level: Self::resolve_log_level(&worker_config),
        }
    }

    pub fn from_env() -> Self {
        Self::new(WorkerConfig::default())
    }

    pub fn create_env_filter(&self) -> impl Fn() -> EnvFilter + '_ {
        move || {
            // Don't show `h2` or `hyper`'s debug logs: they're super verbose
            EnvFilter::builder()
                .with_default_directive(self.log_level.into())
                .from_env_lossy()
                .add_directive("h2=warn".parse().unwrap())
                .add_directive("hyper=warn".parse().unwrap())
        }
    }
}

/// Extract tracing context from HTTP headers.
///
/// This function extracts the distributed tracing context from HTTP headers
/// using the configured global text map propagator.
pub fn extract_context_from_headers<T>(extractor: T) -> opentelemetry::Context
where
    T: opentelemetry::propagation::Extractor,
{
    opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(&extractor))
}

fn create_resource_builder() -> ResourceBuilder {
    Resource::builder()
        .with_detectors(&[
            Box::new(HostResourceDetector::default()),
            Box::new(OsResourceDetector),
        ])
        .with_service_name("rssfilter")
}

/// Initialise `tracing-subscriber` with worker configuration.
///
/// This function configures tracing for both native and WASM targets. Worker environment variables
/// take precedence over system environment variables. This allows runtime configuration via
/// Cloudflare Worker bindings.
///
/// The logging format can be changed with the `LOG_FORMAT` environment variable or worker binding.
/// If set to `JSON`, the log lines will be formatted as JSON objects, otherwise they will be
/// formatted with the default tracing format.
///
/// For WASM targets, OpenTelemetry traces are output to the console. For native targets,
/// OpenTelemetry traces are sent via OTLP.
pub fn init_default_subscriber(
    worker_config: WorkerConfig,
) -> Result<SdkTracerProvider, TracingError> {
    // Set up propagator for context extraction
    global::set_text_map_propagator(TraceContextPropagator::new());

    let config = LogConfig::new(worker_config);

    let env_filter = config.create_env_filter();

    tracing_subscriber::registry()
        .with(config.create_otel_layer()?)
        .with(config.create_fmt_layer())
        .with(env_filter())
        .init();

    config.create_tracer_provider()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TraceContextExt;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use temp_env::with_var;
    use test_case::test_case;
    use tracing::{error, info, warn};
    use tracing_subscriber::{fmt::MakeWriter, layer::SubscriberExt};

    // Custom writer that captures output for testing
    #[derive(Clone)]
    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn new() -> Self {
            Self {
                buffer: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn get_string(&self) -> String {
            let buffer = self.buffer.lock().unwrap();
            String::from_utf8_lossy(&buffer).to_string()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut buffer = self.buffer.lock().unwrap();
            buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[test_case("pretty", LogFormat::Pretty; "pretty lowercase")]
    #[test_case("PRETTY", LogFormat::Pretty; "pretty uppercase")]
    #[test_case("json", LogFormat::Json; "json lowercase")]
    #[test_case("JSON", LogFormat::Json; "json uppercase")]
    fn test_log_format_from_str_valid(input: &str, expected: LogFormat) {
        assert_eq!(LogFormat::from_str(input).unwrap(), expected);
    }

    #[test_case("invalid"; "invalid format")]
    #[test_case(""; "empty string")]
    #[test_case("xml"; "unsupported format")]
    fn test_log_format_from_str_invalid(input: &str) {
        assert!(LogFormat::from_str(input).is_err());
    }

    #[test_case(None, None, None, None, LogFormat::default(), Level::INFO; "all unset")]
    #[test_case(Some("json"), Some("debug"), None, None, LogFormat::Json, Level::DEBUG; "env vars only")]
    #[test_case(None, None, Some("pretty"), Some("warn"), LogFormat::Pretty, Level::WARN; "worker vars only")]
    #[test_case(Some("json"), Some("debug"), Some("pretty"), Some("warn"), LogFormat::Pretty, Level::WARN; "worker vars override env vars")]
    #[test_case(Some("json"), Some("debug"), Some("pretty"), None, LogFormat::Pretty, Level::DEBUG; "worker log_format overrides, env rust_log used")]
    #[test_case(Some("json"), Some("debug"), None, Some("warn"), LogFormat::Json, Level::WARN; "env log_format used, worker rust_log overrides")]
    #[test_case(Some("invalid"), Some("invalid"), Some("json"), Some("error"), LogFormat::Json, Level::ERROR; "worker vars override invalid env vars")]
    #[test_case(Some("json"), Some("debug"), Some("invalid"), Some("invalid"), LogFormat::Json, Level::DEBUG; "fallback to env when worker vars invalid")]
    fn test_log_config_worker_precedence(
        env_log_format: Option<&'static str>,
        env_rust_log: Option<&'static str>,
        worker_log_format: Option<&'static str>,
        worker_rust_log: Option<&'static str>,
        expected_format: LogFormat,
        expected_level: Level,
    ) {
        with_var("LOG_FORMAT", env_log_format, || {
            with_var("RUST_LOG", env_rust_log, || {
                let worker_config = WorkerConfig {
                    log_format: worker_log_format.map(String::from),
                    rust_log: worker_rust_log.map(String::from),
                };
                let config = LogConfig::new(worker_config);
                assert_eq!(config.log_format, expected_format);
                assert_eq!(config.log_level, expected_level);
            });
        });
    }

    #[test]
    fn test_env_filter_creation() {
        let config = LogConfig {
            log_format: LogFormat::Pretty,
            log_level: Level::WARN,
        };

        let env_filter = config.create_env_filter();
        let filter = env_filter();

        assert!(filter.to_string().contains("warn"));
    }

    #[test]
    fn test_writer_injection() {
        let config = LogConfig {
            log_format: LogFormat::Json,
            log_level: Level::INFO,
        };

        let writer = CaptureWriter::new();
        let layer = config.create_fmt_layer_with_writer(writer.clone());

        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            info!("Test message for writer injection");
        });

        let output = writer.get_string();
        assert!(output.contains("Test message for writer injection"));

        let lines: Vec<&str> = output.trim().split('\n').collect();
        assert!(!lines.is_empty(), "Should have at least one log line");

        let json: Value = serde_json::from_str(lines[0]).expect("Should be valid JSON");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .contains("Test message for writer injection")
        );
        assert_eq!(json["level"].as_str().unwrap(), "INFO");
    }

    #[test]
    fn test_extract_context_from_headers() {
        init_default_subscriber(WorkerConfig::default()).expect("Failed to initialise subscriber");

        let mut headers = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-00000000000000000000000000000001-0000000000000001-01".to_string(),
        );

        let context = extract_context_from_headers(headers);

        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid());
        assert!(span_context.is_remote(), "Span context should be remote");
        assert_eq!(
            span_context.trace_id().to_string(),
            "00000000000000000000000000000001"
        );
        assert_eq!(span_context.span_id().to_string(), "0000000000000001");
        // `-01` in `traceparent` indicates that the span is sampled
        assert!(span_context.is_sampled(), "Span context should be sampled");
    }

    #[test]
    fn test_tracing_error_display() {
        let error = TracingError::OtlpError("test error".to_string());
        assert_eq!(error.to_string(), "OTLP error: test error");
    }

    #[test]
    fn test_integration_with_actual_logging() {
        let config = LogConfig {
            log_format: LogFormat::Json,
            log_level: Level::INFO,
        };

        let writer = CaptureWriter::new();
        let layer = config.create_fmt_layer_with_writer(writer.clone());
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            info!("Test info message");
            warn!("Test warning message");
            error!("Test error message");
        });

        let output = writer.get_string();
        let lines: Vec<&str> = output
            .trim()
            .split('\n')
            .filter(|line| !line.is_empty())
            .collect();
        assert_eq!(lines.len(), 3, "Should have exactly 3 log lines");

        let mut levels = Vec::new();
        let mut messages = Vec::new();

        for line in lines {
            let json: Value = serde_json::from_str(line).expect("Each line should be valid JSON");

            assert!(json["timestamp"].is_string(), "Should have timestamp");
            assert!(json["level"].is_string(), "Should have level");
            assert!(json["message"].is_string(), "Should have message");

            levels.push(json["level"].as_str().unwrap().to_string());
            messages.push(json["message"].as_str().unwrap().to_string());
        }

        // Verify all expected levels are present
        assert!(levels.contains(&"INFO".to_string()));
        assert!(levels.contains(&"WARN".to_string()));
        assert!(levels.contains(&"ERROR".to_string()));

        // Verify all expected messages are present
        assert!(messages.iter().any(|m| m.contains("Test info message")));
        assert!(messages.iter().any(|m| m.contains("Test warning message")));
        assert!(messages.iter().any(|m| m.contains("Test error message")));
    }
}
