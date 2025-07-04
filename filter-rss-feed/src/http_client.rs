use async_trait::async_trait;
use bytes::Bytes;
use http::{HeaderName, HeaderValue, Request as HttpRequest, Response as HttpResponse};
use thiserror::Error;
use tracing::debug;

#[cfg(not(target_arch = "wasm32"))]
use tracing::instrument;

#[cfg(target_arch = "wasm32")]
use std::hash::{Hash, Hasher};

#[derive(Debug, Error)]
pub enum HttpClientError {
    #[error("Request failed: {0}")]
    Request(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Header conversion error: {0}")]
    Header(String),

    #[error("Body conversion error: {0}")]
    Body(String),
}

/// Abstraction over HTTP clients that work with standard http crate types
#[async_trait]
#[cfg(not(target_arch = "wasm32"))]
pub trait HttpClient: Send + Sync {
    async fn send(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, HttpClientError>;
}

#[async_trait(?Send)]
#[cfg(target_arch = "wasm32")]
pub trait HttpClient {
    async fn send(
        &self,
        request: HttpRequest<Bytes>,
    ) -> Result<HttpResponse<Bytes>, HttpClientError>;
}

/// Configuration for cache behaviour
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Time-to-live for cached responses in seconds. Default is 300 seconds (5 minutes)
    #[allow(dead_code)]
    pub ttl_seconds: u64,
    #[allow(dead_code)]
    pub cache_key_prefix: String,
    pub status_header_name: String,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl_seconds: 300, // 5 minutes
            cache_key_prefix: "http-cache".to_string(),
            status_header_name: "x-rssfilter-cache-status".to_string(),
        }
    }
}

// Non-WASM implementation using reqwest
#[cfg(not(target_arch = "wasm32"))]
pub mod reqwest_client {
    use super::*;

    pub fn default_reqwest_client() -> Result<reqwest::Client, reqwest::Error> {
        let builder = reqwest::ClientBuilder::new()
            .user_agent("filter-rss-feed https://github.com/iainlane/filter-rss-feed")
            .brotli(true)
            .deflate(true)
            .gzip(true)
            .zstd(true);

        builder.build()
    }

    pub struct ReqwestHttpClient {
        client: reqwest::Client,
        cache_config: CacheConfig,
    }

    impl ReqwestHttpClient {
        pub fn new(client: reqwest::Client, cache_config: CacheConfig) -> Self {
            Self {
                client,
                cache_config,
            }
        }

        fn convert_request(
            &self,
            req: HttpRequest<Bytes>,
        ) -> Result<reqwest::Request, HttpClientError> {
            let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes())
                .map_err(|e| HttpClientError::Request(format!("Invalid method: {e}")))?;

            let url = req.uri().to_string();

            let mut request_builder = self.client.request(method, &url);

            // Convert headers
            for (name, value) in req.headers() {
                let header_value = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
                    .map_err(|e| HttpClientError::Header(format!("Invalid header value: {e}")))?;
                request_builder = request_builder.header(name.as_str(), header_value);
            }

            // Add body if present
            let body = req.into_body();
            if !body.is_empty() {
                request_builder = request_builder.body(body);
            }

            request_builder
                .build()
                .map_err(|e| HttpClientError::Request(format!("Failed to build request: {e}")))
        }

        async fn convert_response(
            &self,
            resp: reqwest::Response,
        ) -> Result<HttpResponse<Bytes>, HttpClientError> {
            let status = resp.status();
            let headers = resp.headers().clone();
            let body = resp
                .bytes()
                .await
                .map_err(|e| HttpClientError::Body(format!("Failed to read response body: {e}")))?;

            let mut response_builder = HttpResponse::builder().status(status.as_u16());

            // Convert headers
            for (name, value) in headers.iter() {
                response_builder = response_builder.header(name.as_str(), value.as_bytes());
            }

            // Add cache status header
            let cache_header_name = HeaderName::from_bytes(
                self.cache_config.status_header_name.as_bytes(),
            )
            .map_err(|e| HttpClientError::Header(format!("Invalid cache header name: {e}")))?;
            let cache_header_value = HeaderValue::from_static("MISS");
            response_builder = response_builder.header(cache_header_name, cache_header_value);

            response_builder
                .body(body)
                .map_err(|e| HttpClientError::Body(format!("Failed to build response: {e}")))
        }
    }

    #[async_trait]
    impl HttpClient for ReqwestHttpClient {
        #[instrument(skip(self, request))]
        async fn send(
            &self,
            request: HttpRequest<Bytes>,
        ) -> Result<HttpResponse<Bytes>, HttpClientError> {
            debug!("Making HTTP request via reqwest");

            let reqwest_request = self.convert_request(request)?;
            let reqwest_response = self
                .client
                .execute(reqwest_request)
                .await
                .map_err(|e| HttpClientError::Request(format!("Request failed: {e}")))?;

            self.convert_response(reqwest_response).await
        }
    }
}

// WASM implementation using workers-rs Fetch
#[cfg(target_arch = "wasm32")]
pub mod worker_client {
    use super::*;
    use crate::header_cf_cache_status::CfCacheStatus;
    use headers::HeaderMapExt;
    use http::HeaderMap;
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;
    use worker::{CfProperties, Fetch, Request as WorkerRequest, RequestInit};

    pub struct WorkerHttpClient {
        cache_config: CacheConfig,
    }

    impl WorkerHttpClient {
        pub fn new(cache_config: CacheConfig) -> Self {
            Self { cache_config }
        }

        pub fn create_cache_key(&self, request: &HttpRequest<Bytes>) -> String {
            let mut hasher = DefaultHasher::new();
            request.uri().to_string().hash(&mut hasher);
            request.method().as_str().hash(&mut hasher);

            for (name, value) in request.headers() {
                name.as_str().hash(&mut hasher);
                value.as_bytes().hash(&mut hasher);
            }

            format!(
                "{}-{:x}",
                self.cache_config.cache_key_prefix,
                hasher.finish()
            )
        }

        async fn convert_and_send(
            &self,
            request: HttpRequest<Bytes>,
        ) -> Result<HttpResponse<Bytes>, HttpClientError> {
            let cache_key = self.create_cache_key(&request);
            let uri = request.uri().to_string();

            // Convert http::Request to worker::Request
            let worker_headers = worker::Headers::new();
            for (name, value) in request.headers() {
                let value_str = std::str::from_utf8(value.as_bytes()).map_err(|e| {
                    HttpClientError::Header(format!("Invalid UTF-8 in header: {e}"))
                })?;
                worker_headers
                    .set(name.as_str(), value_str)
                    .map_err(|e| HttpClientError::Header(format!("Failed to set header: {e}")))?;
            }

            // Configure CloudFlare properties with caching
            let mut cache_ttl_by_status = HashMap::new();
            cache_ttl_by_status.insert("200-299".to_string(), self.cache_config.ttl_seconds as i32);
            cache_ttl_by_status.insert(
                "300-399".to_string(),
                (self.cache_config.ttl_seconds / 2) as i32,
            ); // Shorter for redirects

            let cf_properties = CfProperties {
                cache_everything: Some(true),
                cache_ttl: Some(self.cache_config.ttl_seconds as u32),
                cache_key: Some(cache_key.clone()),
                cache_ttl_by_status: Some(cache_ttl_by_status),
                ..Default::default()
            };

            let method = match *request.method() {
                http::Method::GET => worker::Method::Get,
                http::Method::POST => worker::Method::Post,
                http::Method::PUT => worker::Method::Put,
                http::Method::DELETE => worker::Method::Delete,
                http::Method::HEAD => worker::Method::Head,
                http::Method::OPTIONS => worker::Method::Options,
                http::Method::PATCH => worker::Method::Patch,
                _ => {
                    return Err(HttpClientError::Request(format!(
                        "Unsupported method: {}",
                        request.method()
                    )))
                }
            };

            let mut request_init = RequestInit::new();
            request_init
                .with_method(method)
                .with_headers(worker_headers)
                .with_cf_properties(cf_properties);

            // Add body if present
            let body = request.into_body();
            if !body.is_empty() {
                let js_body = body.to_vec().into_boxed_slice().into();
                request_init.with_body(Some(js_body));
            }

            let worker_request =
                WorkerRequest::new_with_init(&uri, &request_init).map_err(|e| {
                    HttpClientError::Request(format!("Failed to create worker request: {e}"))
                })?;

            // Send request
            let mut worker_response = Fetch::Request(worker_request)
                .send()
                .await
                .map_err(|e| HttpClientError::Request(format!("Fetch failed: {e}")))?;

            // Extract what we need before consuming the response
            let header_map: HeaderMap = worker_response.headers().into();
            let status = worker_response.status_code();

            // Check if response came from cache
            let cf_cache_status = &header_map
                .typed_get::<CfCacheStatus>()
                .unwrap_or(CfCacheStatus::Miss)
                .to_string();

            // Now consume the response to get the body
            let body: Bytes = worker_response
                .bytes()
                .await
                .map_err(|e| HttpClientError::Body(format!("Failed to read response body: {e}")))?
                .into();

            let mut response_builder = HttpResponse::builder().status(status);

            response_builder = header_map
                .iter()
                .fold(response_builder, |builder, (key, value)| {
                    builder.header(key.as_str(), value)
                });

            // Add our cache status header
            let cache_header_name = HeaderName::from_bytes(
                self.cache_config.status_header_name.as_bytes(),
            )
            .map_err(|e| HttpClientError::Header(format!("Invalid cache header name: {e}")))?;
            let cache_header_value = HeaderValue::from_str(cf_cache_status)
                .map_err(|e| HttpClientError::Header(format!("Invalid cache header value: {e}")))?;
            response_builder = response_builder.header(cache_header_name, cache_header_value);

            debug!(
                cache_key = cache_key,
                cache_status = cf_cache_status,
                status = status,
                "HTTP request completed"
            );

            response_builder
                .body(body)
                .map_err(|e| HttpClientError::Body(format!("Failed to build response: {e}")))
        }
    }

    // For WASM targets, we need to conditionally implement the trait without Send bounds
    #[async_trait(?Send)]
    impl HttpClient for WorkerHttpClient {
        async fn send(
            &self,
            request: HttpRequest<Bytes>,
        ) -> Result<HttpResponse<Bytes>, HttpClientError> {
            debug!("Making HTTP request via CloudFlare Workers Fetch");
            self.convert_and_send(request).await
        }
    }
}

// Factory functions
pub fn create_http_client() -> Result<Box<dyn HttpClient>, HttpClientError> {
    create_http_client_with_config(CacheConfig::default())
}

pub fn create_http_client_with_config(
    cache_config: CacheConfig,
) -> Result<Box<dyn HttpClient>, HttpClientError> {
    #[cfg(target_arch = "wasm32")]
    {
        Ok(Box::new(worker_client::WorkerHttpClient::new(cache_config)))
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let reqwest_client = reqwest_client::default_reqwest_client().map_err(|e| {
            HttpClientError::Request(format!("Failed to create reqwest client: {e}"))
        })?;
        Ok(Box::new(reqwest_client::ReqwestHttpClient::new(
            reqwest_client,
            cache_config,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.ttl_seconds, 300);
        assert_eq!(config.cache_key_prefix, "http-cache");
        assert_eq!(config.status_header_name, "x-rssfilter-cache-status");
    }

    #[test]
    fn test_cache_config_custom() {
        let config = CacheConfig {
            ttl_seconds: 600,
            cache_key_prefix: "my-cache".to_string(),
            status_header_name: "X-My-Cache".to_string(),
        };
        assert_eq!(config.ttl_seconds, 600);
        assert_eq!(config.cache_key_prefix, "my-cache");
        assert_eq!(config.status_header_name, "X-My-Cache");
    }

    // Integration tests for non-WASM
    #[cfg(not(target_arch = "wasm32"))]
    mod reqwest_tests {
        use super::*;
        use http::{Method, StatusCode};

        const CREATED: u16 = StatusCode::CREATED.as_u16();
        const OK: u16 = StatusCode::OK.as_u16();

        #[tokio::test]
        async fn test_reqwest_client_get() {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("GET", "/test")
                .with_status(OK as usize)
                .with_header("content-type", "application/json")
                .with_body(r#"{"status": "ok"}"#)
                .create_async()
                .await;

            let client = create_http_client().unwrap();

            let request = HttpRequest::builder()
                .method(Method::GET)
                .uri(format!("{}/test", server.url()))
                .body(Bytes::new())
                .unwrap();

            let response = client.send(request).await.unwrap();

            assert_eq!(response.status(), OK);
            assert_eq!(
                response.headers().get("x-rssfilter-cache-status").unwrap(),
                "MISS"
            );

            let body = response.into_body();
            assert_eq!(body, r#"{"status": "ok"}"#);
        }

        #[tokio::test]
        async fn test_reqwest_client_custom_headers() {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("GET", "/test")
                .match_header("user-agent", "test-agent")
                .match_header("authorization", "Bearer token123")
                .with_status(OK as usize)
                .create_async()
                .await;

            let client = create_http_client().unwrap();

            let request = HttpRequest::builder()
                .method(Method::GET)
                .uri(format!("{}/test", server.url()))
                .header("user-agent", "test-agent")
                .header("authorization", "Bearer token123")
                .body(Bytes::new())
                .unwrap();

            let response = client.send(request).await.unwrap();
            assert_eq!(response.status(), OK);
        }

        #[tokio::test]
        async fn test_reqwest_client_post_with_body() {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/test")
                .match_header("content-type", "application/json")
                .match_body(r#"{"test": "data"}"#)
                .with_status(CREATED as usize)
                .create_async()
                .await;

            let client = create_http_client().unwrap();

            let body = Bytes::from_static(br#"{"test": "data"}"#);
            let request = HttpRequest::builder()
                .method(Method::POST)
                .uri(format!("{}/test", server.url()))
                .header("content-type", "application/json")
                .body(body)
                .unwrap();

            let response = client.send(request).await.unwrap();
            assert_eq!(response.status(), CREATED);
        }

        #[tokio::test]
        async fn test_reqwest_client_error_handling() {
            let client = create_http_client().unwrap();

            let request = HttpRequest::builder()
                .method(Method::GET)
                .uri("http://localhost:99999/nonexistent") // Non-existent server
                .body(Bytes::new())
                .unwrap();

            let result = client.send(request).await;
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), HttpClientError::Request(_)));
        }

        #[tokio::test]
        async fn test_custom_cache_config() {
            let config = CacheConfig {
                ttl_seconds: 600,
                cache_key_prefix: "test-cache".to_string(),
                status_header_name: "X-Test-Cache".to_string(),
            };

            let mut server = mockito::Server::new_async().await;
            server
                .mock("GET", "/test")
                .with_status(OK as usize)
                .create_async()
                .await;

            let client = create_http_client_with_config(config).unwrap();

            let request = HttpRequest::builder()
                .method(Method::GET)
                .uri(format!("{}/test", server.url()))
                .body(Bytes::new())
                .unwrap();

            let response = client.send(request).await.unwrap();

            assert_eq!(response.status(), OK);
            assert_eq!(response.headers().get("X-Test-Cache").unwrap(), "MISS");
        }
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;

    use http::Method;
    use http::StatusCode;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

    const OK: StatusCode = StatusCode::OK;

    wasm_bindgen_test_configure!(run_in_node_experimental);

    #[wasm_bindgen_test]
    async fn test_worker_client_creation() {
        let client = create_http_client();
        assert!(client.is_ok());
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_with_custom_config() {
        let config = CacheConfig {
            ttl_seconds: 600,
            cache_key_prefix: "test-cache".to_string(),
            status_header_name: "X-Test-Cache".to_string(),
        };

        let client = create_http_client_with_config(config);
        assert!(client.is_ok());
    }

    /**
     * !!! Most of these tests run against a live server (httpbin.org). !!!
     *
     * TODO: Mock these requests to avoid hitting the live server. But how to do this in wasm?
     */

    #[wasm_bindgen_test]
    async fn test_worker_client_get_request() {
        let client = create_http_client().unwrap();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://httpbin.org/get")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await;
        assert!(response.is_ok());

        let response = response.unwrap();
        assert_eq!(response.status(), OK);

        // Check that our cache status header is present
        assert!(response.headers().contains_key("x-rssfilter-cache-status"));

        // Verify we got some body content
        let body = response.into_body();
        assert!(!body.is_empty());
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_get_with_headers() {
        let client = create_http_client().unwrap();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://httpbin.org/headers")
            .header("User-Agent", "test-agent")
            .header("X-Test-Header", "test-value")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await;
        assert!(response.is_ok());

        let response = response.unwrap();
        assert_eq!(response.status(), OK);

        // httpbin.org/headers echoes back the headers we sent
        let body = response.into_body();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("test-agent"));
        assert!(body_str.contains("test-value"));
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_post_with_json() {
        let client = create_http_client().unwrap();

        let json_body = r#"{"test": "data", "number": 42}"#;
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("https://httpbin.org/post")
            .header("Content-Type", "application/json")
            .body(Bytes::from(json_body))
            .unwrap();

        let response = client.send(request).await;
        assert!(response.is_ok());

        let response = response.unwrap();
        assert_eq!(response.status(), OK);

        // httpbin.org/post echoes back the data we sent
        let body = response.into_body();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(body_str.contains("test"));
        assert!(body_str.contains("data"));
        assert!(body_str.contains("42"));
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_custom_cache_header() {
        let config = CacheConfig {
            ttl_seconds: 300,
            cache_key_prefix: "custom-prefix".to_string(),
            status_header_name: "X-Custom-Cache".to_string(),
        };

        let client = create_http_client_with_config(config).unwrap();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://httpbin.org/get")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();

        // Check that our custom cache header is present
        assert!(response.headers().contains_key("X-Custom-Cache"));
        let cache_status = response.headers().get("X-Custom-Cache").unwrap();
        assert!(cache_status == "HIT" || cache_status == "MISS");
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_different_methods() {
        let client = create_http_client().unwrap();

        // Test PUT
        let put_request = HttpRequest::builder()
            .method(Method::PUT)
            .uri("https://httpbin.org/put")
            .header("Content-Type", "text/plain")
            .body(Bytes::from("put data"))
            .unwrap();

        let put_response = client.send(put_request).await.unwrap();
        assert_eq!(put_response.status(), OK);

        // Test DELETE
        let delete_request = HttpRequest::builder()
            .method(Method::DELETE)
            .uri("https://httpbin.org/delete")
            .body(Bytes::new())
            .unwrap();

        let delete_response = client.send(delete_request).await.unwrap();
        assert_eq!(delete_response.status(), OK);

        // Test PATCH
        let patch_request = HttpRequest::builder()
            .method(Method::PATCH)
            .uri("https://httpbin.org/patch")
            .header("Content-Type", "application/json")
            .body(Bytes::from(r#"{"patched": true}"#))
            .unwrap();

        let patch_response = client.send(patch_request).await.unwrap();
        assert_eq!(patch_response.status(), OK);
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_cache_key_generation() {
        use super::worker_client::WorkerHttpClient;

        let config = CacheConfig::default();
        let client = WorkerHttpClient::new(config);

        // Create two identical requests
        let request1 = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/test")
            .header("Accept", "application/json")
            .body(Bytes::new())
            .unwrap();

        let request2 = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/test")
            .header("Accept", "application/json")
            .body(Bytes::new())
            .unwrap();

        let key1 = client.create_cache_key(&request1);
        let key2 = client.create_cache_key(&request2);

        // Identical requests should generate identical cache keys
        assert_eq!(key1, key2);
        assert!(key1.starts_with("http-cache-"));

        // Different requests should generate different cache keys
        let request3 = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://example.com/different")
            .header("Accept", "application/json")
            .body(Bytes::new())
            .unwrap();

        let key3 = client.create_cache_key(&request3);
        assert_ne!(key1, key3);
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_error_handling() {
        let client = create_http_client().unwrap();

        // Test with an invalid URL scheme (should fail)
        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("invalid://not-a-real-url")
            .body(Bytes::new())
            .unwrap();

        let result = client.send(request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpClientError::Request(_)));
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_response_headers() {
        let client = create_http_client().unwrap();

        let request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://httpbin.org/response-headers?X-Test=value")
            .body(Bytes::new())
            .unwrap();

        let response = client.send(request).await.unwrap();
        assert_eq!(response.status(), OK);

        // httpbin sets the X-Test header as requested
        assert!(
            response.headers().contains_key("x-test") || response.headers().contains_key("X-Test")
        );

        // Our cache status header should be present
        assert!(response.headers().contains_key("x-rssfilter-cache-status"));
    }

    #[wasm_bindgen_test]
    async fn test_worker_client_empty_vs_non_empty_body() {
        let client = create_http_client().unwrap();

        // Test with empty body
        let empty_request = HttpRequest::builder()
            .method(Method::GET)
            .uri("https://httpbin.org/get")
            .body(Bytes::new())
            .unwrap();

        let empty_response = client.send(empty_request).await.unwrap();
        assert_eq!(empty_response.status(), OK);

        // Test with non-empty body
        let body_request = HttpRequest::builder()
            .method(Method::POST)
            .uri("https://httpbin.org/post")
            .header("Content-Type", "text/plain")
            .body(Bytes::from("test content"))
            .unwrap();

        let body_response = client.send(body_request).await.unwrap();
        assert_eq!(body_response.status(), OK);

        let response_body = body_response.into_body();
        let response_str = std::str::from_utf8(&response_body).unwrap();
        assert!(response_str.contains("test content"));
    }
}
