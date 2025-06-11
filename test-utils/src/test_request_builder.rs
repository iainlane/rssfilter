use http::{Method, Request};
use worker::Body;

pub struct RequestBuilder {
    path: String,
    query_params: Vec<(String, String)>,
    method: Method,
}

impl Default for RequestBuilder {
    fn default() -> Self {
        RequestBuilder {
            path: "/".to_string(),
            query_params: Vec::new(),
            method: Method::GET,
        }
    }
}

impl RequestBuilder {
    pub fn new() -> Self {
        RequestBuilder::default()
    }

    pub fn with_feed_url(mut self, feed_url: &str) -> Self {
        self.query_params
            .push(("url".to_string(), feed_url.to_string()));
        self
    }

    pub fn with_title_filter_regex(mut self, title_regex: &str) -> Self {
        self.query_params
            .push(("title_filter_regex".to_string(), title_regex.to_string()));
        self
    }

    pub fn with_guid_filter_regex(mut self, uid_regex: &str) -> Self {
        self.query_params
            .push(("uid_filter_regex".to_string(), uid_regex.to_string()));
        self
    }

    pub fn with_path(mut self, path: &str) -> Self {
        self.path = path.to_string();
        self
    }

    pub fn with_method(mut self, method: Method) -> Self {
        self.method = method;
        self
    }

    pub fn build(self) -> Result<Request<Body>, http::Error> {
        let mut url = format!("https://test.example.com{}", self.path);

        if !self.query_params.is_empty() {
            url.push('?');
            let query_string = self
                .query_params
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&");
            url.push_str(&query_string);
        }

        Request::builder()
            .method(self.method)
            .uri(url)
            .body(Body::empty())
    }
}
