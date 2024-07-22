use std::{env, str::FromStr};

use lambda_runtime::Error as LambdaError;
use opentelemetry::trace::TracerProvider;
use opentelemetry::{global, KeyValue};
use opentelemetry_aws::trace::{XrayIdGenerator, XrayPropagator};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{
    runtime,
    trace::{self, Sampler},
    Resource,
};
use tracing::Level;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{self, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

const DEFAULT_LOG_LEVEL: &str = "INFO";

/// Initialize `tracing-subscriber` with default options.
///
/// This function uses environment variables set with [Lambda's advanced logging
/// controls](https://aws.amazon.com/blogs/compute/introducing-advanced-logging-controls-for-aws-lambda-functions/)
/// if they're configured for your function.
///
/// This subscriber sets the logging level based on environment variables:
///     - if `AWS_LAMBDA_LOG_LEVEL` is set, it takes predecence over any other environment variables.
///     - if `AWS_LAMBDA_LOG_LEVEL` is not set, check if `RUST_LOG` is set.
///     - if none of those two variables are set, use `INFO` as the logging level.
///
/// The logging format can also be changed based on Lambda's advanced logging
/// controls.  If the `AWS_LAMBDA_LOG_FORMAT` environment variable is set to
/// `JSON`, the log lines will be formatted as json objects, otherwise they will
/// be formatted with the default tracing format.
///
/// This was [copied from `lambda_runtime_api_client`][copied-code] and modified
/// to add an OTLP exporter for sending traces to the OTLP receiver, running as
/// a Lambda layer sending traces to AWS X-Ray.
///
/// [copied-code]: https://github.com/awslabs/aws-lambda-rust-runtime/blob/92cdd74b2aa4b5397f7ff4f1800b54c9b949d96a/lambda-runtime-api-client/src/tracing.rs#L20-L52
pub fn init_default_subscriber() -> Result<opentelemetry_sdk::trace::TracerProvider, LambdaError> {
    global::set_text_map_propagator(XrayPropagator::default());

    let log_format = env::var("AWS_LAMBDA_LOG_FORMAT").unwrap_or_default();
    let log_level_str = env::var("AWS_LAMBDA_LOG_LEVEL").or_else(|_| env::var("RUST_LOG"));
    let log_level = Level::from_str(log_level_str.as_deref().unwrap_or(DEFAULT_LOG_LEVEL))
        .unwrap_or(Level::INFO);

    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint("http://localhost:4317");

    let trace_config = trace::Config::default()
        .with_resource(Resource::new(vec![KeyValue::new(
            "service.name",
            "lambda-rssfilter",
        )]))
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(XrayIdGenerator::default());

    let tracer_provider = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_trace_config(trace_config)
        .with_exporter(exporter)
        .install_batch(runtime::Tokio)
        .unwrap();

    let tracer = tracer_provider.tracer("lambda-rssfilter");

    let env_filter = || {
        // Don't show `h2` or `hyper`'s debug logs: they're super verbose
        EnvFilter::builder()
            .with_default_directive(log_level.into())
            .from_env_lossy()
            .add_directive("h2=warn".parse().unwrap())
            .add_directive("hyper=warn".parse().unwrap())
    };

    // Create a layer for sending traces to the OTLP receiver
    let traces_layer = OpenTelemetryLayer::new(tracer).with_filter(env_filter());

    let fmt_layer_base = tracing_subscriber::fmt::layer();

    let stdout_layer = (if log_format.eq_ignore_ascii_case("json") {
        fmt_layer_base.json().flatten_event(true).boxed()
    } else {
        fmt_layer_base.boxed()
    })
    .with_filter(env_filter());

    tracing_subscriber::registry()
        .with(traces_layer)
        .with(stdout_layer)
        .init();

    Ok(tracer_provider)
}
