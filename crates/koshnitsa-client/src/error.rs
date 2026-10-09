#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not logged in: run `koshnitsa login` first")]
    NotLoggedIn,
    #[error("the eBag session has expired: copy a fresh Cookie header and run `koshnitsa login`")]
    SessionExpired,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("eBag is rate limiting requests; wait a minute before retrying")]
    RateLimited,
    #[error("unexpected response {status}: {body}")]
    Unexpected { status: u16, body: String },
    #[error("could not decode the response from {path}: {source}")]
    Decode {
        path: String,
        source: serde_json::Error,
    },
    #[error("invalid cookie: {0}")]
    InvalidCookie(&'static str),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}
