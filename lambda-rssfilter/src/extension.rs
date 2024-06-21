// This is lambda-rssfilter/src/extension.rs

use std::{error::Error as StdError, fmt, iter::FromIterator, sync::Arc};

use lambda_extension::{
    service_fn, Error as LambdaError, Extension, LambdaEvent, NextEvent, RegisteredExtension,
};
use opentelemetry::trace::TraceError;
use opentelemetry_sdk::trace::TracerProvider;
use thiserror::Error;
use tokio::sync::{
    mpsc::{error::SendError, unbounded_channel, UnboundedReceiver, UnboundedSender},
    Mutex,
};
use tower::Service;
use tracing::info;

/// A combined error type that can hold multiple errors.
///
/// Flushing telemetry can result in multiple errors, so this type is used to
/// collect them all and return/display them at once.
#[derive(Debug)]
pub struct CombinedError<E> {
    errors: Vec<E>,
}

impl Default for CombinedError<TraceError> {
    fn default() -> Self {
        CombinedError { errors: Vec::new() }
    }
}

impl StdError for CombinedError<TraceError> {}

impl<E: fmt::Display> fmt::Display for CombinedError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Multiple errors occurred:")?;

        for (i, error) in self.errors.iter().enumerate() {
            write!(f, "\n  {}. {}", i + 1, error)?;
        }

        Ok(())
    }
}

impl<T, E> FromIterator<Result<T, E>> for CombinedError<E> {
    fn from_iter<I: IntoIterator<Item = Result<T, E>>>(iter: I) -> Self {
        let errors: Vec<E> = iter.into_iter().filter_map(Result::err).collect();
        CombinedError { errors }
    }
}

impl<E> From<CombinedError<E>> for Result<(), CombinedError<E>> {
    fn from(errors: CombinedError<E>) -> Result<(), CombinedError<E>> {
        errors.errors.is_empty().then_some(()).ok_or(errors)
    }
}

#[derive(Debug, Error)]
pub enum LamdbaExtensionError {
    #[error("failed to flush logs and telemetry")]
    TraceError(#[from] CombinedError<TraceError>),

    #[error("failed to notify telemetry channel about done request")]
    ChannelError(#[from] SendError<()>),

    #[error("unsupported event type for extension: {0:?}")]
    UnsupportedEvent(NextEvent),
}

impl<E> TryInto<()> for CombinedError<E> {
    type Error = CombinedError<E>;

    fn try_into(self) -> Result<(), Self::Error> {
        self.errors.is_empty().then_some(()).ok_or(self)
    }
}

/// Creates an internal Lambda extension to flush logs/telemetry after each request.
///
/// The extension will wait for the runtime to finish processing the request, then
/// flush all logs and telemetry when signalled via an unbounded channel.
pub struct FlushExtension {
    request_done_receiver: Mutex<UnboundedReceiver<()>>,
    pub request_done_sender: UnboundedSender<()>,

    tracer_provider: TracerProvider,
}

impl FlushExtension {
    pub fn new(tracer_provider: TracerProvider) -> Self {
        let (request_done_sender, request_done_receiver) = unbounded_channel();

        Self {
            request_done_sender,
            request_done_receiver: Mutex::new(request_done_receiver),
            tracer_provider,
        }
    }

    pub async fn new_extension(
        tracer_provider: TracerProvider,
    ) -> Result<
        (
            RegisteredExtension<
                impl Service<LambdaEvent, Response = (), Error = LamdbaExtensionError>
                    + Send
                    + Sync
                    + 'static,
            >,
            Arc<Self>,
        ),
        LambdaError,
    > {
        let flush_extension = Arc::new(Self::new(tracer_provider));
        let flush_extension_clone = flush_extension.clone();

        let ext = Extension::new()
            .with_events(&["INVOKE"])
            .with_events_processor(service_fn(move |event: LambdaEvent| {
                let flush_extension = flush_extension.clone();

                async move { flush_extension.invoke(event).await }
            }))
            .with_extension_name("internal-flush-traces")
            .register()
            .await?;

        Ok((ext, flush_extension_clone))
    }

    /// Called by the Lambda runtime when the function is invoked.
    pub async fn invoke(&self, event: LambdaEvent) -> Result<(), LamdbaExtensionError> {
        match event.next {
            // Internal Lambda extensions only support the INVOKE event.
            NextEvent::Invoke(_e) => Ok(()),
            e => Err(LamdbaExtensionError::UnsupportedEvent(e)),
        }?;

        info!("extension waiting for event to be processed");

        // This will block until the runtime signals that it's done processing
        // the request.
        let recv = self.request_done_receiver.lock().await.recv().await;

        // The channel was closed, which means the runtime is shutting down.
        if recv.is_none() {
            info!("extension received shutdown signal");
            return Ok(());
        }

        info!("flushing logs and telemetry");

        self.tracer_provider
            .force_flush()
            .into_iter()
            .collect::<CombinedError<_>>()
            .try_into()
            .map_err(LamdbaExtensionError::TraceError)
    }

    pub fn notify_request_done(&self) -> Result<(), LambdaError> {
        self.request_done_sender.send(()).map_err(|e| {
            LambdaError::from(format!(
                "failed to notify telemetry channel about done request: {:?}",
                e,
            ))
        })?;

        Ok(())
    }
}
