use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static CONFIG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Case {
    name: &'static str,
    document: String,
    valid: bool,
}

fn fresh_config_dir() -> PathBuf {
    let number = CONFIG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "bass-schema-config-{}-{number}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("create configuration directory");
    path
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
    command
}

fn bass(args: &[&str]) -> Command {
    command_in(&fresh_config_dir(), args)
}

fn status_code(output: &Output) -> i32 {
    output.status.code().expect("bass exit status")
}

fn run_with_stdin(args: &[&str], document: &[u8]) -> Output {
    let mut command = bass(args);
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

fn request_validator() -> jsonschema::Validator {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema/decision-request.schema.json");
    let text = std::fs::read_to_string(&path).expect("read request schema");
    let schema: Value = serde_json::from_str(&text).expect("request schema is JSON");
    jsonschema::draft202012::options()
        .build(&schema)
        .expect("request schema compiles")
}

fn assert_agreement(validator: &jsonschema::Validator, case: &Case) {
    let output = run_with_stdin(&["decide", "--check", "-"], case.document.as_bytes());
    let code = status_code(&output);
    let bass_ok = code == 0;
    assert_eq!(
        bass_ok,
        case.valid,
        "{} exit {code} stderr {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{}", case.name);
    if case.valid {
        assert!(output.stderr.is_empty(), "{}", case.name);
    } else {
        assert_eq!(code, 3, "{}", case.name);
        assert!(output.stderr.ends_with(b"\n"), "{}", case.name);
    }
    let schema_ok = match serde_json::from_str::<Value>(&case.document) {
        Ok(value) => validator.is_valid(&value),
        Err(_) => false,
    };
    if schema_ok != bass_ok {
        let detail = match serde_json::from_str::<Value>(&case.document) {
            Ok(value) => match validator.validate(&value) {
                Ok(()) => String::new(),
                Err(errors) => errors
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
            },
            Err(error) => error.to_string(),
        };
        panic!(
            "{}: schema accepted {schema_ok}, bass accepted {bass_ok}: {detail}",
            case.name
        );
    }
}

fn valid(name: &'static str, document: &str) -> Case {
    Case {
        name,
        document: document.to_string(),
        valid: true,
    }
}

fn invalid(name: &'static str, document: &str) -> Case {
    Case {
        name,
        document: document.to_string(),
        valid: false,
    }
}

fn questions(count: usize) -> String {
    let mut questions = serde_json::Map::new();
    for index in 0..count {
        questions.insert(
            format!("q{index}"),
            serde_json::json!({"type": "noul", "instructions": "Yes?"}),
        );
    }
    serde_json::json!({
        "model": "demo",
        "state": "text",
        "questions": questions
    })
    .to_string()
}

fn choices(count: usize) -> String {
    let mut criteria = serde_json::Map::new();
    for index in 0..count {
        criteria.insert(format!("c{index}"), Value::String("An option.".to_string()));
    }
    serde_json::json!({
        "model": "demo",
        "state": "text",
        "questions": {
            "team": {
                "type": "choice",
                "instructions": "Which?",
                "criteria": criteria
            }
        }
    })
    .to_string()
}

fn score_levels(count: usize) -> String {
    let levels: Vec<String> = (0..count).map(|index| format!("level {index}")).collect();
    serde_json::json!({
        "model": "demo",
        "state": "text",
        "questions": {
            "severity": {
                "type": "score",
                "instructions": "How severe?",
                "criteria": levels
            }
        }
    })
    .to_string()
}

fn sized_request(total: usize) -> String {
    let prefix = r#"{"model":"demo","state":""#;
    let suffix = r#"","questions":{"q":{"type":"noul","instructions":"Yes?"}}}"#;
    let pad = total - prefix.len() - suffix.len();
    let mut document = String::with_capacity(total);
    document.push_str(prefix);
    document.extend(std::iter::repeat('a').take(pad));
    document.push_str(suffix);
    assert_eq!(document.len(), total);
    document
}

fn agreement_cases() -> Vec<Case> {
    let question_id_100 = "a".repeat(100);
    let question_id_101 = "a".repeat(101);
    let choice_id_100 = "~".repeat(100);
    let choice_id_101 = "~".repeat(101);
    let mut cases = vec![
        valid(
            "minimal noul",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "noul criteria",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","criteria":{"true":"Meets the rule.","false":"Does not."}}}}"#,
        ),
        valid(
            "choice",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#,
        ),
        valid(
            "choice ids with space and punctuation",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"net ops":"Network.","~":"Tilde."}}}}"#,
        ),
        valid(
            "score",
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#,
        ),
        valid(
            "all three question types",
            r#"{"model":"demo","state":{"invoice":1842},"questions":{"eligible":{"type":"noul","instructions":"Eligible?"},"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"Payments","technical":"Outages"}},"severity":{"type":"score","instructions":"How severe?","criteria":["low","medium","high"]}}}"#,
        ),
        valid(
            "state number",
            r#"{"model":"demo","state":0,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state boolean",
            r#"{"model":"demo","state":false,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state empty string",
            r#"{"model":"demo","state":"","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state empty array",
            r#"{"model":"demo","state":[],"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state empty object",
            r#"{"model":"demo","state":{},"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state integer at i64 minimum",
            r#"{"model":"demo","state":-9223372036854775808,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state integer at u64 maximum",
            r#"{"model":"demo","state":18446744073709551615,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state whole number with a fraction",
            r#"{"model":"demo","state":1.0,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "state exponent",
            r#"{"model":"demo","state":1e2,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        valid(
            "unicode model and instructions",
            r#"{"model":"決定","state":"text","questions":{"eligible":{"type":"noul","instructions":"適格ですか"}}}"#,
        ),
        valid(
            "question id punctuation",
            r#"{"model":"demo","state":"text","questions":{"team.primary_1-a":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "empty model",
            r#"{"model":"","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "missing model",
            r#"{"state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "model number",
            r#"{"model":1,"state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "null state",
            r#"{"model":"demo","state":null,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "missing state",
            r#"{"model":"demo","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "missing questions",
            r#"{"model":"demo","state":"text"}"#,
        ),
        invalid(
            "empty questions",
            r#"{"model":"demo","state":"text","questions":{}}"#,
        ),
        invalid(
            "question id with a space",
            r#"{"model":"demo","state":"text","questions":{"bad id":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "empty question id",
            r#"{"model":"demo","state":"text","questions":{"":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "unicode question id",
            r#"{"model":"demo","state":"text","questions":{"質問":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "unknown field",
            r#"{"model":"demo","state":"text","note":1,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "unknown question field",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","note":1}}}"#,
        ),
        invalid(
            "empty instructions",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":""}}}"#,
        ),
        invalid(
            "unknown question type",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"other","instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "missing question type",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"instructions":"Eligible?"}}}"#,
        ),
        invalid(
            "noul criteria missing a side",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","criteria":{"true":"Meets the rule."}}}}"#,
        ),
        invalid(
            "noul criteria empty string",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","criteria":{"true":"","false":"No."}}}}"#,
        ),
        invalid(
            "noul criteria extra field",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","criteria":{"true":"Yes.","false":"No.","note":"Extra."}}}}"#,
        ),
        invalid(
            "noul criteria null",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?","criteria":null}}}"#,
        ),
        invalid(
            "choice with one option",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"only":"one"}}}}"#,
        ),
        invalid(
            "choice empty description",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"","technical":"Outages"}}}}"#,
        ),
        invalid(
            "choice id with a newline",
            "{\"model\":\"demo\",\"state\":\"text\",\"questions\":{\"team\":{\"type\":\"choice\",\"instructions\":\"Which?\",\"criteria\":{\"bad\\nid\":\"One.\",\"technical\":\"Outages.\"}}}}",
        ),
        invalid(
            "choice id delete",
            "{\"model\":\"demo\",\"state\":\"text\",\"questions\":{\"team\":{\"type\":\"choice\",\"instructions\":\"Which?\",\"criteria\":{\"bad\\u007fid\":\"One.\",\"technical\":\"Outages.\"}}}}",
        ),
        invalid(
            "empty choice id",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"":"One.","technical":"Outages."}}}}"#,
        ),
        invalid(
            "choice criteria array",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":["billing","technical"]}}}"#,
        ),
        invalid(
            "choice missing criteria",
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?"}}}"#,
        ),
        invalid(
            "score with one level",
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low"]}}}"#,
        ),
        invalid(
            "score empty level",
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low",""]}}}"#,
        ),
        invalid(
            "score criteria object",
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":{"low":"high"}}}}"#,
        ),
        invalid(
            "score missing criteria",
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?"}}}"#,
        ),
        invalid("top-level array", r#"["model"]"#),
        invalid("not json", "not-json"),
        invalid(
            "questions array",
            r#"{"model":"demo","state":"text","questions":[{"type":"noul","instructions":"Eligible?"}]}"#,
        ),
        invalid(
            "question string",
            r#"{"model":"demo","state":"text","questions":{"eligible":"noul"}}"#,
        ),
        invalid(
            "instructions number",
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":1}}}"#,
        ),
        invalid("trailing value", r#"{"model":"demo"}{}"#),
    ];
    cases.push(Case {
        name: "question id length 100",
        document: format!(
            r#"{{"model":"demo","state":"text","questions":{{"{question_id_100}":{{"type":"noul","instructions":"Eligible?"}}}}}}"#
        ),
        valid: true,
    });
    cases.push(Case {
        name: "question id length 101",
        document: format!(
            r#"{{"model":"demo","state":"text","questions":{{"{question_id_101}":{{"type":"noul","instructions":"Eligible?"}}}}}}"#
        ),
        valid: false,
    });
    cases.push(Case {
        name: "choice id length 100",
        document: format!(
            r#"{{"model":"demo","state":"text","questions":{{"team":{{"type":"choice","instructions":"Which?","criteria":{{"{choice_id_100}":"One.","other":"Two."}}}}}}}}"#
        ),
        valid: true,
    });
    cases.push(Case {
        name: "choice id length 101",
        document: format!(
            r#"{{"model":"demo","state":"text","questions":{{"team":{{"type":"choice","instructions":"Which?","criteria":{{"{choice_id_101}":"One.","other":"Two."}}}}}}}}"#
        ),
        valid: false,
    });
    cases.push(Case {
        name: "64 questions",
        document: questions(64),
        valid: true,
    });
    cases.push(Case {
        name: "65 questions",
        document: questions(65),
        valid: false,
    });
    cases.push(Case {
        name: "255 options",
        document: choices(255),
        valid: true,
    });
    cases.push(Case {
        name: "256 options",
        document: choices(256),
        valid: false,
    });
    cases.push(Case {
        name: "10 score levels",
        document: score_levels(10),
        valid: true,
    });
    cases.push(Case {
        name: "11 score levels",
        document: score_levels(11),
        valid: false,
    });
    cases.push(Case {
        name: "request of 1048576 bytes",
        document: sized_request(1024 * 1024),
        valid: true,
    });
    cases.push(Case {
        name: "state nested 20 arrays",
        document: nested_request(20),
        valid: true,
    });
    cases
}

fn nested_request(array_depth: usize) -> String {
    let mut document = String::from(r#"{"model":"demo","state":"#);
    document.extend(std::iter::repeat('[').take(array_depth));
    document.extend(std::iter::repeat(']').take(array_depth));
    document.push_str(r#","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#);
    document
}

fn nested_state_value(array_depth: usize) -> Value {
    let mut value = Value::Array(Vec::new());
    for _ in 1..array_depth {
        value = Value::Array(vec![value]);
    }
    value
}

fn request_value_with_state(state: Value) -> Value {
    serde_json::json!({
        "model": "demo",
        "state": state,
        "questions": {
            "eligible": {"type": "noul", "instructions": "Eligible?"}
        }
    })
}

#[test]
fn request_schema_agrees_with_check() {
    let validator = request_validator();
    for case in agreement_cases() {
        assert_agreement(&validator, &case);
    }
}

#[test]
fn request_schema_diverges_where_it_cannot_match_check() {
    let validator = request_validator();
    let rows = [
        (
            "repeated object key",
            r#"{"model":"demo","model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#.to_string(),
            "The request repeats an object key.\n",
        ),
        (
            "integer above u64",
            r#"{"model":"demo","state":18446744073709551616,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#.to_string(),
            "An integer is outside the 64-bit range.\n",
        ),
        (
            "integer below i64",
            r#"{"model":"demo","state":-9223372036854775809,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#.to_string(),
            "An integer is outside the 64-bit range.\n",
        ),
        (
            "nesting deeper than 512",
            nested_request(512),
            "The request nests too deeply.\n",
        ),
    ];
    for (name, document, stderr) in rows {
        let output = run_with_stdin(&["decide", "--check", "-"], document.as_bytes());
        assert_eq!(status_code(&output), 3, "{name}");
        assert_eq!(output.stderr, stderr.as_bytes(), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        let value = if name == "nesting deeper than 512" {
            request_value_with_state(nested_state_value(512))
        } else {
            serde_json::from_str(&document).unwrap_or_else(|error| {
                panic!("{name}: schema parser rejected the document: {error}")
            })
        };
        assert!(
            validator.is_valid(&value),
            "{name}: the request schema rejected a document whose only fault is a rule it cannot express"
        );
    }

    let within_depth = run_with_stdin(&["decide", "--check", "-"], nested_request(511).as_bytes());
    assert_eq!(status_code(&within_depth), 0);
    assert!(within_depth.stdout.is_empty());
    assert!(within_depth.stderr.is_empty());
    assert!(validator.is_valid(&request_value_with_state(nested_state_value(511))));
}

#[test]
fn check_does_not_apply_the_token_rule() {
    let document = r#"{"model":"demo","state":"distinct-token-value","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#;
    let mut command = bass(&["decide", "--check", "-"]);
    command.env("BASS_TOKEN", "distinct-token-value");
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
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_str(document).expect("document is JSON");
    assert!(
        request_validator().is_valid(&value),
        "the request schema cannot see the token, and --check does not apply that rule"
    );
}

#[test]
fn encoded_size_limit_is_outside_the_request_schema() {
    let validator = request_validator();
    let document = sized_request(1024 * 1024 + 1);
    let output = run_with_stdin(&["decide", "--check", "-"], document.as_bytes());
    assert_eq!(status_code(&output), 3);
    assert_eq!(output.stderr, b"The request is too large.\n");
    assert!(output.stdout.is_empty());
    let value: Value = serde_json::from_str(&document).expect("oversize document is JSON");
    assert!(
        validator.is_valid(&value),
        "the request schema rejected a document whose only fault is the bass byte limit"
    );
}

#[test]
fn check_ignores_endpoint_token_and_environment_file() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    let config = fresh_config_dir();
    let directory = config.join("bass");
    std::fs::create_dir_all(&directory).expect("create bass configuration directory");
    let path = directory.join("environment");
    std::fs::write(
        &path,
        "BASS_ENDPOINT=http://127.0.0.1:9/from-file\nBASS_TOKEN=file-token-marker\n",
    )
    .expect("write environment file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("set permissive mode");
    }
    let request = config.join("request.json");
    std::fs::write(
        &request,
        r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    )
    .expect("write request");
    let mut command = command_in(
        &config,
        &["decide", "--check", request.to_str().expect("request path")],
    );
    command.env("BASS_ENDPOINT", format!("http://{address}/decision"));
    command.env("BASS_TOKEN", "process-token-marker");
    let output = command.output().expect("run bass");
    assert_eq!(
        status_code(&output),
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).expect("stdout is utf-8")
        + &String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert!(!text.contains("process-token-marker"));
    assert!(!text.contains("file-token-marker"));
    listener.set_nonblocking(true).expect("nonblocking");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("expected no connection, got {other:?}"),
    }
}

#[test]
fn check_uses_the_request_invalid_message() {
    let output = run_with_stdin(
        &["decide", "--check", "-"],
        br#"{"model":"demo","state":null,"questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    assert_eq!(status_code(&output), 3);
    assert_eq!(output.stderr, b"The state is null.\n");
    assert!(output.stdout.is_empty());
}

#[test]
fn check_rejects_network_options() {
    let rejected = "--check does not accept --timeout-seconds, --environment-file, or --allow-cleartext-host.\n";
    let cases = [
        (
            &["decide", "--check", "--timeout-seconds", "5"][..],
            rejected,
        ),
        (
            &[
                "decide",
                "--check",
                "--environment-file",
                "missing-environment-marker",
            ][..],
            rejected,
        ),
        (
            &[
                "decide",
                "--check",
                "--allow-cleartext-host",
                "example.invalid",
            ][..],
            rejected,
        ),
        (
            &[
                "decide",
                "--check",
                "--print-response-schema",
                "--timeout-seconds",
                "5",
            ][..],
            rejected,
        ),
        (
            &["decide", "--print-response-schema"][..],
            "--print-response-schema requires --check.\n",
        ),
        (
            &[
                "decide",
                "--print-response-schema",
                "--timeout-seconds",
                "5",
            ][..],
            "--print-response-schema requires --check.\n",
        ),
        (
            &["decide", "--check", "--timeout-seconds", "0"][..],
            "The timeout is outside the allowed range.\n",
        ),
        (
            &["decide", "--check", "--allow-cleartext-host", "10.0.0.1"][..],
            "The cleartext host name is not valid.\n",
        ),
        (
            &["decide", "--check", "--check"][..],
            "A flag was repeated.\n",
        ),
    ];
    for (args, stderr) in cases {
        let output = bass(args).output().expect("run bass");
        assert_eq!(status_code(&output), 2, "{args:?}");
        assert_eq!(output.stderr, stderr.as_bytes(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let text = String::from_utf8(output.stderr.clone()).expect("stderr is utf-8");
        assert!(!text.contains("missing-environment-marker"), "{args:?}");
    }
}

#[test]
fn print_response_schema_matches_golden_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/response_schema");
    for name in ["noul", "choice", "score", "mixed"] {
        let request = root.join(format!("{name}.request.json"));
        let expected = std::fs::read(root.join(format!("{name}.schema.json")))
            .unwrap_or_else(|_| panic!("read {name} schema"));
        let printed = bass(&[
            "decide",
            "--check",
            "--print-response-schema",
            request.to_str().expect("request path"),
        ])
        .output()
        .expect("run bass");
        assert_eq!(
            status_code(&printed),
            0,
            "{}",
            String::from_utf8_lossy(&printed.stderr)
        );
        assert!(printed.stderr.is_empty(), "{name}");
        assert_eq!(printed.stdout, expected, "{name}");

        let checked = bass(&["decide", "--check", request.to_str().expect("request path")])
            .output()
            .expect("run bass");
        assert_eq!(status_code(&checked), 0, "{name}");
        assert!(checked.stdout.is_empty(), "{name}");
        assert!(checked.stderr.is_empty(), "{name}");
    }
}

#[test]
fn print_response_schema_writes_nothing_for_an_invalid_request() {
    let output = run_with_stdin(
        &["decide", "--check", "--print-response-schema", "-"],
        br#"{"model":"","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
    );
    assert_eq!(status_code(&output), 3);
    assert_eq!(output.stderr, b"The model is empty.\n");
    assert!(output.stdout.is_empty());
}

#[test]
fn decide_help_documents_check_and_the_response_schema() {
    let output = bass(&["decide", "--help"]).output().expect("run bass");
    assert_eq!(status_code(&output), 0);
    let text = String::from_utf8(output.stdout).expect("help is utf-8");
    for phrase in [
        "--check",
        "--print-response-schema",
        "does not read BASS_ENDPOINT, BASS_TOKEN, or an environment file",
        "cannot test whether the request contains the token",
        "valid only together with --check",
        "draft 2020-12",
    ] {
        assert!(text.contains(phrase), "{phrase} missing from decide help");
    }
    assert!(output.stderr.is_empty());
}

#[test]
fn omitted_check_on_a_terminal_exits_2() {
    let script = r#"
import os
import pty
import select
import sys

binary = sys.argv[1]
pid, fd = pty.fork()
if pid == 0:
    os.execv(binary, [binary, "decide", "--check"])
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
