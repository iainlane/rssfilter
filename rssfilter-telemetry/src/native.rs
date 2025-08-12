use std::env;

use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_resource_detectors::ProcessResourceDetector;
use opentelemetry_sdk::{
    Resource,
    trace::{RandomIdGenerator, Sampler, SdkTracerProvider},
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{
    Layer,
    fmt::{MakeWriter, layer},
    registry::LookupSpan,
};

use crate::{LogConfig, LogFormat, TracingError, create_resource_builder};

impl LogConfig {
    pub fn create_fmt_layer<S>(&self) -> impl Layer<S> + Send + Sync
    where
        S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        self.create_fmt_layer_with_writer(std::io::stdout)
    }

    pub fn create_fmt_layer_with_writer<S, W>(&self, writer: W) -> impl Layer<S> + Send + Sync
    where
        S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup>,
        W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
    {
        let fmt_layer_base = layer().with_writer(writer);

        match self.log_format {
            LogFormat::Json => fmt_layer_base.json().flatten_event(true).boxed(),
            LogFormat::Pretty => fmt_layer_base.pretty().boxed(),
        }
    }

    pub fn create_tracer_provider(&self) -> Result<SdkTracerProvider, TracingError> {
        let otlp_endpoint = env::var("OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318/v1/traces".to_string());

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(otlp_endpoint)
            .build()?;

        let service_name =
            env::var("SERVICE_NAME").unwrap_or_else(|_| "cloudflare-worker".to_string());

        let resource = create_resource_builder()
            .with_detector(Box::new(ProcessResourceDetector))
            .build();

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(
                Resource::builder_empty()
                    .with_attributes([KeyValue::new("service.name", service_name)])
                    .build(),
            )
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(RandomIdGenerator::default())
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();

        Ok(tracer_provider)
    }

    pub fn create_otel_layer<S>(&self) -> Result<impl Layer<S> + Send + Sync, TracingError>
    where
        S: tracing::Subscriber + for<'lookup> LookupSpan<'lookup> + Send + Sync,
    {
        let tracer_provider = self.create_tracer_provider()?;
        let tracer = tracer_provider.tracer("cloudflare-worker");

        Ok(OpenTelemetryLayer::new(tracer))
    }
}
