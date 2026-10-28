mod decision;
mod environment;
mod json;
mod transport;

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

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
            name: "--allow-cleartext-host",
            value: Some("NAME"),
            summary: Summary::Fixed(
                "Permit plain HTTP to one hostname when no token is configured, or when the token is an OpenShell credential placeholder. A real token is refused. This flag is not read from the environment.",
            ),
        },
        Flag {
            name: "--check",
            value: None,
            summary: Summary::Fixed(
                "Validate the request and do not connect. BASS_ENDPOINT, BASS_TOKEN, and an environment file are not read. Duplicate keys, 64-bit integers, and nesting depth are still checked. Whether the request contains the token is not checked, because that rule needs the token. The read still stops after the default timeout. Exits 0 with no output when the request is valid. Rejected together with --timeout-seconds, --environment-file, or --allow-cleartext-host.",
            ),
        },
        Flag {
            name: "--print-response-schema",
            value: None,
            summary: Summary::Fixed(
                "With --check, write the JSON Schema document for the response this request requires. The dialect is draft 2020-12. Rejected without --check.",
            ),
        },
    ]
}

enum Command {
    Help,
    DecideHelp,
    Version,
    Decide(Invocation),
}

struct Invocation {
    request: RequestSource,
    timeout: Duration,
    environment_file: Option<PathBuf>,
    cleartext_host: Option<String>,
    check: bool,
    print_response_schema: bool,
}

enum RequestSource {
    Path(PathBuf),
    StandardInput { explicit: bool },
}

enum UsageError {
    EndpointRequired,
    UnknownArgument,
    DuplicateFlag,
    MissingFlagValue,
    BadTimeout,
    Endpoint(transport::EndpointError),
    RequestUnreadable,
    HelpMixed,
    VersionMixed,
    InvalidConfigurationText,
    Environment(environment::FileError),
    CleartextNameInvalid,
    CleartextCredential,
    TokenWhitespace,
    CheckRejectsNetworkOption,
    ResponseSchemaRequiresCheck,
    SubcommandRequired,
    UnknownCommand,
    RequestAlreadyProvided,
    OmittedRequestIsTerminal,
}

enum Failure {
    Usage(UsageError),
    Request(decision::RequestError),
    RequestTimeout,
    Transport(transport::TransportError),
    Response(decision::ResponseError),
}

enum LimitedRead {
    TooLarge,
    Io,
    TimedOut,
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
        Failure::RequestTimeout => 4,
        Failure::Transport(error) => match error {
            transport::TransportError::RetryableStatus { .. } => 6,
            transport::TransportError::NonRetryableStatus { .. } => 7,
            transport::TransportError::ContentEncoding => 5,
            _ => 4,
        },
        Failure::Response(_) => 5,
    }
}

fn execute(args: &[String]) -> Result<Vec<u8>, Failure> {
    match parse_invocation(args) {
        Ok(Command::Help) => Ok(top_level_help().into_bytes()),
        Ok(Command::DecideHelp) => Ok(decide_help().into_bytes()),
        Ok(Command::Version) => Ok(format!("bass {}\n", env!("CARGO_PKG_VERSION")).into_bytes()),
        Ok(Command::Decide(invocation)) => decide(invocation),
        Err(error) => Err(Failure::Usage(error)),
    }
}

fn decide(invocation: Invocation) -> Result<Vec<u8>, Failure> {
    if matches!(
        invocation.request,
        RequestSource::StandardInput { explicit: false }
    ) && std::io::stdin().is_terminal()
    {
        return Err(Failure::Usage(UsageError::OmittedRequestIsTerminal));
    }
    if invocation.check {
        return check_request(&invocation);
    }
    let assignments = load_assignments(&invocation)?;
    let endpoint = resolve_endpoint(invocation.cleartext_host.as_deref(), &assignments)?;
    let token = resolve_token(&assignments)?;
    if endpoint.named_cleartext()
        && token
            .as_ref()
            .is_some_and(|token| !token.is_credential_placeholder())
    {
        return Err(Failure::Usage(UsageError::CleartextCredential));
    }
    let started = Instant::now();
    let bytes = read_request(&invocation.request, invocation.timeout)?;
    let request = decision::parse_request(&bytes).map_err(Failure::Request)?;
    let body = decision::render_request(&request);
    if token.as_ref().is_some_and(|token| token.occurs_in(&body)) {
        return Err(Failure::Request(decision::RequestError::TokenInDocument));
    }
    let remaining = invocation.timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return Err(Failure::Transport(transport::TransportError::Timeout));
    }
    let response = transport::post(&endpoint, token.as_ref(), &body, remaining, BODY_LIMIT)
        .map_err(Failure::Transport)?;
    let decision = decision::parse_decision(&response).map_err(Failure::Response)?;
    decision::check_decision(&request, &decision).map_err(Failure::Response)?;
    let mut output = decision::render_decision(&decision);
    output.push(b'\n');
    Ok(output)
}

fn check_request(invocation: &Invocation) -> Result<Vec<u8>, Failure> {
    let bytes = read_request(&invocation.request, invocation.timeout)?;
    let request = decision::parse_request(&bytes).map_err(Failure::Request)?;
    if invocation.print_response_schema {
        let mut output = decision::response_schema(&request);
        output.push(b'\n');
        return Ok(output);
    }
    Ok(Vec::new())
}

fn parse_invocation(args: &[String]) -> Result<Command, UsageError> {
    if args.is_empty() {
        return Err(UsageError::SubcommandRequired);
    }
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::Help);
    }
    if args.len() == 1 && args[0] == "--version" {
        return Ok(Command::Version);
    }
    if args[0] == "--help" {
        return Err(UsageError::HelpMixed);
    }
    if args[0] == "--version" {
        return Err(UsageError::VersionMixed);
    }
    if args[0] != "decide" {
        return Err(UsageError::UnknownCommand);
    }
    parse_decide(&args[1..])
}

fn parse_decide(args: &[String]) -> Result<Command, UsageError> {
    if args.len() == 1 && args[0] == "--help" {
        return Ok(Command::DecideHelp);
    }
    if args.iter().any(|argument| argument == "--help") {
        return Err(UsageError::HelpMixed);
    }

    let mut request = None;
    let mut timeout = None;
    let mut environment_file = None;
    let mut cleartext_host = None;
    let mut check = false;
    let mut print_response_schema = false;
    let mut options_ended = false;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if !options_ended && argument == "--" {
            options_ended = true;
            index += 1;
            continue;
        }
        if !options_ended && argument.starts_with("--") {
            if argument.contains('=') {
                return Err(UsageError::UnknownArgument);
            }
            let flag = flags()
                .iter()
                .find(|flag| flag.name == argument)
                .ok_or(UsageError::UnknownArgument)?;
            if flag.value.is_none() {
                index += 1;
                match flag.name {
                    "--check" => {
                        if check {
                            return Err(UsageError::DuplicateFlag);
                        }
                        check = true;
                    }
                    "--print-response-schema" => {
                        if print_response_schema {
                            return Err(UsageError::DuplicateFlag);
                        }
                        print_response_schema = true;
                    }
                    _ => return Err(UsageError::UnknownArgument),
                }
                continue;
            }
            let Some(value) = args.get(index + 1) else {
                return Err(UsageError::MissingFlagValue);
            };
            if value.starts_with("--") {
                return Err(UsageError::MissingFlagValue);
            }
            index += 2;
            match flag.name {
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
                "--allow-cleartext-host" => {
                    if cleartext_host.is_some() {
                        return Err(UsageError::DuplicateFlag);
                    }
                    cleartext_host = Some(
                        transport::normalize_hostname(value)
                            .ok_or(UsageError::CleartextNameInvalid)?,
                    );
                }
                _ => return Err(UsageError::UnknownArgument),
            }
            continue;
        }
        let source = if argument == "-" {
            RequestSource::StandardInput { explicit: true }
        } else {
            RequestSource::Path(PathBuf::from(argument))
        };
        if request.is_some() {
            return Err(UsageError::RequestAlreadyProvided);
        }
        request = Some(source);
        index += 1;
    }

    if print_response_schema && !check {
        return Err(UsageError::ResponseSchemaRequiresCheck);
    }
    if check && (timeout.is_some() || environment_file.is_some() || cleartext_host.is_some()) {
        return Err(UsageError::CheckRejectsNetworkOption);
    }

    Ok(Command::Decide(Invocation {
        request: request.unwrap_or(RequestSource::StandardInput { explicit: false }),
        timeout: timeout.unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS)),
        environment_file,
        cleartext_host,
        check,
        print_response_schema,
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

fn top_level_help() -> String {
    "\
bass decide [OPTIONS] [REQUEST]
bass --help
bass --version

decide posts one decision request. Run bass decide --help for the options.
--help writes this text to standard output and exits 0.
--version writes the package version to standard output and exits 0.
"
    .to_string()
}

fn decide_help() -> String {
    let mut lines = vec![
        "bass decide [OPTIONS] [REQUEST]".to_string(),
        String::new(),
        "REQUEST is a file path, or - for standard input. When REQUEST is omitted, bass reads standard input if it is not a terminal. A terminal exits 2. A path that begins with a hyphen is written after --.".to_string(),
        String::new(),
        "BASS_ENDPOINT is the decision URL. The process environment overrides the environment file. The token is sent only to that endpoint. BASS_TOKEN is an optional bearer token. When it is absent, empty, or only ASCII whitespace, bass sends no Authorization header. Leading or trailing ASCII whitespace is rejected. bass does not write the token.".to_string(),
        String::new(),
        "--check validates REQUEST and does not connect. It does not read BASS_ENDPOINT, BASS_TOKEN, or an environment file. It applies every request rule that does not need the token, including a repeated object key, an integer outside the 64-bit range, and nesting deeper than 512. It cannot test whether the request contains the token. A token that is only ASCII whitespace, and a token with leading or trailing ASCII whitespace, are not reported here. It exits 0 with no output when the request is valid. --print-response-schema writes one JSON Schema document for the response and is valid only together with --check.".to_string(),
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
                "Stop the request read and the exchange after N seconds in total. The default is {DEFAULT_TIMEOUT_SECONDS}. The maximum is {MAX_TIMEOUT_SECONDS}."
            ),
        };
        lines.push(label);
        lines.push(format!("    {detail}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn read_request(source: &RequestSource, timeout: Duration) -> Result<Vec<u8>, Failure> {
    let result = match source {
        RequestSource::Path(path) => {
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(_) => return Err(Failure::Usage(UsageError::RequestUnreadable)),
            };
            read_bounded(file, timeout)
        }
        RequestSource::StandardInput { .. } => read_bounded(std::io::stdin(), timeout),
    };
    match result {
        Ok(bytes) => Ok(bytes),
        Err(LimitedRead::TooLarge) => Err(Failure::Request(decision::RequestError::BodyTooLarge)),
        Err(LimitedRead::Io) => Err(Failure::Usage(UsageError::RequestUnreadable)),
        Err(LimitedRead::TimedOut) => Err(Failure::RequestTimeout),
    }
}

fn read_bounded(
    reader: impl Read + Send + 'static,
    timeout: Duration,
) -> Result<Vec<u8>, LimitedRead> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(read_limited(reader));
    });
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(LimitedRead::TimedOut),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(LimitedRead::Io),
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
            UsageError::Endpoint(error) => error.fmt(formatter),
            UsageError::RequestUnreadable => formatter.write_str("The request could not be read."),
            UsageError::OmittedRequestIsTerminal => {
                formatter.write_str("The request was omitted and standard input is a terminal.")
            }
            UsageError::SubcommandRequired => {
                formatter.write_str("bass decide [OPTIONS] [REQUEST]\nA subcommand is required.")
            }
            UsageError::UnknownCommand => formatter.write_str("The subcommand was not recognized."),
            UsageError::RequestAlreadyProvided => {
                formatter.write_str("Only one request path is accepted.")
            }
            UsageError::HelpMixed => formatter.write_str("--help is a sole argument."),
            UsageError::VersionMixed => formatter.write_str("--version is a sole argument."),
            UsageError::InvalidConfigurationText => {
                formatter.write_str("A configuration value is not valid text.")
            }
            UsageError::Environment(error) => error.fmt(formatter),
            UsageError::CleartextNameInvalid => {
                formatter.write_str("The cleartext host name is not valid.")
            }
            UsageError::CleartextCredential => formatter.write_str(
                "Cleartext to a named host requires no token or an OpenShell credential placeholder.",
            ),
            UsageError::TokenWhitespace => {
                formatter.write_str("BASS_TOKEN has leading or trailing whitespace.")
            }
            UsageError::CheckRejectsNetworkOption => formatter.write_str(
                "--check does not accept --timeout-seconds, --environment-file, or --allow-cleartext-host.",
            ),
            UsageError::ResponseSchemaRequiresCheck => {
                formatter.write_str("--print-response-schema requires --check.")
            }
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
    cleartext_host: Option<&str>,
    assignments: &environment::Assignments,
) -> Result<transport::Endpoint, Failure> {
    let raw = if let Some(value) = process_value("BASS_ENDPOINT")? {
        value
    } else if let Some(value) = &assignments.endpoint {
        value.clone()
    } else {
        return Err(Failure::Usage(UsageError::EndpointRequired));
    };
    transport::Endpoint::parse(&raw, cleartext_host)
        .map_err(|error| Failure::Usage(UsageError::Endpoint(error)))
}

fn resolve_token(
    assignments: &environment::Assignments,
) -> Result<Option<transport::Token>, Failure> {
    let value = match process_value("BASS_TOKEN")? {
        Some(value) => Some(value),
        None => assignments.token.clone(),
    };
    match value.as_deref() {
        None => Ok(None),
        Some(text) => classify_token(text),
    }
}

fn classify_token(text: &str) -> Result<Option<transport::Token>, Failure> {
    let trimmed = text.trim_ascii();
    if trimmed.is_empty() {
        Ok(None)
    } else if trimmed.len() == text.len() {
        Ok(Some(transport::Token::new(text)))
    } else {
        Err(Failure::Usage(UsageError::TokenWhitespace))
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
            Failure::RequestTimeout => formatter.write_str("The request read timed out."),
            Failure::Transport(error) => error.fmt(formatter),
            Failure::Response(error) => error.fmt(formatter),
        }
    }
}
