# bass

`bass decide` posts one decision request and prints the decision.

The endpoint is a full URL you supply. No host is built into the program. `BASS_TOKEN` is an optional bearer token. When it is absent, empty, or only ASCII whitespace, `bass` sends no `Authorization` header and does not read `BASS_TOKEN_ORIGIN`. A token with leading or trailing ASCII whitespace is rejected. `bass` does not accept the token as an argument, and it does not write the token or any environment-file value.

## Send a decision

Put the endpoint and the token in the environment file, then pass a request document.

```
bass decide request.json
```

You can still pass the endpoint on the command line and export the token.

```
export BASS_TOKEN="replace-with-the-token"
bass decide --endpoint "https://example.invalid/decision" request.json
```

Omit the request path to read the document from a pipe. `bass` reads standard input only when it is not a terminal.

```
generator | bass decide --endpoint "https://example.invalid/decision"
```

`example.invalid` and `gateway.example` are documentation hosts. They are not services.

On success, `bass` writes one JSON object and a newline to standard output, and it exits 0. The fields `model`, `answers`, and `usage` are written in that order, and each answer writes its own fields in a fixed order. The maps `answers`, `probabilities`, and `legend` are sorted by key.

## Request document

The document is one JSON object with `model`, `state`, and `questions`.

`model` is a non-empty string. `state` is any JSON value except null. `questions` is an object of 1 to 64 questions. Each question id is 1 to 100 characters from letters, digits, `_`, `.`, and `-`.

A `noul` question asks for a yes or no probability. `instructions` is a non-empty string. `criteria` is optional. When present, it is an object with string fields `true` and `false`.

A `choice` question picks one option. `criteria` is an object of 2 to 255 options. Each option id is 1 to 100 printable ASCII characters, and each description is a non-empty string.

A `score` question rates the state on an ordered rubric. `criteria` is an array of 2 to 10 non-empty strings, lowest first.

`bass` parses the request and sends it again in canonical form. Object keys are written in sorted order. `state` is that canonical JSON, not the original bytes. An integer literal outside the signed 64-bit range or the unsigned 64-bit range is rejected. A repeated object key is rejected.

```json
{
  "model": "demo",
  "state": "The applicant asked for a review.",
  "questions": {
    "eligible": {
      "type": "noul",
      "instructions": "Is the applicant eligible?"
    }
  }
}
```

## Response document

The response is one JSON object with `model` and `answers`. `usage` is optional. When present, it has integer fields `input_tokens` and `output_tokens`, each a non-negative integer. `output_tokens` may be 0. The decision server's gateway documentation says `usage` is always present. That statement was not verified against a live server. This is inferred. `bass` still accepts a response that omits `usage`.

`model` must equal the request model. Every question has one answer of the same type, and every answer id is a question id.

A `noul` answer has `noul`, a probability from 0 to 1. A `choice` answer has `choice`, `probabilities`, and `confidence`. `choice` is one of the option ids you sent. The decision server's documentation says those probabilities cover every option id. `bass` accepts a subset and rejects a key outside the option ids. That tolerance is deliberate. A `score` answer has `score`, `legend`, `probabilities`, and `confidence`. `score` is the probability-weighted anchor index, a finite number from 0 through one less than the number of levels in the request. It is not a 0 to 1 scale. `legend` and `probabilities` keys, when present, are the decimal index strings `0` through that last index, without a leading zero. Legend values are strings. Probabilities and confidence are numbers from 0 to 1. This score rule is inferred from the decision server's gateway documentation and was not verified against a live server.

Unknown fields at the top level of the response, and unknown fields inside each answer, are dropped. They are not stored and they are not printed. Duplicate keys are still rejected. Known fields keep their validation. Dropping unknown fields is a security decision, because response text can reach an agent. Preserving those fields is an open decision. An unknown field inside `usage` is still rejected.

`bass` sends `Accept: application/json` and `Accept-Encoding: identity`. It accepts HTTP 200 and a content type of `application/json`. A `Content-Encoding` other than `identity` exits 5 with `The response content encoding is not identity.` A missing `Content-Encoding` is identity. That exit is a decision: a retry does not change a stable coding, so the response is invalid rather than a transport failure. `bass` prints the parsed decision, not the raw response bytes.

## Environment file

`environment.example` is the committed sample. Copy it to the configuration path on each machine and remove group and other access. `bass` does not create the file. The real file stays on that machine.

On macOS and Linux, `bass` reads `$XDG_CONFIG_HOME/bass/environment` when `XDG_CONFIG_HOME` is set and not empty. Otherwise it reads `$HOME/.config/bass/environment`. On Windows, `bass` reads `$XDG_CONFIG_HOME/bass/environment` when that variable is set and not empty. Otherwise it reads `%APPDATA%\bass\environment`. An absent default file is ignored. `--environment-file PATH` names a different file, and that file must exist.

On Unix the file mode must grant no group or other access. `600` satisfies the check. `bass` rejects the file when any group or other bit is set, including when `--endpoint` and the process environment already supply both values. Windows support covers the path only. `bass` does not inspect a Windows access control list.

`--endpoint` overrides `BASS_ENDPOINT` in the process environment. The process environment overrides the file. `BASS_TOKEN` in the process environment overrides the file. An empty process value still overrides the file. An empty `BASS_TOKEN`, or one that is only ASCII whitespace, after that precedence, is absent, so `bass` sends no `Authorization` header and does not use a token from a lower-precedence source. A value that contains a token and also has leading or trailing ASCII whitespace is rejected with `BASS_TOKEN has leading or trailing whitespace.` `bass` does not trim that value before use. That refusal is inferred. When no source supplies an endpoint, `bass` exits 2 and writes `An endpoint is required.` The cleartext host allowance is not one of these keys.

The file is UTF-8 and at most 64 KiB. `bass` splits it on line feeds and removes one carriage return immediately before a line feed. It ignores a line that is empty or contains only spaces and tabs. It ignores a line whose first character, after spaces and tabs, is `#`. Every other line has the form `KEY=value`. The key is the text before the first `=` and contains no spaces. The value is the rest of the line and is literal. Quotes, dollar signs, and backticks stay as characters. `bass` does not expand them and does not execute them. A `#` later in the line does not start a comment. The allowed keys are `BASS_ENDPOINT`, `BASS_TOKEN`, and `BASS_TOKEN_ORIGIN`. A repeated key is rejected. Any other key is rejected. The model remains a field of the request document.

`BASS_TOKEN_ORIGIN` applies only when a real token is present. When no token is configured, `bass` does not read `BASS_TOKEN_ORIGIN`. It has a scheme, a host, and an optional port, and it has no path. A trailing slash is accepted. Any other path is rejected. When `BASS_TOKEN_ORIGIN` is set, in the process environment or in the file, that origin is the only origin to which `bass` sends the token. The process value overrides the file. When it is unset, `bass` uses the origin of `BASS_ENDPOINT` from the process environment or the file. The endpoint in use, including an endpoint given by `--endpoint`, must have that origin. Host comparison ignores ASCII case, and an omitted port is the scheme default. An absent token and a credential placeholder are not bound to an origin. This binding is inferred. Acceptance of a trailing slash is inferred. Treating an empty token as absent is the configured behavior. An empty process value overriding a file token follows the empty-override rule. That interaction is inferred.

## Flags

`bass` with no subcommand writes usage and exits 2. `bass --help` names the subcommand. `bass --version` writes the package version. Both are sole arguments. `bass decide --help` writes the options for `decide`.

`REQUEST` is one optional positional argument. It is a file path, or `-` for standard input. When `REQUEST` is omitted, `bass` reads standard input if it is not a terminal. When standard input is a terminal, `bass` exits 2 and does not read it. A path that begins with a hyphen, other than a lone `-`, is written after `--`.

`--endpoint URL` sets the decision URL for one invocation. `--environment-file PATH` selects an environment file. `--allow-cleartext-host NAME` permits plain HTTP to one hostname. `--timeout-seconds N` bounds the request read and the exchange together. The default is 60. The maximum is 600. An idle pipe exits 4 when that limit is reached.

`--allow-cleartext-host` is accepted once. It is read only from the command line. A process variable or an environment-file key does not enable it, and an unknown file key is rejected. It permits plain HTTP to that hostname when no token is configured, or when the token is an OpenShell credential placeholder. A real token is refused. The name matches one hostname after normalization. There is no wildcard, suffix, prefix, or address range. One trailing dot is removed, then ASCII letters are compared without regard to case. A second trailing dot does not match. An IP address is not a valid name. Loopback IP literals stay allowed without this flag. Cleartext to any other non-loopback host stays refused.

## Exit status

| Code | Meaning | Retry |
| --- | --- | --- |
| 0 | The decision, the help text, or the version was written. | |
| 1 | Standard output could not be written. | |
| 2 | The invocation or the configuration was rejected. | Do not retry the same invocation. |
| 3 | The request document was rejected. `bass` does not connect. | Do not retry the same document. |
| 4 | The transport failed, the connection failed, the exchange timed out, or the request read timed out. | Retry may help. |
| 5 | The response document was rejected, or the content encoding is not identity. | Do not retry the same exchange. |
| 6 | The endpoint returned HTTP 429 or an HTTP 5xx status. | Retry with backoff may help. |
| 7 | The endpoint returned any other non-200 status. | Do not retry. |

Error text is a fixed sentence. It names the status class and does not include the response body. It does not include the token, an environment-file value, or the endpoint.

| Code | Message |
| --- | --- |
| 4 | `The request read timed out.` or `The exchange timed out.` or `The endpoint could not be reached.` or `The exchange failed.` |
| 5 | `The response content encoding is not identity.` |
| 6 | `The endpoint returned retryable status N.` |
| 7 | `The endpoint returned non-retryable status N.` |

## Limits

By default, cleartext HTTP is allowed only for a loopback IP literal such as `127.0.0.1` or `::1`. A hostname, including `localhost`, is rejected unless `--allow-cleartext-host` names that host. With that flag, `bass` accepts no token or an OpenShell credential placeholder. A real token is refused before a connection, and the token is not written. The placeholder form is `openshell:resolve:env:` followed by one or more characters from `A` to `Z`, digits, and `_`. That form is taken from OpenShell documentation and the maintainer's existing profile. It is an inference to be confirmed. A request whose serialized form contains the token is rejected before a connection is opened. Redirects are not followed. `HTTP_PROXY` and the related variables are not used. A URL with user information is rejected. Request and response bodies are limited to 1 MiB. The request read shares the timeout with the exchange. HTTPS uses the rustls webpki roots.

Sending no token means the request is unauthenticated. The server decides whether to accept it. HTTP 401 and HTTP 403 still exit 7. Plain HTTP over a network the operator trusts, for example an encrypted private overlay network, is the operator's responsibility. `bass` cannot verify what that network provides. This limit is inferred.

A repeated object key is rejected in the request and in the response. Nesting deeper than 512 levels is rejected. This depth limit is inferred. Each `legend` string is limited to 65536 bytes. This cap is inferred. A `score` answer has at most as many `legend` entries and `probabilities` entries as the question has levels. Those keys must be anchor index strings. This is inferred from the decision server's gateway documentation. A `choice` answer has at most as many `probabilities` entries as the question has options, and those keys are a subset of the option ids. The shared timeout that bounds the request read is inferred.

The decision server's gateway documentation states limits that are stricter than the checks `bass` applies to a request. They were not verified against a live server. This is inferred. `bass` does not enforce them. A request the server rejects is reported from the HTTP status. HTTP 400, 413, and 415 exit 7 with `The endpoint returned non-retryable status N.`

| Decision server limit | `bass` check |
| --- | --- |
| Request body at most 262144 bytes | 1 MiB |
| `state` at most 16 levels deep | 512 levels |
| Question ids of 1 to 128 characters | 1 to 100 characters |
| 1 to 64 questions | 1 to 64 questions |
| 1 to 64 choice options | 2 to 255 choice options |
| 1 to 64 score anchors | 2 to 10 score levels |
| Instructions at most 8192 characters | No separate instruction cap |

## Supported deployments

These are the two situations `bass` is intended to serve. The examples use placeholder values only.

### Direct use on a host machine

The real token stays in the mode `600` environment file or in the process environment. The endpoint is HTTPS. The cleartext allowance is omitted. Plain HTTP remains available only for a loopback IP literal.

```
# ~/.config/bass/environment on macOS and Linux.
# %APPDATA%\bass\environment on Windows when XDG_CONFIG_HOME is unset.
BASS_ENDPOINT=https://gateway.example/v1
BASS_TOKEN=replace-with-the-token
BASS_TOKEN_ORIGIN=https://gateway.example
```

```
bass decide request.json
```

On this side the default stays strict. A real token is not sent to a named host over cleartext. That path accepts no token or a credential placeholder, and it rejects a real token before connecting.

### A private network without a credential

Omit `BASS_TOKEN`. `bass` sends no `Authorization` header. If the decision server is a named host on a network the operator trusts, pass `--allow-cleartext-host` with that hostname. `decision.overlay.example` is a documentation host. It is not a service.

```
bass decide --allow-cleartext-host decision.overlay.example \
  --endpoint "http://decision.overlay.example:8080/decision" \
  request.json
```

The request is unauthenticated. The server decides whether to accept it. HTTP 401 and HTTP 403 exit 7. The operator is responsible for what the network provides. `bass` cannot verify that protection. This is inferred.

### Inside an OpenShell sandbox

The sandbox holds the placeholder token and the host-local relay URL. Run `bass` with the allowance set to that relay name. The real token and the real gateway name stay outside the sandbox.

```
export BASS_TOKEN="openshell:resolve:env:EXAMPLE_TOKEN"
bass decide --allow-cleartext-host host.openshell.internal \
  --endpoint "http://host.openshell.internal:8080/decision" \
  request.json
```

The following statements about OpenShell were not verified from this repository. They are inferred from OpenShell documentation and the maintainer's existing profile. The OpenShell proxy substitutes the real credential only when its policy permits the binary, host, port, path, and method. A compromised sandbox therefore yields only the call that policy already allows. Adding `bass` to the allowed binaries adds no new class of access beyond the other binaries that policy already permits. Narrow the profile to the decision path and to POST only. That is a recommendation for the profile owner. `bass` does not enforce the profile.

If the relay returns 403 with a credential endpoint mismatch, the profile does not permit this binary, host, port, path, or method. Adjust the profile. Do not replace the placeholder with a real token inside the sandbox. A credential placeholder is not bound to a token origin.

## Using bass from an agent skill

Build each request with `jq -n --arg`, `jq -n --argjson`, or an equivalent encoder that escapes untrusted text. Do not build the document by string interpolation or with a heredoc.

Read the numeric fields and the enumerated fields from the response. Treat every `legend` string as untrusted text.

Keep secrets out of `state`. The decision server receives `state`, and `bass` rejects a serialized request that contains the token.

`bass` does not define the `score` scale, the calibration of `noul`, or the meaning of `confidence`. Those semantics belong to the decision server's contract.

`bass` checks that the response `model` equals the request `model`. It does not pin or verify the model version behind that name.

## Publication

Version 0.0.1 on crates.io is the placeholder program. This source replaces that program. Publishing it requires a new version number. This tree does not publish.
