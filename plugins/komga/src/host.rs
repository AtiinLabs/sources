//! The seam between the plugin and the Ito runtime.
//!
//! Everything that reaches outside the wasm module (the stored settings and HTTP) goes
//! through [`Host`]. The rest of the crate is plain Rust, so unit tests drive it with a
//! scripted fake and never need the runtime.

use ito_rs::Result;
use ito_rs::net::{NetResponse, Request};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
        }
    }
}

/// An outgoing request. `ito_rs::net::Request` cannot be inspected, so requests are
/// described here first and only turned into one at the host boundary.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

pub trait Host {
    /// A plugin setting as the user last saved it, or `None` if it was never set.
    fn get_setting(&self, key: &str) -> Result<Option<String>>;

    /// Performs the request. A response with an error status is still `Ok`.
    fn send(&self, request: &HttpRequest) -> Result<NetResponse>;
}

/// The real host: Ito's runtime, reached through `ito-rs`.
pub struct ItoHost;

impl Host for ItoHost {
    fn get_setting(&self, key: &str) -> Result<Option<String>> {
        ito_rs::defaults::get(key)
    }

    fn send(&self, request: &HttpRequest) -> Result<NetResponse> {
        let mut outgoing = Request::new(request.url.as_str(), request.method.as_str());
        for (name, value) in &request.headers {
            outgoing.header(name, value);
        }
        if let Some(body) = &request.body {
            outgoing.body(body);
        }
        outgoing.send()
    }
}
