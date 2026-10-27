use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

fn bass(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bass"));
    command.args(args);
    command.stdin(Stdio::null());
    command.env_remove("BASS_TOKEN");
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
    for name in [
        "--endpoint",
        "--request",
        "--timeout-seconds",
        "--help",
        "--version",
        "BASS_TOKEN",
    ] {
        assert!(text.contains(name), "{name} missing from help");
    }
    assert!(!text.contains("Bearer"));
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_endpoint_exits_2() {
    let output = output_of(&mut bass(&[]));
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
fn missing_token_exits_2() {
    let output = output_of(&mut bass(&["--endpoint", "http://127.0.0.1:9/decision"]));
    assert_eq!(status_code(&output), 2);
    assert_eq!(output.stderr, b"BASS_TOKEN is missing.\n");
}

#[test]
fn cleartext_hostname_is_rejected() {
    let output = output_of(&mut bass(&[
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
        "--endpoint",
        "https://example.invalid/decision",
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
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
        "--endpoint",
        &format!("http://{address}/decision"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
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
        "--endpoint",
        &format!("http://{address}/decision"),
        "--request",
        path.to_str().expect("temp path"),
        "--timeout-seconds",
        "5",
    ]);
    command.env("BASS_TOKEN", "decision-token");
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
    assert_eq!(status_code(&output), 4);
    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert_eq!(stderr, "The endpoint returned redirect status 302.\n");
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

fn post_stdin(endpoint: &str, document: &[u8]) -> Output {
    let mut command = bass(&["--endpoint", endpoint, "--timeout-seconds", "5"]);
    command.env("BASS_TOKEN", "decision-token");
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
