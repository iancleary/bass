# bass

`bass decide` posts one decision request and prints the decision.

The endpoint is a full URL you supply. No host is built into the program. The bearer token is `BASS_TOKEN`. `bass` does not accept the token as an argument, and it does not write the token or any environment-file value.

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

On success, `bass` writes one JSON object and a newline to standard output, and it exits 0. Object keys in that object are sorted.

## Request document

The document is one JSON object with `model`, `state`, and `questions`.

`model` is a non-empty string. `state` is any JSON value except null. `questions` is an object of 1 to 64 questions. Each question id is 1 to 100 characters from letters, digits, `_`, `.`, and `-`.

A `noul` question asks for a yes or no probability. `instructions` is a non-empty string. `criteria` is optional. When present, it is an object with string fields `true` and `false`.

A `choice` question picks one option. `criteria` is an object of 2 to 255 options. Each option id is 1 to 100 printable ASCII characters, and each description is a non-empty string.

A `score` question rates the state on an ordered rubric. `criteria` is an array of 2 to 10 non-empty strings, lowest first.

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

The response is one JSON object with `model` and `answers`. `usage` is optional. When present, it has integer fields `input_tokens` and `output_tokens`.

`model` must equal the request model. Every question has one answer of the same type, and every answer id is a question id.

A `noul` answer has `noul`, a probability from 0 to 1. A `choice` answer has `choice`, `probabilities`, and `confidence`. `choice` is one of the option ids you sent. A `score` answer has `score`, `legend`, `probabilities`, and `confidence`. `score` is a finite number. Probabilities and confidence are numbers from 0 to 1. Legend values are strings.

`bass` accepts HTTP 200 and a content type of `application/json`. It prints the parsed decision, not the raw response bytes.

## Environment file

`environment.example` is the committed sample. Copy it to the configuration path on each machine and remove group and other access. `bass` does not create the file. The real file stays on that machine.

On macOS and Linux, `bass` reads `$XDG_CONFIG_HOME/bass/environment` when `XDG_CONFIG_HOME` is set and not empty. Otherwise it reads `$HOME/.config/bass/environment`. On Windows, `bass` reads `$XDG_CONFIG_HOME/bass/environment` when that variable is set and not empty. Otherwise it reads `%APPDATA%\bass\environment`. An absent default file is ignored. `--environment-file PATH` names a different file, and that file must exist.

On Unix the file mode must grant no group or other access. `600` satisfies the check. `bass` rejects the file when any group or other bit is set, including when `--endpoint` and the process environment already supply both values. Windows support covers the path only. `bass` does not inspect a Windows access control list.

`--endpoint` overrides `BASS_ENDPOINT` in the process environment. The process environment overrides the file. `BASS_TOKEN` in the process environment overrides the file. An empty process value still overrides the file. When no source supplies an endpoint, `bass` exits 2 and writes `An endpoint is required.` The cleartext host allowance is not one of these keys.

The file is UTF-8 and at most 64 KiB. `bass` splits it on line feeds and removes one carriage return immediately before a line feed. It ignores a line that is empty or contains only spaces and tabs. It ignores a line whose first character, after spaces and tabs, is `#`. Every other line has the form `KEY=value`. The key is the text before the first `=` and contains no spaces. The value is the rest of the line and is literal. Quotes, dollar signs, and backticks stay as characters. `bass` does not expand them and does not execute them. A `#` later in the line does not start a comment. The allowed keys are `BASS_ENDPOINT` and `BASS_TOKEN`. A repeated key is rejected. Any other key is rejected. The model remains a field of the request document.

## Flags

`bass` with no subcommand writes usage and exits 2. `bass --help` names the subcommand. `bass --version` writes the package version. Both are sole arguments. `bass decide --help` writes the options for `decide`.

`REQUEST` is one optional positional argument. It is a file path, or `-` for standard input. When `REQUEST` is omitted, `bass` reads standard input if it is not a terminal. When standard input is a terminal, `bass` exits 2 and does not read it. A path that begins with a hyphen, other than a lone `-`, is written after `--`.

`--endpoint URL` sets the decision URL for one invocation. `--environment-file PATH` selects an environment file. `--allow-cleartext-host NAME` permits plain HTTP to one hostname. `--timeout-seconds N` stops the exchange after N seconds. The default is 60. The maximum is 600.

`--allow-cleartext-host` is accepted once. It is read only from the command line. A process variable or an environment-file key does not enable it, and an unknown file key is rejected. The name matches one hostname after normalization. There is no wildcard, suffix, prefix, or address range. One trailing dot is removed, then ASCII letters are compared without regard to case. A second trailing dot does not match. An IP address is not a valid name. Loopback IP literals stay allowed without this flag.

## Exit status

- 0 means the decision, the help text, or the version was written.
- 1 means standard output could not be written.
- 2 means the invocation was rejected.
- 3 means the request document was rejected. `bass` does not connect.
- 4 means the exchange failed.
- 5 means the response document was rejected.

Error text is a fixed sentence. It does not include the token, an environment-file value, the response body, or the endpoint.

## Limits

By default, cleartext HTTP is allowed only for a loopback IP literal such as `127.0.0.1` or `::1`. A hostname, including `localhost`, is rejected unless `--allow-cleartext-host` names that host. When that flag is used, the token must be an OpenShell credential placeholder. The placeholder form is `openshell:resolve:env:` followed by one or more characters from `A` to `Z`, digits, and `_`. That form is taken from OpenShell documentation and the maintainer's existing profile. It is an inference to be confirmed. A missing token, or any other token, is rejected before a connection is opened, and the token is not written. Redirects are not followed. `HTTP_PROXY` and the related variables are not used. A URL with user information is rejected. Request and response bodies are limited to 1 MiB. HTTPS uses the rustls webpki roots.

## Supported deployments

These are the two situations `bass` is intended to serve. The examples use placeholder values only.

### Direct use on a host machine

The real token stays in the mode `600` environment file or in the process environment. The endpoint is HTTPS. The cleartext allowance is omitted. Plain HTTP remains available only for a loopback IP literal.

```
# ~/.config/bass/environment on macOS and Linux.
# %APPDATA%\bass\environment on Windows when XDG_CONFIG_HOME is unset.
BASS_ENDPOINT=https://gateway.example/v1
BASS_TOKEN=replace-with-the-token
```

```
bass decide request.json
```

On this side the default stays strict. A real token is not sent to a named host over cleartext, because that path requires the placeholder and rejects every other token before connecting.

### Inside an OpenShell sandbox

The sandbox holds the placeholder token and the host-local relay URL. Run `bass` with the allowance set to that relay name. The real token and the real gateway name stay outside the sandbox.

```
export BASS_TOKEN="openshell:resolve:env:EXAMPLE_TOKEN"
bass decide --allow-cleartext-host host.openshell.internal \
  --endpoint "http://host.openshell.internal:8080/decision" \
  request.json
```

The following statements about OpenShell were not verified from this repository. They are inferred from OpenShell documentation and the maintainer's existing profile. The OpenShell proxy substitutes the real credential only when its policy permits the binary, host, port, path, and method. A compromised sandbox therefore yields only the call that policy already allows. Adding `bass` to the allowed binaries adds no new class of access beyond the other binaries that policy already permits. Narrow the profile to the decision path and to POST only. That is a recommendation for the profile owner. `bass` does not enforce the profile.

If the relay returns 403 with a credential endpoint mismatch, the profile does not permit this binary, host, port, path, or method. Adjust the profile. Do not replace the placeholder with a real token inside the sandbox.

## Publication

Version 0.0.1 on crates.io is the placeholder program. This source replaces that program. Publishing it requires a new version number. This tree does not publish.
