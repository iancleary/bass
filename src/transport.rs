use std::io::Read;
use std::time::Duration;

use url::{Host, Url};

pub(super) struct Endpoint {
    url: Url,
    named_cleartext: bool,
}

pub(super) struct Token(String);

pub(super) enum EndpointError {
    Unparseable,
    Userinfo,
    Fragment,
    CleartextHost,
    CleartextName,
    Scheme,
}

pub(super) enum TransportError {
    RetryableStatus { status: u16 },
    NonRetryableStatus { status: Option<u16> },
    ContentType,
    ContentEncoding,
    Timeout,
    Connect,
    BodyTooLarge,
    Exchange,
}

pub(super) fn post(
    endpoint: &Endpoint,
    token: Option<&Token>,
    body: &[u8],
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>, TransportError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(0)
        .build();
    let mut request = agent
        .post(endpoint.url.as_str())
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("Accept-Encoding", "identity")
        .set("User-Agent", concat!("bass/", env!("CARGO_PKG_VERSION")));
    if let Some(token) = token {
        request = request.set("Authorization", &token.bearer());
    }
    match request.send_bytes(body) {
        Ok(response) => read_response(response, limit),
        Err(ureq::Error::Status(status, _response)) => status_error(status),
        Err(ureq::Error::Transport(error)) => Err(map_transport(&error)),
    }
}

fn read_response(response: ureq::Response, limit: usize) -> Result<Vec<u8>, TransportError> {
    let status = response.status();
    if status != 200 {
        return Err(classify_status(status));
    }
    let content_type = response.header("content-type").unwrap_or("");
    if !is_json(content_type) {
        return Err(TransportError::ContentType);
    }
    if let Some(encoding) = response.header("content-encoding") {
        if !encoding.trim().eq_ignore_ascii_case("identity") {
            return Err(TransportError::ContentEncoding);
        }
    }
    let mut buffer = Vec::new();
    response
        .into_reader()
        .take((limit as u64) + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| TransportError::Exchange)?;
    if buffer.len() > limit {
        return Err(TransportError::BodyTooLarge);
    }
    Ok(buffer)
}

fn status_error(status: u16) -> Result<Vec<u8>, TransportError> {
    Err(classify_status(status))
}

fn classify_status(status: u16) -> TransportError {
    if status == 429 || (500..600).contains(&status) {
        TransportError::RetryableStatus { status }
    } else {
        TransportError::NonRetryableStatus {
            status: Some(status),
        }
    }
}

fn map_transport(error: &ureq::Transport) -> TransportError {
    match error.kind() {
        ureq::ErrorKind::TooManyRedirects => TransportError::NonRetryableStatus { status: None },
        ureq::ErrorKind::Dns
        | ureq::ErrorKind::ConnectionFailed
        | ureq::ErrorKind::ProxyConnect => TransportError::Connect,
        ureq::ErrorKind::Io if timed_out(error) => TransportError::Timeout,
        _ => TransportError::Exchange,
    }
}

fn timed_out(error: &ureq::Transport) -> bool {
    let mut current = Some(error as &(dyn std::error::Error + 'static));
    while let Some(item) = current {
        if let Some(io_error) = item.downcast_ref::<std::io::Error>() {
            if io_error.kind() == std::io::ErrorKind::TimedOut {
                return true;
            }
        }
        current = item.source();
    }
    false
}

fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/json")
}

impl Endpoint {
    pub(super) fn parse(
        raw: &str,
        allowed_cleartext_host: Option<&str>,
    ) -> Result<Self, EndpointError> {
        let mut url = Url::parse(raw).map_err(|_| EndpointError::Unparseable)?;
        if url.host().is_none() {
            return Err(EndpointError::Unparseable);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EndpointError::Userinfo);
        }
        if url.fragment().is_some() {
            return Err(EndpointError::Fragment);
        }
        match url.scheme() {
            "https" => Ok(Endpoint {
                url,
                named_cleartext: false,
            }),
            "http" if host_is_loopback(&url) => Ok(Endpoint {
                url,
                named_cleartext: false,
            }),
            "http" => {
                let Some(allowed) = allowed_cleartext_host else {
                    return Err(EndpointError::CleartextHost);
                };
                let host = comparable_host(&url).ok_or(EndpointError::CleartextName)?;
                if host != allowed {
                    return Err(EndpointError::CleartextName);
                }
                url.set_host(Some(&host))
                    .map_err(|_| EndpointError::CleartextName)?;
                Ok(Endpoint {
                    url,
                    named_cleartext: true,
                })
            }
            _ => Err(EndpointError::Scheme),
        }
    }

    pub(super) fn named_cleartext(&self) -> bool {
        self.named_cleartext
    }
}

pub(super) fn normalize_hostname(raw: &str) -> Option<String> {
    if raw.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let stripped = raw.strip_suffix('.').unwrap_or(raw);
    if stripped.is_empty()
        || stripped.len() > 253
        || stripped.parse::<std::net::Ipv4Addr>().is_ok()
        || stripped.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return None;
    }
    let mut normalized = String::new();
    for label in stripped.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return None;
        }
        if !normalized.is_empty() {
            normalized.push('.');
        }
        normalized.push_str(&label.to_ascii_lowercase());
    }
    Some(normalized)
}

fn comparable_host(url: &Url) -> Option<String> {
    normalize_hostname(url.host_str()?)
}

fn host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

impl Token {
    pub(super) fn new(text: &str) -> Self {
        Token(text.to_string())
    }

    fn bearer(&self) -> String {
        let mut value = String::from("Bearer ");
        value.push_str(&self.0);
        value
    }

    pub(super) fn occurs_in(&self, body: &[u8]) -> bool {
        let needle = self.0.as_bytes();
        !needle.is_empty() && body.windows(needle.len()).any(|window| window == needle)
    }

    // The placeholder form is `openshell:resolve:env:` followed by one or more
    // characters from [A-Z0-9_]. The form is taken from OpenShell documentation
    // and the maintainer's existing profile. It is an inference to be confirmed.
    pub(super) fn is_credential_placeholder(&self) -> bool {
        let Some(name) = self.0.strip_prefix("openshell:resolve:env:") else {
            return false;
        };
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token([redacted])")
    }
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            EndpointError::Unparseable => "The endpoint is not a URL.",
            EndpointError::Userinfo => "The endpoint contains user info.",
            EndpointError::Fragment => "The endpoint contains a fragment.",
            EndpointError::CleartextHost => "Cleartext is limited to a loopback IP literal.",
            EndpointError::CleartextName => "The cleartext host is not the allowed name.",
            EndpointError::Scheme => "The endpoint scheme is unsupported.",
        })
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::RetryableStatus { status } => {
                write!(
                    formatter,
                    "The endpoint returned retryable status {status}."
                )
            }
            TransportError::NonRetryableStatus {
                status: Some(status),
            } => {
                write!(
                    formatter,
                    "The endpoint returned non-retryable status {status}."
                )
            }
            TransportError::NonRetryableStatus { status: None } => {
                formatter.write_str("The endpoint returned a non-retryable status.")
            }
            TransportError::ContentType => {
                formatter.write_str("The response content type is not JSON.")
            }
            TransportError::ContentEncoding => {
                formatter.write_str("The response content encoding is not identity.")
            }
            TransportError::Timeout => formatter.write_str("The exchange timed out."),
            TransportError::Connect => formatter.write_str("The endpoint could not be reached."),
            TransportError::BodyTooLarge => formatter.write_str("The response is too large."),
            TransportError::Exchange => formatter.write_str("The exchange failed."),
        }
    }
}
