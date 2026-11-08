use protocol::ErrorCode;

/// Code of a failure that never reached a response, for [`ApiError::code`].
const NETWORK_CODE: &str = "network";
/// Code of a body that could not be read, for [`ApiError::code`].
const DECODE_CODE: &str = "decode";

/// Failure of an API call.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in")]
    Unauthorized,
    #[error("{message}")]
    Api { code: ErrorCode, message: String },
    /// The response was not one of the server's own errors, such as a proxy's error page.
    #[error("{}", describe_status(*status, reason))]
    Status { status: u16, reason: String },
    #[error("request failed: {0}")]
    Transport(String),
    #[error("unexpected response: {0}")]
    Decode(String),
}

impl ApiError {
    /// Whether the same request may succeed when sent again: the connection failed, a proxy
    /// answered instead of the server, a resumable upload's offset moved on, or the server
    /// hit a passing error.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Transport(_) | Self::Decode(_) => true,
            Self::Api { code, .. } => matches!(code, ErrorCode::Conflict | ErrorCode::Internal),
            Self::Status { status, .. } => *status >= 500 || matches!(status, 408 | 429),
            Self::Unauthorized => false,
        }
    }

    /// Whether the server as a whole could not be reached: the connection failed or a
    /// gateway in front of the server answered that it is down.
    pub fn is_unreachable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Status { status, .. } => is_gateway_status(*status),
            Self::Unauthorized | Self::Api { .. } | Self::Decode(_) => false,
        }
    }

    /// A short code for the failure, such as `502`, `conflict` or `network`.
    pub fn code(&self) -> String {
        match self {
            Self::Unauthorized => error_code_name(ErrorCode::Unauthorized).to_owned(),
            Self::Api { code, .. } => error_code_name(*code).to_owned(),
            Self::Status { status, .. } => status.to_string(),
            Self::Transport(_) => NETWORK_CODE.to_owned(),
            Self::Decode(_) => DECODE_CODE.to_owned(),
        }
    }
}

/// The wire name of `code`, as the server sends it.
fn error_code_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Unauthorized => "unauthorized",
        ErrorCode::NotFound => "not_found",
        ErrorCode::Conflict => "conflict",
        ErrorCode::BadRequest => "bad_request",
        ErrorCode::PayloadTooLarge => "payload_too_large",
        ErrorCode::Gone => "gone",
        ErrorCode::TooManyRequests => "too_many_requests",
        ErrorCode::Internal => "internal",
    }
}

/// Statuses a gateway or proxy sends when the server behind it is down or too slow,
/// including Cloudflare's 520–527.
fn is_gateway_status(status: u16) -> bool {
    matches!(status, 502..=504 | 520..=527)
}

fn describe_status(status: u16, reason: &str) -> String {
    let reason = cloudflare_reason(status).unwrap_or(reason);
    let named = if reason.is_empty() {
        status.to_string()
    } else {
        format!("{status} {reason}")
    };
    if is_gateway_status(status) {
        format!("server unreachable ({named})")
    } else {
        format!("HTTP {named}")
    }
}

/// Names of Cloudflare's own statuses, which the HTTP standard does not name.
fn cloudflare_reason(status: u16) -> Option<&'static str> {
    Some(match status {
        520 => "Unknown Error",
        521 => "Web Server Is Down",
        522 => "Connection Timed Out",
        523 => "Origin Is Unreachable",
        524 => "A Timeout Occurred",
        525 => "SSL Handshake Failed",
        526 => "Invalid SSL Certificate",
        527 => "Railgun Error",
        _ => return None,
    })
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        let message = with_causes(&err);
        if err.is_decode() {
            Self::Decode(message)
        } else {
            Self::Transport(message)
        }
    }
}

/// `err` followed by its sources, since reqwest's own message ("error sending request")
/// leaves out why, such as a refused connection or a failed TLS handshake.
fn with_causes(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: u16, reason: &str) -> ApiError {
        ApiError::Status {
            status,
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn a_gateway_error_reads_as_an_unreachable_server() {
        assert_eq!(
            status(502, "Bad Gateway").to_string(),
            "server unreachable (502 Bad Gateway)"
        );
        assert_eq!(
            status(523, "").to_string(),
            "server unreachable (523 Origin Is Unreachable)"
        );
    }

    #[test]
    fn another_status_reads_as_plain_http() {
        assert_eq!(
            status(413, "Payload Too Large").to_string(),
            "HTTP 413 Payload Too Large"
        );
    }

    #[test]
    fn gateway_errors_and_lost_connections_mean_unreachable() {
        assert!(status(502, "").is_unreachable());
        assert!(status(522, "").is_unreachable());
        assert!(ApiError::Transport("reset".to_owned()).is_unreachable());
        assert!(!status(500, "").is_unreachable());
        assert!(!status(413, "").is_unreachable());
        assert!(
            !ApiError::Api {
                code: ErrorCode::Internal,
                message: String::new(),
            }
            .is_unreachable()
        );
    }

    #[test]
    fn server_errors_and_throttling_are_transient() {
        assert!(status(500, "").is_transient());
        assert!(status(429, "").is_transient());
        assert!(!status(413, "").is_transient());
    }

    #[test]
    fn codes_name_the_status_or_the_category() {
        assert_eq!(status(502, "").code(), "502");
        assert_eq!(
            ApiError::Api {
                code: ErrorCode::PayloadTooLarge,
                message: String::new(),
            }
            .code(),
            "payload_too_large"
        );
        assert_eq!(ApiError::Transport(String::new()).code(), NETWORK_CODE);
    }
}
