use reqwest::{StatusCode, Url};
use std::fmt;

/// A device answered with a non-success status. Kept typed so callers can match on it.
#[derive(Debug)]
pub struct HttpStatus {
    pub status: StatusCode,
    pub url: Url,
    pub body: String,
}

impl fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} returned {}: {}", self.url, self.status, self.body)
    }
}

impl std::error::Error for HttpStatus {}

/// The status a device answered with, if that's what went wrong
pub fn http_status(e: &anyhow::Error) -> Option<StatusCode> {
    e.downcast_ref::<HttpStatus>().map(|h| h.status)
}
