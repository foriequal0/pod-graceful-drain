use thiserror::Error;

#[derive(Debug, Error)]
#[error("oops, my bad: {message}")]
pub struct Bug {
    pub message: String,
    pub source: Option<eyre::Report>,
}

#[derive(Debug, Error)]
#[error("oops, not my fault: {message}")]
pub struct NotMyFault {
    pub message: String,
    pub source: Option<eyre::Report>,
}
