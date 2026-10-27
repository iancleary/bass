use std::io::Read;
use std::time::Duration;

use url::{Host, Url};

pub(super) struct Endpoint(Url);

pub(super) struct Token(String);

pub(super) enum EndpointError {
    Unparseable,
    Userinfo,
    Fragment,
    CleartextHost,
    Scheme,
}

pub(super) enum TransportError {
    Redirect { status: Option<u16> },
    Status { status: u16 },
    ContentType,
    Timeout,
    Connect,
    BodyTooLarge,
    Exchange,
}

pub(super) enum TokenError {
    Missing,
    Empty,
}

pub(super) fn token_from_env(value: Option<&str>) -> Result<Token, TokenError> {
    match value {
        None => Err(TokenError::Missing),
        Some("") => Err(TokenError::Empty),
        Some(text) => Ok(Token(text.to_string())),
    }
}

pub(super) fn post(
    endpoint: &Endpoint,
    token: &Token,
    body: &[u8],
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>, TransportError> {
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(0)
        .build();
    match agent
        .post(endpoint.0.as_str())
        .set("Authorization", &token.bearer())
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("User-Agent", concat!("bass/", env!("CARGO_PKG_VERSION")))
        .send_bytes(body)
    {
        Ok(response) => read_response(response, limit),
        Err(ureq::Error::Status(status, _response)) => status_error(status),
        Err(ureq::Error::Transport(error)) => Err(map_transport(&error)),
    }
}

fn read_response(response: ureq::Response, limit: usize) -> Result<Vec<u8>, TransportError> {
    let status = response.status();
    if (300..400).contains(&status) {
        return Err(TransportError::Redirect {
            status: Some(status),
        });
    }
    if status != 200 {
        return Err(TransportError::Status { status });
    }
    let content_type = response.header("content-type").unwrap_or("");
    if !is_json(content_type) {
        return Err(TransportError::ContentType);
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
    if (300..400).contains(&status) {
        Err(TransportError::Redirect {
            status: Some(status),
        })
    } else {
        Err(TransportError::Status { status })
    }
}

fn map_transport(error: &ureq::Transport) -> TransportError {
    match error.kind() {
        ureq::ErrorKind::TooManyRedirects => TransportError::Redirect { status: None },
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
    pub(super) fn parse(raw: &str) -> Result<Self, EndpointError> {
        let url = Url::parse(raw).map_err(|_| EndpointError::Unparseable)?;
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
            "https" => Ok(Endpoint(url)),
            "http" if host_is_loopback(&url) => Ok(Endpoint(url)),
            "http" => Err(EndpointError::CleartextHost),
            _ => Err(EndpointError::Scheme),
        }
    }
}

fn host_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

impl Token {
    fn bearer(&self) -> String {
        let mut value = String::from("Bearer ");
        value.push_str(&self.0);
        value
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
            EndpointError::Scheme => "The endpoint scheme is unsupported.",
        })
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Redirect {
                status: Some(status),
            } => {
                write!(formatter, "The endpoint returned redirect status {status}.")
            }
            TransportError::Redirect { status: None } => {
                formatter.write_str("The endpoint redirected the request.")
            }
            TransportError::Status { status } => {
                write!(formatter, "The endpoint returned status {status}.")
            }
            TransportError::ContentType => {
                formatter.write_str("The response content type is not JSON.")
            }
            TransportError::Timeout => formatter.write_str("The exchange timed out."),
            TransportError::Connect => formatter.write_str("The endpoint could not be reached."),
            TransportError::BodyTooLarge => formatter.write_str("The response is too large."),
            TransportError::Exchange => formatter.write_str("The exchange failed."),
        }
    }
}
