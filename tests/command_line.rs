use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

static CONFIG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn fresh_config_dir() -> PathBuf {
    let number = CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("bass-config-{}-{number}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create configuration directory");
    path
}

fn bass(args: &[&str]) -> Command {
    command_in(&fresh_config_dir(), args)
}

fn command_in(config: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bass"));
    command.args(args);
    command.stdin(Stdio::null());
    command.env("XDG_CONFIG_HOME", config);
    command.env("HOME", config);
    command.env_remove("APPDATA");
    command.env_remove("BASS_TOKEN");
    command.env_remove("BASS_ENDPOINT");
    command.env_remove("BASS_TOKEN_ORIGIN");
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env_remove(name);
    }
    command
}

fn output_of(command: &mut Command) -> Output {
    command.output().expect("run bass")
}

fn status_code(output: &Output) -> i32 {
    output.status.code().expect("bass exit status")
}

#[test]
fn version_prints_the_package_version() {
    let output = output_of(&mut bass(&["--version"]));
    assert_eq!(status_code(&output), 0);
    assert_eq!(
        output.stdout,
        format!("bass {}\n", env!("CARGO_PKG_VERSION")).into_bytes()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn help_lists_the_flags_and_the_token_variable() {
    let output = output_of(&mut bass(&["--help"]));
    assert_eq!(status_code(&output), 0);
    let text = String::from_utf8(output.stdout).expect("help is utf-8");
    assert!(text.contains("bass decide [OPTIONS] [REQUEST]"));
    assert!(text.contains("--help"));
    assert!(text.contains("--version"));
    assert!(!text.contains("--request"));
    assert!(!text.contains("Bearer"));
    assert!(output.stderr.is_empty());

    let decision = output_of(&mut bass(&["decide", "--help"]));
    assert_eq!(status_code(&decision), 0);
    let decision_text = String::from_utf8(decision.stdout).expect("decide help is utf-8");
    for name in [
        "--endpoint",
        "--timeout-seconds",
        "--environment-file",
        "--allow-cleartext-host",
        "REQUEST",
        "BASS_TOKEN",
        "BASS_ENDPOINT",
        "BASS_TOKEN_ORIGIN",
    ] {
        assert!(
            decision_text.contains(name),
            "{name} missing from decide help"
        );
    }
    assert!(!decision_text.contains("--request"));
    assert!(decision.stderr.is_empty());
}

#[test]
fn missing_subcommand_exits_2() {
    let output = output_of(&mut bass(&[]));
    assert_eq!(status_code(&output), 2);
    assert!(output.stdout.is_empty());
    assert_eq!(
        output.stderr,
        b"bass decide [OPTIONS] [REQUEST]\nA subcommand is required.\n"
    );
}

#[test]
fn missing_endpoint_exits_2() {
    let output = output_of(&mut bass(&["decide"]));
    assert_eq!(status_code(&output), 2);
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"An endpoint is required.\n");
}

#[test]
fn help_beside_another_argument_exits_2() {
    let output = output_of(&mut bass(&["--help", "--version"]));
    assert_eq!(status_code(&output), 2);
    assert_eq!(output.stderr, b"--help is a sole argument.\n");
}

#[test]
fn absent_token_sends_no_authorization_header() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (headers, _) = split_http(&captured);
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
}

#[test]
fn cleartext_hostname_is_rejected() {
    let output = output_of(&mut bass(&[
        "decide",
        "--endpoint",
        "http://example.invalid/decision",
    ]));
    assert_eq!(status_code(&output), 2);
    assert_eq!(
        output.stderr,
        b"Cleartext is limited to a loopback IP literal.\n"
    );
}

#[test]
fn userinfo_is_rejected_without_echoing_the_secret() {
    let output = output_of(&mut bass(&[
        "decide",
        "--endpoint",
        "http://operator:endpoint-secret@127.0.0.1/decision",
    ]));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The endpoint contains user info.\n");
    assert!(!stderr.contains("endpoint-secret"));
}

#[test]
fn https_hostname_is_accepted_before_the_document_is_checked() {
    let mut command = bass(&[
        "decide",
        "--endpoint",
        "https://example.invalid/decision",
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", "https://example.invalid");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(br#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"only":"one"}}}}"#)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 3);
    assert_eq!(
        output.stderr,
        b"A choice question is outside 2 to 255 options.\n"
    );
}

#[test]
fn invalid_request_does_not_connect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/decision")),
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(br#"{"model":"demo","state":null,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 3);
    assert_eq!(output.stderr, b"The state is null.\n");
    listener.set_nonblocking(true).expect("nonblocking");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn decision_is_posted_and_printed() {
    let proxy = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let proxy_address = proxy.local_addr().expect("proxy address");
    proxy.set_nonblocking(true).expect("nonblocking proxy");
    let request = br#"{"questions":{"beta":{"instructions":"Is beta true?","type":"noul"},"alpha":{"instructions":"Is alpha true?","type":"noul"}},"state":"pending","model":"demo"}"#;
    let response = br#"{"usage":{"output_tokens":2,"input_tokens":10},"answers":{"beta":{"noul":0.25,"type":"noul"},"alpha":{"type":"noul","noul":1}},"model":"demo"}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let path = std::env::temp_dir().join(format!("bass-request-{}.json", std::process::id()));
    std::fs::write(&path, request).expect("write request file");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        path.to_str().expect("temp path"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/decision")),
    );
    command.env("HTTP_PROXY", format!("http://{proxy_address}"));
    command.env("http_proxy", format!("http://{proxy_address}"));
    let output = output_of(&mut command);
    let captured = handle.join().expect("server thread");
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout,
        b"{\"model\":\"demo\",\"answers\":{\"alpha\":{\"type\":\"noul\",\"noul\":1.0},\"beta\":{\"type\":\"noul\",\"noul\":0.25}},\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}\n"
    );
    let (headers, body) = split_http(&captured);
    assert!(headers.starts_with("POST /decision "));
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer decision-token"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
    assert_eq!(
        body,
        br#"{"model":"demo","state":"pending","questions":{"alpha":{"type":"noul","instructions":"Is alpha true?"},"beta":{"type":"noul","instructions":"Is beta true?"}}}"#
    );
    match proxy.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("proxy received a connection: {other:?}"),
    }
}

#[test]
fn redirect_is_not_followed() {
    let target = TcpListener::bind("127.0.0.1:0").expect("bind target");
    let target_address = target.local_addr().expect("target address");
    target.set_nonblocking(true).expect("nonblocking target");
    let location = format!("Location: http://{target_address}/redirect-target-marker\r\n");
    let (address, handle) = serve(302, &location, b"");
    let output = post_stdin(
        &format!("http://{address}/decision"),
        br#"{"model":"demo","state":"pending","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    let _ = handle.join();
    assert_eq!(status_code(&output), 7);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The endpoint returned non-retryable status 302.\n");
    assert!(!stderr.contains("redirect-target-marker"));
    assert!(!stderr.contains("decision-token"));
    match target.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("redirect target received a connection: {other:?}"),
    }
}

#[test]
fn answer_kind_mismatch_omits_the_response_body() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"choice","choice":"RESPONSE-BODY-MARKER","probabilities":{"RESPONSE-BODY-MARKER":1},"confidence":1}},"usage":{"input_tokens":1,"output_tokens":1}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let output = post_stdin(
        &format!("http://{address}/decision"),
        br#"{"model":"demo","state":"pending","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    let _ = handle.join();
    assert_eq!(status_code(&output), 5);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "An answer kind differs from its question.\n");
    assert!(!stderr.contains("RESPONSE-BODY-MARKER"));
    assert!(!stderr.contains("decision-token"));
}

#[test]
fn environment_file_supplies_endpoint_and_token() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        &format!("# host file\n\nBASS_ENDPOINT=http://{address}/decision\nBASS_TOKEN=file-token\n"),
        0o600,
    );
    let request = write_request("environment-file");
    let mut command = command_in(
        &config,
        &[
            "decide",
            request.to_str().expect("request path"),
            "--timeout-seconds",
            "5",
        ],
    );
    let output = output_of(&mut command);
    let captured = handle.join().expect("server thread");
    let _ = std::fs::remove_file(&request);
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(!output
        .stdout
        .windows(b"file-token".len())
        .any(|window| window == b"file-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer file-token"));
    assert!(headers.starts_with("POST /decision "));
}

#[test]
fn flag_and_process_environment_override_the_file() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ENDPOINT=http://127.0.0.1:9/from-file\nBASS_TOKEN=file-token\n",
        0o600,
    );
    let request = write_request("override-file");
    let mut command = command_in(
        &config,
        &[
            "decide",
            "--endpoint",
            &format!("http://{address}/decision"),
            request.to_str().expect("request path"),
            "--timeout-seconds",
            "5",
        ],
    );
    command.env("BASS_ENDPOINT", format!("http://{address}/from-process"));
    command.env("BASS_TOKEN", "process-token");
    let output = output_of(&mut command);
    let captured = handle.join().expect("server thread");
    let _ = std::fs::remove_file(&request);
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert!(text.is_empty(), "{text}");
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(!stdout.contains("file-token"));
    assert!(!stdout.contains("process-token"));
    assert!(!text.contains("file-token"));
    assert!(!text.contains("process-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer process-token"));
    assert!(!headers.to_ascii_lowercase().contains("file-token"));
    assert!(headers.starts_with("POST /decision "));
}

#[test]
fn process_environment_overrides_the_file_endpoint() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ENDPOINT=http://127.0.0.1:9/from-file\nBASS_TOKEN=file-token\n",
        0o600,
    );
    let mut command = command_in(&config, &["decide", "--timeout-seconds", "5"]);
    command.env("BASS_ENDPOINT", format!("http://{address}/decision"));
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output
        .stdout
        .windows(b"file-token".len())
        .any(|window| window == b"file-token"));
    assert!(!output
        .stderr
        .windows(b"file-token".len())
        .any(|window| window == b"file-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer file-token"));
    assert!(headers.starts_with("POST /decision "));
}

#[cfg(unix)]
#[test]
fn permissive_environment_file_is_refused() {
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ENDPOINT=http://127.0.0.1:9/decision\nBASS_TOKEN=file-token\n",
        0o644,
    );
    let mut command = command_in(
        &config,
        &[
            "decide",
            "--endpoint",
            "http://127.0.0.1:9/decision",
            "--timeout-seconds",
            "5",
        ],
    );
    command.env("BASS_TOKEN", "process-token");
    let output = output_of(&mut command);
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(
        stderr,
        "The environment file permits group or other access.\n"
    );
    assert!(!stderr.contains("file-token"));
    assert!(!stderr.contains("process-token"));
    assert!(!stderr.contains(&config.display().to_string()));
    assert!(output.stdout.is_empty());
}

#[test]
fn malformed_and_duplicate_environment_files_are_rejected() {
    let malformed = fresh_config_dir();
    write_environment(&malformed, "BASS_TOKEN\n", 0o600);
    let malformed_output = output_of(&mut command_in(
        &malformed,
        &["decide", "--endpoint", "http://127.0.0.1:9/decision"],
    ));
    assert_eq!(status_code(&malformed_output), 2);
    assert_eq!(
        malformed_output.stderr,
        b"The environment file has a malformed line.\n"
    );

    let duplicate = fresh_config_dir();
    write_environment(
        &duplicate,
        "BASS_TOKEN=file-token\nBASS_TOKEN=other-token\nBASS_ENDPOINT=http://127.0.0.1:9/decision\n",
        0o600,
    );
    let duplicate_output = output_of(&mut command_in(&duplicate, &["decide"]));
    assert_eq!(status_code(&duplicate_output), 2);
    let stderr = String::from_utf8(duplicate_output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The environment file repeats a key.\n");
    assert!(!stderr.contains("file-token"));
    assert!(!stderr.contains("other-token"));
}

#[test]
fn unknown_environment_key_is_rejected() {
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_MODEL=demo\nBASS_TOKEN=file-token\nBASS_ENDPOINT=http://127.0.0.1:9/decision\n",
        0o600,
    );
    let output = output_of(&mut command_in(&config, &["decide"]));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The environment file has an unknown key.\n");
    assert!(!stderr.contains("file-token"));
    assert!(!stderr.contains("demo"));
}

#[test]
fn named_environment_file_must_exist() {
    let config = fresh_config_dir();
    let missing = config.join("token-marker-value").join("environment");
    let output = output_of(&mut command_in(
        &config,
        &[
            "decide",
            "--environment-file",
            missing.to_str().expect("missing path"),
            "--endpoint",
            "http://127.0.0.1:9/decision",
        ],
    ));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The environment file could not be read.\n");
    assert!(!stderr.contains("token-marker-value"));
}

#[test]
fn environment_file_value_is_not_shell_expanded() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    let path = config.join("named-environment");
    std::fs::write(
        &path,
        format!("BASS_ENDPOINT=http://{address}/decision\nBASS_TOKEN=$(printf file-token)\n"),
    )
    .expect("write named environment file");
    restrict_environment_file(&path, 0o600);
    let mut command = command_in(
        &config,
        &[
            "decide",
            "--environment-file",
            path.to_str().expect("environment path"),
            "--timeout-seconds",
            "5",
        ],
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output
        .stdout
        .windows(b"file-token".len())
        .any(|window| window == b"file-token"));
    assert!(!output
        .stderr
        .windows(b"file-token".len())
        .any(|window| window == b"file-token"));
    let (headers, _) = split_http(&captured);
    assert!(
        headers.contains("Authorization: Bearer $(printf file-token)")
            || headers
                .to_ascii_lowercase()
                .contains("authorization: bearer $(printf file-token)")
    );
}

#[test]
fn cleartext_endpoint_in_the_file_is_rejected() {
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ENDPOINT=http://example.invalid/decision\nBASS_TOKEN=file-token\n",
        0o600,
    );
    let output = output_of(&mut command_in(
        &config,
        &["decide", "--timeout-seconds", "5"],
    ));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "Cleartext is limited to a loopback IP literal.\n");
    assert!(!stderr.contains("file-token"));
}

#[test]
fn allowed_cleartext_host_accepts_a_credential_placeholder() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let port = address.port();
    let mut command = bass(&[
        "decide",
        "--allow-cleartext-host",
        "HOST.OPENSHELL.INTERNAL.",
        "--endpoint",
        &format!("http://HOST.OPENSHELL.INTERNAL.:{port}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "openshell:resolve:env:EXAMPLE_TOKEN");
    command.env("LD_PRELOAD", named_host_resolver());
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stdout, b"EXAMPLE_TOKEN"));
    assert!(!contains_bytes(&output.stderr, b"EXAMPLE_TOKEN"));
    assert!(!contains_bytes(&output.stdout, b"openshell:resolve:env:"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer openshell:resolve:env:example_token"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("host: host.openshell.internal:"));
    assert!(!headers
        .to_ascii_lowercase()
        .contains("host.openshell.internal."));
}

#[test]
fn allowed_cleartext_host_rejects_a_real_token() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    let mut command = bass(&[
        "decide",
        "--allow-cleartext-host",
        "host.openshell.internal",
        "--endpoint",
        &format!("http://host.openshell.internal:{port}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "live-gateway-token");
    command.env("LD_PRELOAD", named_host_resolver());
    let output = output_of(&mut command);
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(
        stderr,
        "Cleartext to a named host requires no token or an OpenShell credential placeholder.\n"
    );
    assert!(!stderr.contains("live-gateway-token"));
    assert!(output.stdout.is_empty());
    listener.set_nonblocking(true).expect("nonblocking");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn allowed_cleartext_host_accepts_an_absent_token() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let port = address.port();
    let mut command = bass(&[
        "decide",
        "--allow-cleartext-host",
        "host.openshell.internal",
        "--endpoint",
        &format!("http://host.openshell.internal:{port}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("LD_PRELOAD", named_host_resolver());
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let (headers, _) = split_http(&captured);
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("host: host.openshell.internal:"));
}

#[test]
fn empty_token_is_absent_and_overrides_a_file_token() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        &format!("BASS_ENDPOINT=http://{address}/decision\nBASS_TOKEN=file-secret-token\n"),
        0o600,
    );
    let mut command = command_in(&config, &["decide", "--timeout-seconds", "5"]);
    command.env("BASS_TOKEN", "");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stdout, b"file-secret-token"));
    assert!(!contains_bytes(&output.stderr, b"file-secret-token"));
    let (headers, _) = split_http(&captured);
    assert!(!headers.to_ascii_lowercase().contains("authorization:"));
    assert!(!headers.to_ascii_lowercase().contains("file-secret-token"));
}

#[test]
fn a_different_cleartext_host_is_rejected() {
    let mut command = bass(&[
        "decide",
        "--allow-cleartext-host",
        "host.openshell.internal",
        "--endpoint",
        "http://host.openshell.internal.example/decision",
    ]);
    command.env("BASS_TOKEN", "openshell:resolve:env:EXAMPLE_TOKEN");
    let output = output_of(&mut command);
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The cleartext host is not the allowed name.\n");
    assert!(!stderr.contains("EXAMPLE_TOKEN"));
}

#[test]
fn a_second_trailing_dot_does_not_match_the_allowed_host() {
    let mut command = bass(&[
        "decide",
        "--allow-cleartext-host",
        "host.openshell.internal",
        "--endpoint",
        "http://host.openshell.internal../decision",
    ]);
    command.env("BASS_TOKEN", "openshell:resolve:env:EXAMPLE_TOKEN");
    let output = output_of(&mut command);
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The cleartext host is not the allowed name.\n");
    assert!(!stderr.contains("EXAMPLE_TOKEN"));
}

#[test]
fn cleartext_host_allowance_is_not_read_from_the_environment() {
    let mut command = bass(&[
        "decide",
        "--endpoint",
        "http://host.openshell.internal/decision",
    ]);
    command.env("BASS_ALLOW_CLEARTEXT_HOST", "host.openshell.internal");
    command.env("BASS_TOKEN", "openshell:resolve:env:EXAMPLE_TOKEN");
    let output = output_of(&mut command);
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "Cleartext is limited to a loopback IP literal.\n");
    assert!(!stderr.contains("EXAMPLE_TOKEN"));
}

#[test]
fn cleartext_host_allowance_is_not_an_environment_file_key() {
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ALLOW_CLEARTEXT_HOST=host.openshell.internal\nBASS_ENDPOINT=http://host.openshell.internal/decision\nBASS_TOKEN=openshell:resolve:env:EXAMPLE_TOKEN\n",
        0o600,
    );
    let output = output_of(&mut command_in(&config, &["decide"]));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The environment file has an unknown key.\n");
    assert!(!stderr.contains("EXAMPLE_TOKEN"));
}

#[test]
fn an_address_is_not_a_cleartext_host_name() {
    let output = output_of(&mut bass(&["decide", "--allow-cleartext-host", "10.0.0.1"]));
    assert_eq!(status_code(&output), 2);
    assert_eq!(output.stderr, b"The cleartext host name is not valid.\n");
}

#[test]
fn token_flag_is_rejected_without_echoing_the_value() {
    let output = output_of(&mut bass(&["decide", "--token", "command-line-token"]));
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "An argument was not recognized.\n");
    assert!(!stderr.contains("command-line-token"));
}

#[test]
fn duplicate_request_keys_exit_3_without_connecting() {
    let cases = [
        (
            r#"{"model":"demo","model":"other-model","state":"pending","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
            "other-model",
        ),
        (
            r#"{"model":"demo","state":"first-state","state":"second-state","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
            "second-state",
        ),
        (
            r#"{"model":"demo","state":"pending","questions":{"eligible":{"type":"noul","instructions":"Eligible?"},"eligible":{"type":"noul","instructions":"Other?"}}}"#,
            "Other?",
        ),
        (
            r#"{"model":"demo","state":{"note":"kept","note":"dropped"},"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
            "dropped",
        ),
    ];
    for (document, marker) in cases {
        let output = reject_before_connect(document);
        assert_eq!(status_code(&output), 3, "{marker}");
        let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
        assert_eq!(stderr, "The request repeats an object key.\n");
        assert!(!stderr.contains(marker), "{marker}");
        assert!(!stderr.contains("decision-token"));
    }
}

#[test]
fn duplicate_response_key_exits_5() {
    let response = br#"{"model":"demo","model":"other-model","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let output = post_stdin(&format!("http://{address}/decision"), REQUEST_DOCUMENT);
    let _ = handle.join();
    assert_eq!(status_code(&output), 5);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The response repeats an object key.\n");
    assert!(!stderr.contains("other-model"));
    assert!(!stderr.contains("decision-token"));
}

#[test]
fn flag_endpoint_on_another_origin_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "origin-secret-token");
    command.env("BASS_ENDPOINT", "http://127.0.0.1:9/decision");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT);
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(
        stderr,
        "The endpoint origin differs from the token origin.\n"
    );
    assert!(!stderr.contains("origin-secret-token"));
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn same_origin_endpoint_is_accepted() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "origin-secret-token");
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/from-origin")),
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stdout, b"origin-secret-token"));
    assert!(!contains_bytes(&output.stderr, b"origin-secret-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer origin-secret-token"));
}

#[test]
fn placeholder_token_is_exempt_from_origin_binding() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "openshell:resolve:env:EXAMPLE_TOKEN");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let _ = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stderr, b"EXAMPLE_TOKEN"));
}

#[test]
fn token_origin_comparison_folds_host_case_and_default_ports() {
    let mut same = bass(&[
        "decide",
        "--endpoint",
        "https://example.invalid:443/decision",
        "--timeout-seconds",
        "1",
    ]);
    same.env("BASS_TOKEN", "decision-token");
    same.env("BASS_TOKEN_ORIGIN", "https://EXAMPLE.INVALID");
    same.stdin(Stdio::piped());
    same.stdout(Stdio::piped());
    same.stderr(Stdio::piped());
    let mut child = same.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 4);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_ne!(
        stderr,
        "The endpoint origin differs from the token origin.\n"
    );
    assert!(!stderr.contains("decision-token"));

    let mut command = bass(&[
        "decide",
        "--endpoint",
        "https://example.invalid/decision",
        "--timeout-seconds",
        "1",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", "https://example.invalid:444");
    let different = output_of(&mut command);
    assert_eq!(status_code(&different), 2);
    let stderr = String::from_utf8(different.stderr).expect("stderr is utf-8");
    assert_eq!(
        stderr,
        "The endpoint origin differs from the token origin.\n"
    );
    assert!(!stderr.contains("decision-token"));
}

#[test]
fn file_token_origin_binds_the_token() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        &format!(
            "BASS_ENDPOINT=http://127.0.0.1:9/unused\nBASS_TOKEN=file-origin-token\nBASS_TOKEN_ORIGIN=http://{address}\n"
        ),
        0o600,
    );
    let mut command = command_in(
        &config,
        &[
            "decide",
            "--endpoint",
            &format!("http://{address}/decision"),
            "--timeout-seconds",
            "5",
        ],
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stdout, b"file-origin-token"));
    assert!(!contains_bytes(&output.stderr, b"file-origin-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer file-origin-token"));
}

#[test]
fn process_token_origin_overrides_the_file() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let config = fresh_config_dir();
    write_environment(
        &config,
        "BASS_ENDPOINT=http://127.0.0.1:9/unused\nBASS_TOKEN=file-origin-token\nBASS_TOKEN_ORIGIN=http://127.0.0.1:9\n",
        0o600,
    );
    let mut command = command_in(
        &config,
        &[
            "decide",
            "--endpoint",
            &format!("http://{address}/decision"),
            "--timeout-seconds",
            "5",
        ],
    );
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/decision")),
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!contains_bytes(&output.stderr, b"file-origin-token"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer file-origin-token"));
}

#[test]
fn request_containing_the_token_exits_3_without_connecting() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let document = br#"{"model":"demo","state":{"note":"distinct-token-value"},"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#;
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "distinct-token-value");
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/decision")),
    );
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(document)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 3);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The request contains the token.\n");
    assert!(!stderr.contains("distinct-token-value"));
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn response_legend_and_probabilities_are_bounded() {
    let score_request = br#"{"model":"demo","state":"pending","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#;
    let choice_request = br#"{"model":"demo","state":"pending","questions":{"team":{"type":"choice","instructions":"Which team?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#;
    let long = "x".repeat(65537);
    let long_legend = format!(
        r#"{{"model":"demo","answers":{{"severity":{{"type":"score","score":1,"legend":{{"0":"{long}"}},"probabilities":{{"0":1}},"confidence":1}}}}}}"#
    );
    assert_response(
        score_request,
        long_legend.as_bytes(),
        5,
        "A legend string exceeds 65536 bytes.\n",
    );
    assert_response(
        score_request,
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"a","1":"b","2":"c"},"probabilities":{"0":1},"confidence":1}}}"#,
        5,
        "A legend has more entries than the question allows.\n",
    );
    assert_response(
        score_request,
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"minor"},"probabilities":{"0":0.2,"1":0.3,"2":0.5},"confidence":1}}}"#,
        5,
        "A probability map has more entries than the question allows.\n",
    );
    assert_response(
        choice_request,
        br#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"other":1},"confidence":1}}}"#,
        5,
        "A probability key is outside the question criteria.\n",
    );
    let (address, handle) = serve(
        200,
        "Content-Type: application/json\r\n",
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"minor","1":"major"},"probabilities":{"0":0.25,"1":0.75},"confidence":0.5}}}"#,
    );
    let output = post_stdin(&format!("http://{address}/decision"), score_request);
    let _ = handle.join();
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn retryable_and_other_statuses_use_distinct_exit_codes() {
    assert_status(503, 6, "The endpoint returned retryable status 503.\n");
    assert_status(429, 6, "The endpoint returned retryable status 429.\n");
    assert_status(404, 7, "The endpoint returned non-retryable status 404.\n");
    assert_status(401, 7, "The endpoint returned non-retryable status 401.\n");
    assert_status(403, 7, "The endpoint returned non-retryable status 403.\n");
    assert_status(400, 7, "The endpoint returned non-retryable status 400.\n");
    assert_status(413, 7, "The endpoint returned non-retryable status 413.\n");
    assert_status(415, 7, "The endpoint returned non-retryable status 415.\n");
}

#[test]
fn unknown_response_fields_are_dropped() {
    let response = br#"{"model":"demo","trace":"TOP-LEVEL-MARKER","answers":{"eligible":{"type":"noul","noul":0.25,"note":"ANSWER-MARKER"}},"usage":{"input_tokens":4,"output_tokens":0}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let output = post_stdin(&format!("http://{address}/decision"), REQUEST_DOCUMENT);
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    assert!(!stdout.contains("TOP-LEVEL-MARKER"));
    assert!(!stdout.contains("ANSWER-MARKER"));
    assert!(!stdout.contains("trace"));
    assert!(!stdout.contains("note"));
    assert!(stdout.contains("\"output_tokens\":0"));
    assert!(stdout.contains("\"noul\":0.25"));
    let (headers, _) = split_http(&captured);
    assert!(headers
        .to_ascii_lowercase()
        .contains("accept-encoding: identity"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("accept: application/json"));
}

#[test]
fn content_encoding_other_than_identity_exits_5() {
    let (address, handle) = serve(
        200,
        "Content-Type: application/json\r\nContent-Encoding: gzip\r\n",
        b"ENCODING-BODY-MARKER",
    );
    let output = post_stdin(&format!("http://{address}/decision"), REQUEST_DOCUMENT);
    let _ = handle.join();
    assert_eq!(status_code(&output), 5);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The response content encoding is not identity.\n");
    assert!(!stderr.contains("ENCODING-BODY-MARKER"));
    assert!(output.stdout.is_empty());
}

#[test]
fn score_is_an_anchor_index() {
    let request = br#"{"model":"demo","state":"pending","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","mid","high"]}}}"#;
    let (address, handle) = serve(
        200,
        "Content-Type: application/json\r\n",
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1.5,"legend":{"0":"low","2":"high"},"probabilities":{"0":0.25,"1":0.5,"2":0.25},"confidence":0.5}},"usage":{"input_tokens":3,"output_tokens":0}}"#,
    );
    let output = post_stdin(&format!("http://{address}/decision"), request);
    let _ = handle.join();
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"score\":1.5"));

    assert_response(
        request,
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":3,"legend":{"0":"low"},"probabilities":{"0":1},"confidence":1}}}"#,
        5,
        "The score is outside the anchor index range.\n",
    );
    assert_response(
        request,
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"low":"text"},"probabilities":{"0":1},"confidence":1}}}"#,
        5,
        "A legend key is outside the anchor index range.\n",
    );
    assert_response(
        request,
        br#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"low"},"probabilities":{"3":1},"confidence":1}}}"#,
        5,
        "A score probability key is outside the anchor index range.\n",
    );
}

#[test]
fn idle_request_pipe_is_bounded_by_the_timeout() {
    let mut command = bass(&[
        "decide",
        "--endpoint",
        "http://127.0.0.1:9/decision",
        "--timeout-seconds",
        "1",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", "http://127.0.0.1:9");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    let _stdin = child.stdin.take().expect("stdin");
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the request read was not bounded by the timeout");
        }
        match child.try_wait().expect("poll bass") {
            Some(status) => {
                assert_eq!(status.code(), Some(4));
                break;
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_end(&mut stderr)
        .expect("read stderr");
    assert_eq!(stderr, b"The request read timed out.\n");
    assert!(!contains_bytes(&stderr, b"decision-token"));
}

#[test]
fn oversized_integers_are_rejected_and_in_range_integers_stay_integers() {
    let output = reject_before_connect(
        r#"{"model":"demo","state":18446744073709551616,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    assert_eq!(status_code(&output), 3);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "An integer is outside the 64-bit range.\n");
    assert!(!stderr.contains("18446744073709551616"));

    let negative = reject_before_connect(
        r#"{"model":"demo","state":-9223372036854775809,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    assert_eq!(status_code(&negative), 3);
    assert_eq!(
        negative.stderr,
        b"An integer is outside the 64-bit range.\n"
    );

    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let output = post_stdin(
        &format!("http://{address}/decision"),
        br#"{"model":"demo","state":9223372036854775808,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (_, body) = split_http(&captured);
    assert!(contains_bytes(&body, b"9223372036854775808"));
    assert!(!contains_bytes(&body, b"9.223372036854776e18"));
}

#[test]
fn missing_token_origin_explains_the_configuration() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "unbound-token-value");
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT);
    let output = child.wait_with_output().expect("wait for bass");
    assert_eq!(status_code(&output), 2);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(
        stderr,
        "The token origin is not configured. Set BASS_TOKEN_ORIGIN, or set BASS_ENDPOINT.\n"
    );
    assert!(!stderr.contains("unbound-token-value"));
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn hyphen_leading_request_path_is_passed_after_end_of_options() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let directory = fresh_config_dir();
    std::fs::write(directory.join("--decision.json"), REQUEST_DOCUMENT).expect("write request");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
        "--",
        "--decision.json",
    ]);
    command.current_dir(&directory);
    command.env("BASS_TOKEN", "decision-token");
    command.env(
        "BASS_TOKEN_ORIGIN",
        origin_before_path(&format!("http://{address}/decision")),
    );
    let output = output_of(&mut command);
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(captured
        .windows(b"POST /decision ".len())
        .any(|window| window == b"POST /decision "));

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let unused = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let mut rejected = bass(&[
        "decide",
        "--endpoint",
        &format!("http://{unused}/decision"),
        "--timeout-seconds",
        "5",
        "--decision.json",
    ]);
    rejected.current_dir(&directory);
    rejected.env("BASS_TOKEN", "decision-token");
    let rejected_output = output_of(&mut rejected);
    assert_eq!(status_code(&rejected_output), 2);
    assert_eq!(rejected_output.stderr, b"An argument was not recognized.\n");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("unexpected connection: {other:?}"),
    }
}

#[test]
fn explicit_standard_input_marker_reads_a_pipe() {
    let response = br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#;
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let endpoint = format!("http://{address}/decision");
    let mut command = bass(&[
        "decide",
        "--endpoint",
        &endpoint,
        "--timeout-seconds",
        "5",
        "-",
    ]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", origin_before_path(&endpoint));
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(REQUEST_DOCUMENT)
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    let captured = handle.join().expect("server thread");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(captured
        .windows(REQUEST_DOCUMENT.len())
        .any(|window| window == REQUEST_DOCUMENT));
}

#[test]
fn omitted_request_on_a_terminal_exits_2() {
    let script = r#"
import os
import pty
import select
import sys

binary = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.execv(binary, [binary, "decide"])
chunks = []
while True:
    ready, _, _ = select.select([fd], [], [], 5)
    if not ready:
        os.kill(pid, 9)
        sys.stderr.write("timed out\n")
        sys.exit(1)
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    chunks.append(chunk)
_, status = os.waitpid(pid, 0)
code = os.waitstatus_to_exitcode(status)
output = b"".join(chunks).replace(b"\r\n", b"\n")
expected = b"The request was omitted and standard input is a terminal.\n"
if code == 2 and output.endswith(expected):
    sys.exit(0)
sys.stderr.buffer.write(output)
sys.exit(1)
"#;
    let config = fresh_config_dir();
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(env!("CARGO_BIN_EXE_bass"))
        .env_remove("BASS_TOKEN")
        .env_remove("BASS_ENDPOINT")
        .env("XDG_CONFIG_HOME", &config)
        .env("HOME", &config)
        .output()
        .expect("run terminal check");
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout {} stderr {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

const REQUEST_DOCUMENT: &[u8] = br#"{"model":"demo","state":"pending","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#;

fn write_request(name: &str) -> PathBuf {
    let number = CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("bass-{name}-{number}.json"));
    std::fs::write(&path, REQUEST_DOCUMENT).expect("write request");
    path
}

fn write_environment(config: &Path, body: &str, mode: u32) {
    let directory = config.join("bass");
    std::fs::create_dir_all(&directory).expect("create bass configuration directory");
    let path = directory.join("environment");
    std::fs::write(&path, body).expect("write environment file");
    restrict_environment_file(&path, mode);
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn named_host_resolver() -> &'static str {
    use std::sync::OnceLock;
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("bass-resolve-named-host-{}.so", std::process::id()));
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/resolve_named_host.c");
        let status = Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&path)
            .arg(&source)
            .arg("-ldl")
            .status()
            .expect("compile named host resolver");
        assert!(status.success(), "named host resolver compiled");
        path.to_str().expect("resolver path").to_string()
    })
    .as_str()
}

fn restrict_environment_file(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .expect("set environment file mode");
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

fn reject_before_connect(document: &str) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    listener.set_nonblocking(true).expect("nonblocking");
    let endpoint = format!("http://{address}/decision");
    let mut command = bass(&["decide", "--endpoint", &endpoint, "--timeout-seconds", "5"]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", origin_before_path(&endpoint));
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(document.as_bytes())
        .expect("write request");
    let output = child.wait_with_output().expect("wait for bass");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
    output
}

fn assert_response(request: &[u8], response: &[u8], code: i32, message: &str) {
    let (address, handle) = serve(200, "Content-Type: application/json\r\n", response);
    let output = post_stdin(&format!("http://{address}/decision"), request);
    let _ = handle.join();
    assert_eq!(
        status_code(&output),
        code,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, message.as_bytes());
    assert!(!contains_bytes(&output.stderr, b"decision-token"));
}

fn assert_status(status: u16, code: i32, message: &str) {
    let (address, handle) = serve(
        status,
        "Content-Type: text/plain\r\n",
        b"STATUS-BODY-MARKER",
    );
    let output = post_stdin(&format!("http://{address}/decision"), REQUEST_DOCUMENT);
    let _ = handle.join();
    assert_eq!(status_code(&output), code);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, message);
    assert!(!stderr.contains("STATUS-BODY-MARKER"));
    assert!(!stderr.contains("decision-token"));
}

fn origin_before_path(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").expect("url scheme");
    let authority = rest.split('/').next().expect("url authority");
    format!("{scheme}://{authority}")
}

fn post_stdin(endpoint: &str, document: &[u8]) -> Output {
    let mut command = bass(&["decide", "--endpoint", endpoint, "--timeout-seconds", "5"]);
    command.env("BASS_TOKEN", "decision-token");
    command.env("BASS_TOKEN_ORIGIN", origin_before_path(endpoint));
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn bass");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(document)
        .expect("write request");
    child.wait_with_output().expect("wait for bass")
}

fn serve(
    status: u16,
    extra_headers: &str,
    body: &[u8],
) -> (std::net::SocketAddr, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let extra_headers = extra_headers.to_string();
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        let mut stream = accept_within(listener, Duration::from_secs(3));
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let request = read_http(&mut stream);
        let header = format!(
            "HTTP/1.1 {status} TEST\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).expect("write header");
        stream.write_all(&body).expect("write body");
        request
    });
    (address, handle)
}

fn accept_within(listener: TcpListener, timeout: Duration) -> TcpStream {
    let started = std::time::Instant::now();
    listener.set_nonblocking(true).expect("nonblocking");
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("blocking");
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() > timeout {
                    panic!("timed out waiting for a connection");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
    }
}

fn read_http(stream: &mut TcpStream) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        if http_complete(&buffer) {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => buffer.extend_from_slice(&chunk[..count]),
            Err(error)
                if error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                break;
            }
            Err(error) => panic!("read request: {error}"),
        }
    }
    buffer
}

fn http_complete(buffer: &[u8]) -> bool {
    let Some(split) = find_bytes(buffer, b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buffer[..split]);
    let length = content_length(&headers);
    buffer.len() >= split + 4 + length
}

fn split_http(buffer: &[u8]) -> (String, Vec<u8>) {
    let split = find_bytes(buffer, b"\r\n\r\n").expect("http header terminator");
    let headers = String::from_utf8_lossy(&buffer[..split]).into_owned();
    (headers, buffer[split + 4..].to_vec())
}

fn content_length(headers: &str) -> usize {
    for line in headers.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn find_bytes(buffer: &[u8], needle: &[u8]) -> Option<usize> {
    buffer
        .windows(needle.len())
        .position(|window| window == needle)
}
