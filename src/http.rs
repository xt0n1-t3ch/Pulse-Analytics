//! Shared HTTP client setup for provider requests.
//!
//! ureq 3 turns every 4xx and 5xx response into an error that drops the
//! headers and body. Callers that read `Retry-After` or an error payload build
//! their agent with [`agent_with_error_responses`] and pass the result through
//! [`classify`], which restores the status-plus-response shape.

use std::fmt;
use std::time::Duration;

use ureq::Agent;

/// A response with a streamed body.
pub type Response = ureq::http::Response<ureq::Body>;

/// Redirects followed by default; the limit ureq 2 applied before the upgrade.
pub const DEFAULT_REDIRECTS: u32 = 5;

/// Builds an agent whose whole request, redirects included, must finish within `timeout`.
///
/// 4xx and 5xx responses surface as [`ureq::Error::StatusCode`]. Pass
/// `max_redirects = 0` to receive 3xx responses unchanged.
#[must_use]
pub fn agent(timeout: Duration, max_redirects: u32) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(max_redirects)
        .build()
        .into()
}

/// Like [`agent`], but 4xx and 5xx responses are returned as `Ok` so their
/// headers and body stay readable. Run each result through [`classify`].
#[must_use]
pub fn agent_with_error_responses(timeout: Duration, max_redirects: u32) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(max_redirects)
        .http_status_as_error(false)
        .build()
        .into()
}

/// A failed request: the server answered with an error status, or the exchange never completed.
#[derive(Debug)]
pub enum Failure {
    /// The server answered with a 4xx or 5xx status; the response is still readable.
    /// Boxed so the error stays small.
    Status(u16, Box<Response>),
    /// The request failed before a usable response arrived.
    Transport(ureq::Error),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(status, _) => write!(f, "http status {status}"),
            Self::Transport(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Failure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Status(..) => None,
            Self::Transport(error) => Some(error),
        }
    }
}

impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        Self::Transport(error)
    }
}

/// Maps a response from an [`agent_with_error_responses`] agent onto [`Failure`].
///
/// # Errors
///
/// Returns [`Failure::Status`] for a 4xx or 5xx response and
/// [`Failure::Transport`] for any other failure.
pub fn classify(result: Result<Response, ureq::Error>) -> Result<Response, Failure> {
    let response = result?;
    let status = response.status().as_u16();
    if status >= 400 {
        Err(Failure::Status(status, Box::new(response)))
    } else {
        Ok(response)
    }
}

/// Returns a response header as text, or `None` when absent or not valid UTF-8.
#[must_use]
pub fn header<'r>(response: &'r Response, name: &str) -> Option<&'r str> {
    response.headers().get(name)?.to_str().ok()
}

/// Reads the whole body as text, capped at ureq's default 10 MB limit.
///
/// # Errors
///
/// Returns the transport error when the body cannot be read.
pub fn read_text(response: Response) -> Result<String, ureq::Error> {
    response.into_body().read_to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16, body: &str) -> Response {
        ureq::http::Response::builder()
            .status(status)
            .header("retry-after", "7")
            .body(ureq::Body::builder().data(body))
            .expect("response fixture")
    }

    #[test]
    fn classify_keeps_success_responses() {
        let ok = classify(Ok(response(200, "{}"))).expect("200 is a success");
        assert_eq!(ok.status().as_u16(), 200);
    }

    #[test]
    fn classify_keeps_headers_and_body_of_error_statuses() {
        let Err(Failure::Status(status, response)) = classify(Ok(response(429, "slow down")))
        else {
            panic!("429 must classify as a status failure");
        };
        assert_eq!(status, 429);
        assert_eq!(header(&response, "retry-after"), Some("7"));
        assert_eq!(read_text(*response).expect("body"), "slow down");
    }

    #[test]
    fn classify_treats_redirects_as_responses() {
        assert!(classify(Ok(response(302, ""))).is_ok());
    }

    #[test]
    fn classify_wraps_transport_errors() {
        let failure = classify(Err(ureq::Error::HostNotFound)).expect_err("transport failure");
        assert!(matches!(failure, Failure::Transport(_)));
    }
}
