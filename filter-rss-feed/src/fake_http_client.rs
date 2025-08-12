use async_trait::async_trait;
use bytes::Bytes;
use http::{
    HeaderMap, HeaderName, HeaderValue, Request as HttpRequest, Response as HttpResponse,
    StatusCode,
};
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;

#[cfg(any(test, feature = "testing"))]
use derive_builder::Builder;

use crate::http_client::{HttpClient, HttpClientError};

/// Error types that can be simulated by the fake HTTP client.
///
/// This allows tests to verify error handling without relying on external services
/// or network conditions.
#[derive(Debug, Error, Clone)]
pub enum FakeHttpError {
    #[error("Network error: {message}")]
    Network { message: String },

    #[error("Timeout error")]
    Timeout,

    #[error("Invalid content type")]
    InvalidContentType,
}

/// A mock HTTP response for testing purposes.
///
/// Allows precise control over response status, headers, and body content
/// when testing HTTP client interactions. The builder pattern provides
/// convenient methods for common response types.
#[cfg_attr(any(test, feature = "testing"), derive(Builder))]
#[cfg_attr(any(test, feature = "testing"), builder(setter(prefix = "with")))]
#[derive(Debug, Clone)]
pub struct FakeResponse {
    #[cfg_attr(any(test, feature = "testing"), builder(default = "StatusCode::OK"))]
    pub status: StatusCode,
    #[cfg_attr(any(test, feature = "testing"), builder(default))]
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl FakeResponse {
    pub fn new(status: StatusCode, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: HeaderMap::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let header_name = HeaderName::from_str(name.as_ref()).expect("Invalid header name");
        let header_value = HeaderValue::from_str(value.as_ref()).expect("Invalid header value");
        self.headers.insert(header_name, header_value);
        self
    }

    pub fn with_content_type(self, content_type: impl AsRef<str>) -> Self {
        self.with_header("content-type", content_type)
    }
}

#[cfg(any(test, feature = "testing"))]
impl FakeResponseBuilder {
    /// Convenience methods for building common response types with appropriate
    /// content-type headers.
    pub fn with_header(&mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> &mut Self {
        let mut headers = self.headers.clone().unwrap_or_default();
        let header_name = HeaderName::from_str(name.as_ref()).expect("Invalid header name");
        let header_value = HeaderValue::from_str(value.as_ref()).expect("Invalid header value");
        headers.insert(header_name, header_value);
        self.with_headers(headers)
    }

    pub fn with_content_type(&mut self, content_type: impl AsRef<str>) -> &mut Self {
        self.with_header("content-type", content_type)
    }

    /// Create a JSON response with application/json content type.
    pub fn json(body: impl Into<Bytes>) -> Self {
        FakeResponseBuilder::default()
            .with_status(StatusCode::OK)
            .with_content_type("application/json")
            .with_body(body.into())
            .clone()
    }

    /// Create an XML response with application/xml content type.
    pub fn xml(body: impl Into<Bytes>) -> Self {
        FakeResponseBuilder::default()
            .with_status(StatusCode::OK)
            .with_content_type("application/xml")
            .with_body(body.into())
            .clone()
    }

    /// Create an RSS response with application/rss+xml content type.
    pub fn rss(body: impl Into<Bytes>) -> Self {
        FakeResponseBuilder::default()
            .with_status(StatusCode::OK)
            .with_content_type("application/rss+xml")
            .with_body(body.into())
            .clone()
    }
}

/// A fake HTTP client implementation for testing.
///
/// Provides deterministic responses based on URL patterns, eliminating
/// dependencies on external services in tests. Supports both successful
/// responses and error simulation.
///
/// The client uses URL-based routing to return pre-configured responses
/// or errors, making tests reliable and fast.
#[cfg_attr(any(test, feature = "testing"), derive(Builder))]
#[cfg_attr(any(test, feature = "testing"), builder(setter(prefix = "with")))]
pub struct FakeHttpClient {
    #[cfg_attr(any(test, feature = "testing"), builder(default))]
    responses: HashMap<String, FakeResponse>,
    #[cfg_attr(any(test, feature = "testing"), builder(default))]
    errors: HashMap<String, FakeHttpError>,
    #[cfg_attr(
        any(test, feature = "testing"),
        builder(setter(into), default = "\"MISS\".to_string()")
    )]
    cache_status: String,
}

#[cfg(any(test, feature = "testing"))]
impl FakeHttpClientBuilder {
    /// Convenience methods for setting up common response patterns.
    /// These methods simplify test setup by providing pre-configured
    /// responses for typical HTTP interactions.
    pub fn with_response(&mut self, url: impl Into<String>, response: FakeResponse) -> &mut Self {
        let mut responses = self.responses.clone().unwrap_or_default();
        responses.insert(url.into(), response);

        self.with_responses(responses)
    }
    /// Add a JSON response for the given URL.
    pub fn with_json_response(
        &mut self,
        url: impl Into<String>,
        body: impl Into<Bytes>,
    ) -> &mut Self {
        self.with_response(
            url,
            FakeResponseBuilder::json(body)
                .build()
                .expect("Failed to build JSON response"),
        )
    }

    /// Add an XML response for the given URL.
    pub fn with_xml_response(
        &mut self,
        url: impl Into<String>,
        body: impl Into<Bytes>,
    ) -> &mut Self {
        self.with_response(
            url,
            FakeResponseBuilder::xml(body)
                .build()
                .expect("Failed to build XML response"),
        )
    }

    /// Add an RSS response for the given URL.
    pub fn with_rss_response(
        &mut self,
        url: impl Into<String>,
        body: impl Into<Bytes>,
    ) -> &mut Self {
        self.with_response(
            url,
            FakeResponseBuilder::rss(body)
                .build()
                .expect("Failed to build RSS response"),
        )
    }

    /// Add a response with specific status code for the given URL.
    pub fn with_status_response(
        &mut self,
        url: impl Into<String>,
        status: StatusCode,
        body: impl Into<Bytes>,
    ) -> &mut Self {
        self.with_response(url, FakeResponse::new(status, body))
    }

    pub fn with_error(&mut self, url: impl Into<String>, error: FakeHttpError) -> &mut Self {
        let mut errors = self.errors.clone().unwrap_or_default();
        errors.insert(url.into(), error);

        self.with_errors(errors)
    }

    // Convenience methods for simulating common error conditions.

    /// Configure a network error for the given URL.
    pub fn with_network_error(
        &mut self,
        url: impl Into<String>,
        message: impl Into<String>,
    ) -> &mut Self {
        self.with_error(
            url,
            FakeHttpError::Network {
                message: message.into(),
            },
        )
    }

    /// Configure a timeout error for the given URL.
    pub fn with_timeout_error(&mut self, url: impl Into<String>) -> &mut Self {
        self.with_error(url, FakeHttpError::Timeout)
    }

    /// Configure an invalid content type error for the given URL.
    pub fn with_invalid_content_type_error(&mut self, url: impl Into<String>) -> &mut Self {
        self.with_error(url, FakeHttpError::InvalidContentType)
    }
}

impl FakeHttpClient {
    pub fn new() -> Self {
        Self {
            responses: HashMap::new(),
            errors: HashMap::new(),
            cache_status: "MISS".to_string(),
        }
    }

    fn convert_fake_error(&self, error: &FakeHttpError) -> HttpClientError {
        match error {
            FakeHttpError::Network { message } => HttpClientError::Request(message.clone()),
            FakeHttpError::Timeout => HttpClientError::Request("Request timeout".to_string()),
            FakeHttpError::InvalidContentType => {
                HttpClientError::Request("Invalid content type".to_string())
            }
        }
    }
}

impl Default for FakeHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
#[cfg(not(target_arch = "wasm32"))]
impl HttpClient for FakeHttpClient {
    async fn send(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, HttpClientError> {
        let url = request.uri().to_string();

        // Check for configured errors first
        if let Some(error) = self.errors.get(&url) {
            return Err(self.convert_fake_error(error));
        }

        // Check for configured responses
        if let Some(fake_response) = self.responses.get(&url) {
            let mut response_builder = HttpResponse::builder().status(fake_response.status);

            // Add configured headers
            for (name, value) in &fake_response.headers {
                response_builder = response_builder.header(name, value);
            }

            // Add cache status header
            response_builder =
                response_builder.header("x-rssfilter-cache-status", &self.cache_status);

            return Ok(response_builder.body(fake_response.body.clone())?);
        }

        // Default 404 response for unmatched URLs
        Ok(HttpResponse::builder()
            .status(StatusCode::NOT_FOUND)
            .header("x-rssfilter-cache-status", &self.cache_status)
            .body(Bytes::from("Not Found"))?)
    }
}

#[async_trait(?Send)]
#[cfg(target_arch = "wasm32")]
impl HttpClient for FakeHttpClient {
    async fn send(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, HttpClientError> {
        let url = request.uri().to_string();

        // Check for configured errors first
        if let Some(error) = self.errors.get(&url) {
            return Err(self.convert_fake_error(error));
        }

        // Check for configured responses
        if let Some(fake_response) = self.responses.get(&url) {
            let mut response_builder = HttpResponse::builder().status(fake_response.status);

            // Add configured headers
            for (name, value) in &fake_response.headers {
                response_builder = response_builder.header(name, value);
            }

            // Add cache status header
            response_builder =
                response_builder.header("x-rssfilter-cache-status", &self.cache_status);

            return Ok(response_builder.body(fake_response.body.clone())?);
        }

        // Default 404 response for unmatched URLs
        Ok(HttpResponse::builder()
            .status(StatusCode::NOT_FOUND)
            .header("x-rssfilter-cache-status", &self.cache_status)
            .body(Bytes::from("Not Found"))?)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use http::Method;

    #[tokio::test]
    async fn test_fake_http_client_basic() {
        let client = FakeHttpClientBuilder::default()
            .with_json_response("https://example.com/test", r#"{"test": "data"}"#)
            .build()
            .expect("Failed to build fake client");

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/test")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(response.into_body(), r#"{"test": "data"}"#);
    }

    #[tokio::test]
    async fn test_fake_http_client_builder_convenience_methods() {
        let client = FakeHttpClientBuilder::default()
            .with_json_response("https://example.com/json", r#"{"test": "data"}"#)
            .with_xml_response("https://example.com/xml", "<test>data</test>")
            .with_rss_response("https://example.com/rss", "<rss>data</rss>")
            .with_cache_status("HIT")
            .build()
            .expect("Failed to build fake client");

        // Test JSON response
        let json_request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/json")
            .body(Bytes::new())
            .unwrap();

        let json_response = client.send(json_request).await.unwrap();
        assert_eq!(
            json_response.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(
            json_response
                .headers()
                .get("x-rssfilter-cache-status")
                .unwrap(),
            "HIT"
        );
        assert_eq!(json_response.into_body(), r#"{"test": "data"}"#);

        // Test XML response
        let xml_request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/xml")
            .body(Bytes::new())
            .unwrap();

        let xml_response = client.send(xml_request).await.unwrap();
        assert_eq!(
            xml_response.headers().get("content-type").unwrap(),
            "application/xml"
        );
        assert_eq!(xml_response.into_body(), "<test>data</test>");

        // Test RSS response
        let rss_request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/rss")
            .body(Bytes::new())
            .unwrap();

        let rss_response = client.send(rss_request).await.unwrap();
        assert_eq!(
            rss_response.headers().get("content-type").unwrap(),
            "application/rss+xml"
        );
        assert_eq!(rss_response.into_body(), "<rss>data</rss>");
    }

    #[tokio::test]
    async fn test_fake_http_client_error() {
        let client = FakeHttpClientBuilder::default()
            .with_network_error("https://example.com/error", "Connection failed")
            .build()
            .expect("Failed to build fake client");

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/error")
            .body(Bytes::new())
            .unwrap();

        let result = client.send(request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpClientError::Request(_)));
    }

    #[tokio::test]
    async fn test_fake_http_client_not_found() {
        let client = FakeHttpClient::new();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/nonexistent")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.into_body(), "Not Found");
    }

    #[tokio::test]
    async fn test_fake_http_client_with_headers() {
        let fake_response = FakeResponse::new(StatusCode::OK, "test body")
            .with_header("x-custom-header", "custom-value")
            .with_content_type("text/plain");

        let client = FakeHttpClientBuilder::default()
            .with_response("https://example.com/headers", fake_response)
            .build()
            .expect("Failed to build fake client");

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/headers")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/plain"
        );
        assert_eq!(
            response.headers().get("x-custom-header").unwrap(),
            "custom-value"
        );
        assert_eq!(
            response.headers().get("x-rssfilter-cache-status").unwrap(),
            "MISS"
        );
    }

    #[tokio::test]
    async fn test_fake_response_convenience_methods() {
        let json_response = FakeResponseBuilder::json(r#"{"key": "value"}"#)
            .build()
            .expect("Failed to build JSON response");
        assert_eq!(
            json_response.headers.get("content-type").unwrap(),
            "application/json"
        );

        let xml_response = FakeResponseBuilder::xml("<root></root>")
            .build()
            .expect("Failed to build XML response");
        assert_eq!(
            xml_response.headers.get("content-type").unwrap(),
            "application/xml"
        );

        let rss_response = FakeResponseBuilder::rss("<rss></rss>")
            .build()
            .expect("Failed to build RSS response");
        assert_eq!(
            rss_response.headers.get("content-type").unwrap(),
            "application/rss+xml"
        );
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use http::Method;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    wasm_bindgen_test_configure!(run_in_node_experimental);

    #[wasm_bindgen_test]
    async fn test_fake_http_client_wasm_basic() {
        let client = FakeHttpClientBuilder::default()
            .with_json_response("https://example.com/test", r#"{"test": "data"}"#)
            .build()
            .expect("Failed to build fake client");

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/test")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json"
        );
        assert_eq!(response.into_body(), r#"{"test": "data"}"#);
    }

    #[wasm_bindgen_test]
    async fn test_fake_http_client_wasm_error() {
        let client = FakeHttpClientBuilder::default()
            .with_network_error("https://example.com/error", "Connection failed")
            .build()
            .expect("Failed to build fake client");

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/error")
            .body(Bytes::new())
            .unwrap();

        let result = client.send(request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpClientError::Request(_)));
    }

    #[wasm_bindgen_test]
    async fn test_fake_http_client_wasm_not_found() {
        let client = FakeHttpClient::new();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/nonexistent")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.into_body(), "Not Found");
    }
}
