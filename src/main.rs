mod decision;
mod environment;
mod transport;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const BODY_LIMIT: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 60;
const MAX_TIMEOUT_SECONDS: u64 = 600;

struct Flag {
    name: &'static str,
    value: Option<&'static str>,
    summary: Summary,
}

enum Summary {
    Fixed(&'static str),
    Timeout,
}

fn flags() -> &'static [Flag] {
    &[
        Flag {
            name: "--endpoint",
            value: Some("URL"),
            summary: Summary::Fixed(
                "Full decision URL. Overrides BASS_ENDPOINT from the process environment and the environment file.",
            ),
        },
        Flag {
            name: "--request",
            value: Some("PATH"),
            summary: Summary::Fixed(
                "Read the request document from PATH. Omit this flag to read standard input.",
            ),
        },
        Flag {
            name: "--timeout-seconds",
            value: Some("N"),
            summary: Summary::Timeout,
        },
        Flag {
            name: "--environment-file",
            value: Some("PATH"),
            summary: Summary::Fixed(
                "Read assignments from PATH. Omit this flag to use the default environment file.",
            ),
        },
        Flag {
            name: "--help",
            value: None,
            summary: Summary::Fixed("Write this text to standard output and exit 0."),
        },
        Flag {
            name: "--version",
            value: None,
            summary: Summary::Fixed("Write the package version to standard output and exit 0."),
        },
    ]
}

enum Command {
    Help,
    Version,
    Decide(Invocation),
}

struct Invocation {
    endpoint: Option<transport::Endpoint>,
    request: RequestSource,
    timeout: Duration,
    environment_file: Option<PathBuf>,
}

enum RequestSource {
    Path(PathBuf),
    Stdin,
}

enum UsageError {
    EndpointRequired,
    UnknownArgument,
    DuplicateFlag,
    MissingFlagValue,
    BadTimeout,
    TokenMissing,
    TokenEmpty,
    Endpoint(transport::EndpointError),
    RequestUnreadable,
    StdinIsTerminal,
    HelpMixed,
    VersionMixed,
    InvalidConfigurationText,
    Environment(environment::FileError),
}

enum Failure {
    Usage(UsageError),
    Request(decision::RequestError),
    Transport(transport::TransportError),
    Response(decision::ResponseError),
}

enum LimitedRead {
    TooLarge,
    Io,
}

fn main() -> ExitCode {
    match execute(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(bytes) => match write_stdout(&bytes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(()) => {
                let _ = writeln!(std::io::stderr(), "The decision could not be written.");
                ExitCode::from(1)
            }
        },
        Err(failure) => {
            let _ = writeln!(std::io::stderr(), "{failure}");
            ExitCode::from(exit_code(&failure))
        }
    }
}

fn exit_code(failure: &Failure) -> u8 {
    match failure {
        Failure::Usage(_) => 2,
        Failure::Request(_) => 3,
        Failure::Transport(_) => 4,
        Failure::Response(_) => 5,
    }
}

fn execute(args: &[String]) -> Result<Vec<u8>, Failure> {
    match parse_invocation(args) {
        Ok(Command::Help) => Ok(help_text().into_bytes()),
        Ok(Command::Version) => Ok(format!("bass {}\n", env!("CARGO_PKG_VERSION")).into_bytes()),
        Ok(Command::Decide(invocation)) => decide(invocation),
        Err(error) => Err(Failure::Usage(error)),
    }
}

fn decide(invocation: Invocation) -> Result<Vec<u8>, Failure> {
    let assignments = load_assignments(&invocation)?;
    let endpoint = resolve_endpoint(invocation.endpoint, &assignments)?;
    let token = resolve_token(&assignments)?;
    if matches!(invocation.request, RequestSource::Stdin) && std::io::stdin().is_terminal() {
        return Err(Failure::Usage(UsageError::StdinIsTerminal));
    }
    let bytes = read_request(&invocation.request)?;
    let request = decision::parse_request(&bytes).map_err(Failure::Request)?;
    let body = decision::render_request(&request);
    let response = transport::post(&endpoint, &token, &body, invocation.timeout, BODY_LIMIT)
        .map_err(Failure::Transport)?;
    let decision = decision::parse_decision(&response).map_err(Failure::Response)?;
    decision::check_decision(&request, &decision).map_err(Failure::Response)?;
    let mut output = decision::render_decision(&decision);
    output.push(b'\n');
    Ok(output)
}

fn parse_invocation(args: &[String]) -> Result<Command, UsageError> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    if args.iter().any(|arg| arg == "--help") {
        return Err(UsageError::HelpMixed);
    }
    if args.iter().any(|arg| arg == "--version") {
        return Err(UsageError::VersionMixed);
    }

    let mut endpoint = None;
    let mut request = None;
    let mut timeout = None;
    let mut environment_file = None;
    let mut index = 0;
    while index < args.len() {
        let name = &args[index];
        if !name.starts_with("--") || name.contains('=') {
            return Err(UsageError::UnknownArgument);
        }
        let flag = flags()
            .iter()
            .find(|flag| flag.name == name)
            .ok_or(UsageError::UnknownArgument)?;
        if flag.value.is_none() {
            return Err(UsageError::UnknownArgument);
        }
        let Some(value) = args.get(index + 1) else {
            return Err(UsageError::MissingFlagValue);
        };
        if value.starts_with("--") {
            return Err(UsageError::MissingFlagValue);
        }
        index += 2;
        match flag.name {
            "--endpoint" => {
                if endpoint.is_some() {
                    return Err(UsageError::DuplicateFlag);
                }
                endpoint = Some(transport::Endpoint::parse(value).map_err(UsageError::Endpoint)?);
            }
            "--request" => {
                if request.is_some() {
                    return Err(UsageError::DuplicateFlag);
                }
                request = Some(RequestSource::Path(PathBuf::from(value)));
            }
            "--timeout-seconds" => {
                if timeout.is_some() {
                    return Err(UsageError::DuplicateFlag);
                }
                timeout = Some(parse_timeout(value)?);
            }
            "--environment-file" => {
                if environment_file.is_some() {
                    return Err(UsageError::DuplicateFlag);
                }
                environment_file = Some(PathBuf::from(value));
            }
            _ => return Err(UsageError::UnknownArgument),
        }
    }

    Ok(Command::Decide(Invocation {
        endpoint,
        request: request.unwrap_or(RequestSource::Stdin),
        timeout: timeout.unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS)),
        environment_file,
    }))
}

fn parse_timeout(value: &str) -> Result<Duration, UsageError> {
    let seconds: u64 = value.parse().map_err(|_| UsageError::BadTimeout)?;
    if (1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
        Ok(Duration::from_secs(seconds))
    } else {
        Err(UsageError::BadTimeout)
    }
}

fn help_text() -> String {
    let mut synopsis = String::from("bass");
    for flag in flags() {
        let Some(placeholder) = flag.value else {
            continue;
        };
        synopsis.push_str(" [");
        synopsis.push_str(flag.name);
        synopsis.push(' ');
        synopsis.push_str(placeholder);
        synopsis.push(']');
    }

    let mut lines = vec![
        synopsis,
        String::new(),
        "BASS_TOKEN is the bearer token. BASS_ENDPOINT is the decision URL. A flag overrides the process environment, which overrides the environment file. bass does not write the token.".to_string(),
        String::new(),
    ];
    for flag in flags() {
        let label = match flag.value {
            Some(placeholder) => format!("{} {placeholder}", flag.name),
            None => flag.name.to_string(),
        };
        let detail = match flag.summary {
            Summary::Fixed(text) => text.to_string(),
            Summary::Timeout => format!(
                "Stop after N seconds. The default is {DEFAULT_TIMEOUT_SECONDS}. The maximum is {MAX_TIMEOUT_SECONDS}."
            ),
        };
        lines.push(label);
        lines.push(format!("    {detail}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn read_request(source: &RequestSource) -> Result<Vec<u8>, Failure> {
    let result = match source {
        RequestSource::Path(path) => {
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(_) => return Err(Failure::Usage(UsageError::RequestUnreadable)),
            };
            read_limited(file)
        }
        RequestSource::Stdin => read_limited(std::io::stdin()),
    };
    match result {
        Ok(bytes) => Ok(bytes),
        Err(LimitedRead::TooLarge) => Err(Failure::Request(decision::RequestError::BodyTooLarge)),
        Err(LimitedRead::Io) => Err(Failure::Usage(UsageError::RequestUnreadable)),
    }
}

fn read_limited(mut reader: impl Read) -> Result<Vec<u8>, LimitedRead> {
    let mut buffer = Vec::new();
    reader
        .by_ref()
        .take((BODY_LIMIT as u64) + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| LimitedRead::Io)?;
    if buffer.len() > BODY_LIMIT {
        Err(LimitedRead::TooLarge)
    } else {
        Ok(buffer)
    }
}

fn write_stdout(bytes: &[u8]) -> Result<(), ()> {
    let mut output = std::io::stdout();
    output.write_all(bytes).map_err(|_| ())?;
    output.flush().map_err(|_| ())
}

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsageError::EndpointRequired => formatter.write_str("An endpoint is required."),
            UsageError::UnknownArgument => formatter.write_str("An argument was not recognized."),
            UsageError::DuplicateFlag => formatter.write_str("A flag was repeated."),
            UsageError::MissingFlagValue => formatter.write_str("A flag value is missing."),
            UsageError::BadTimeout => {
                formatter.write_str("The timeout is outside the allowed range.")
            }
            UsageError::TokenMissing => formatter.write_str("BASS_TOKEN is missing."),
            UsageError::TokenEmpty => formatter.write_str("BASS_TOKEN is empty."),
            UsageError::Endpoint(error) => error.fmt(formatter),
            UsageError::RequestUnreadable => formatter.write_str("The request could not be read."),
            UsageError::StdinIsTerminal => {
                formatter.write_str("The request is read from a file or a pipe.")
            }
            UsageError::HelpMixed => formatter.write_str("--help is a sole argument."),
            UsageError::VersionMixed => formatter.write_str("--version is a sole argument."),
            UsageError::InvalidConfigurationText => {
                formatter.write_str("A configuration value is not valid text.")
            }
            UsageError::Environment(error) => error.fmt(formatter),
        }
    }
}

fn load_assignments(invocation: &Invocation) -> Result<environment::Assignments, Failure> {
    let loaded = match &invocation.environment_file {
        Some(path) => environment::load(path, true),
        None => match default_environment_file()? {
            Some(path) => environment::load(&path, false),
            None => Ok(None),
        },
    };
    match loaded {
        Ok(Some(assignments)) => Ok(assignments),
        Ok(None) => Ok(environment::Assignments {
            endpoint: None,
            token: None,
        }),
        Err(error) => Err(Failure::Usage(UsageError::Environment(error))),
    }
}

fn default_environment_file() -> Result<Option<PathBuf>, Failure> {
    let layout = if cfg!(windows) {
        environment::ConfigurationLayout::Windows
    } else {
        environment::ConfigurationLayout::Unix
    };
    let xdg = configuration_directory("XDG_CONFIG_HOME")?;
    let home = configuration_directory("HOME")?;
    let appdata = configuration_directory("APPDATA")?;
    Ok(environment::default_path(
        layout,
        xdg.as_deref(),
        home.as_deref(),
        appdata.as_deref(),
    ))
}

fn configuration_directory(name: &str) -> Result<Option<String>, Failure> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(Failure::Usage(UsageError::InvalidConfigurationText))
        }
    }
}

fn resolve_endpoint(
    flag: Option<transport::Endpoint>,
    assignments: &environment::Assignments,
) -> Result<transport::Endpoint, Failure> {
    if let Some(endpoint) = flag {
        return Ok(endpoint);
    }
    if let Some(value) = process_value("BASS_ENDPOINT")? {
        return transport::Endpoint::parse(&value)
            .map_err(|error| Failure::Usage(UsageError::Endpoint(error)));
    }
    if let Some(value) = &assignments.endpoint {
        return transport::Endpoint::parse(value)
            .map_err(|error| Failure::Usage(UsageError::Endpoint(error)));
    }
    Err(Failure::Usage(UsageError::EndpointRequired))
}

fn resolve_token(assignments: &environment::Assignments) -> Result<transport::Token, Failure> {
    let value = match process_value("BASS_TOKEN")? {
        Some(value) => Some(value),
        None => assignments.token.clone(),
    };
    match transport::token_from_env(value.as_deref()) {
        Ok(token) => Ok(token),
        Err(transport::TokenError::Missing) => Err(Failure::Usage(UsageError::TokenMissing)),
        Err(transport::TokenError::Empty) => Err(Failure::Usage(UsageError::TokenEmpty)),
    }
}

fn process_value(name: &str) -> Result<Option<String>, Failure> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(Failure::Usage(UsageError::InvalidConfigurationText))
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Usage(error) => error.fmt(formatter),
            Failure::Request(error) => error.fmt(formatter),
            Failure::Transport(error) => error.fmt(formatter),
            Failure::Response(error) => error.fmt(formatter),
        }
    }
}
