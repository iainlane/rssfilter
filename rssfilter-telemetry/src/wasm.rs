use std::fmt::Result as FmtResult;

use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::{
    Event, Subscriber,
    span::{Id as SpanID, Record as SpanRecord},
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{
    Layer,
    fmt::time::FormatTime,
    fmt::{
        Layer as FmtLayer, MakeWriter,
        format::{Format, Json, JsonFields, Pretty, Writer as FmtWriter},
        layer,
    },
    layer::Context as LayerContext,
    registry::LookupSpan,
};
use tracing_web::MakeConsoleWriter;
use wasm_bindgen::JsValue;
use web_time::SystemTime;

use crate::{LogConfig, LogFormat, TracingError, create_resource_builder};

/// wasm doesn't have a native time implementation, so we use web_time
pub struct WebTime;

impl FormatTime for WebTime {
    fn format_time(&self, w: &mut FmtWriter<'_>) -> FmtResult {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();

        let secs = now.as_secs();
        let nanos = now.subsec_nanos();

        // Format as RFC3339-like timestamp
        let datetime = js_sys::Date::new(&JsValue::from_f64(secs as f64 * 1000.0));
        write!(
            w,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            datetime.get_utc_full_year(),
            datetime.get_utc_month() + 1, // JS months are 0-based
            datetime.get_utc_date(),
            datetime.get_utc_hours(),
            datetime.get_utc_minutes(),
            datetime.get_utc_seconds(),
            nanos / 1_000_000
        )
    }
}

type JsonFmtLayer<S, W> = FmtLayer<S, JsonFields, Format<Json, WebTime>, W>;
type PrettyFmtLayer<S, W> = FmtLayer<S, Pretty, Format<Pretty, WebTime>, W>;

/// A wrapper enum for `tracing-subscriber` fmt layers that avoids `Send + Sync` requirements in
/// the `wasm32-unknown-unknown` target.
///
/// ## The Problem
///
/// In `wasm32-unknown-unknown`, we cannot use `.boxed()` on tracing layers because it returns
/// `Box<dyn Layer<S> + Send + Sync>`, requiring the layer to implement `Send + Sync`. However:
///
/// 1. The `wasm32-unknown-unknown` target is single-threaded, making `Send + Sync` meaningless
/// 2. `MakeConsoleWriter` (from tracing-web) doesn't implement these traits
/// 3. Even if could we add bounds `W: Send + Sync` to our generic writer, we'd be unnecessarily
///    restricting valid single-threaded use cases
///
/// ## Other solutions we tried and why they don't work.
///
/// - Using `.boxed()` to box up the layer: Works for known concrete types, but in general it
///   requires`Send + Sync` which we can't guarantee in wasm.
/// - Returning `Box<dyn Layer<S>>`: While this removes the `Send + Sync` requirement, it causes
///   issues with `tracing-subscriber`'s `.with()` method expecting sized types. At runtime we
///   can't know the size.
/// - Different return types per format: We can't return different types from match arms.
///
/// ## This Solution
///
/// This approach creates an enum that implements `Layer<S>`, allowing us to avoid dynamic dispatch
/// and boxing entirely. As a result, we can return a concrete, sized type that is compatible with
/// `.with()`, while maintaining type safety.
///
/// The way we have to do this is by implementing all of the methods of `Layer`. Since we are a
/// simple wrapper, every implementation would look the same. So we use a macro to eliminate the
/// boilerplate, at the cost of the harder to read macro code.
pub enum FmtLayerEnum<S, W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    Json(JsonFmtLayer<S, W>),
    Pretty(PrettyFmtLayer<S, W>),
}

macro_rules! delegate_layer {
    ($self:ident, $($a:tt)*) => {
        match $self {
            FmtLayerEnum::Json(layer) => layer.$($a)*,
            FmtLayerEnum::Pretty(layer) => layer.$($a)*,
        }
    };
}

impl<S, W> Layer<S> for FmtLayerEnum<S, W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    #[inline]
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &SpanID,
        ctx: LayerContext<'_, S>,
    ) {
        delegate_layer!(self, on_new_span(attrs, id, ctx))
    }

    #[inline]
    fn on_record(&self, span: &SpanID, values: &SpanRecord<'_>, ctx: LayerContext<'_, S>) {
        delegate_layer!(self, on_record(span, values, ctx))
    }

    #[inline]
    fn on_enter(&self, span: &SpanID, ctx: LayerContext<'_, S>) {
        delegate_layer!(self, on_enter(span, ctx))
    }

    #[inline]
    fn on_exit(&self, span: &SpanID, ctx: LayerContext<'_, S>) {
        delegate_layer!(self, on_exit(span, ctx))
    }

    #[inline]
    fn on_close(&self, id: SpanID, ctx: LayerContext<'_, S>) {
        delegate_layer!(self, on_close(id, ctx))
    }

    #[inline]
    fn on_event(&self, event: &Event<'_>, ctx: LayerContext<'_, S>) {
        delegate_layer!(self, on_event(event, ctx))
    }

    #[inline]
    unsafe fn downcast_raw(&self, id: std::any::TypeId) -> Option<*const ()> {
        unsafe { delegate_layer!(self, downcast_raw(id)) }
    }
}

impl LogConfig {
    pub fn create_fmt_layer_with_writer<S, W>(&self, writer: W) -> FmtLayerEnum<S, W>
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
        W: for<'writer> MakeWriter<'writer> + 'static,
    {
        match self.log_format {
            LogFormat::Json => {
                let layer = layer()
                    .with_writer(writer)
                    .with_timer(WebTime)
                    .json()
                    .flatten_event(true);

                FmtLayerEnum::Json(layer)
            }
            LogFormat::Pretty => {
                let layer = layer().with_writer(writer).with_timer(WebTime).pretty();

                FmtLayerEnum::Pretty(layer)
            }
        }
    }

    pub fn create_fmt_layer<S>(&self) -> FmtLayerEnum<S, MakeConsoleWriter>
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        self.create_fmt_layer_with_writer(MakeConsoleWriter)
    }
    pub fn create_tracer_provider(&self) -> Result<SdkTracerProvider, TracingError> {
        let resource = create_resource_builder().build();
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
            .with_resource(resource)
            .build();

        Ok(tracer_provider)
    }

    pub fn create_otel_layer<S>(&self) -> Result<impl Layer<S>, TracingError>
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        let tracer_provider = self.create_tracer_provider()?;
        let tracer = tracer_provider.tracer("cloudflare-worker");

        Ok(OpenTelemetryLayer::new(tracer))
    }
}
