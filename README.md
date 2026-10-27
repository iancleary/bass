# bass

`bass` posts one decision request and prints the decision.

The endpoint is a full URL you supply. No host is built into the program. The bearer token is `BASS_TOKEN`. `bass` does not accept the token as an argument, and it does not write the token or any environment-file value.

## Send a decision

Put the endpoint and the token in the environment file, then pass a request document.

```
bass --request request.json
```

You can still pass the endpoint on the command line and export the token.

```
export BASS_TOKEN="replace-with-the-token"
bass --endpoint "https://example.invalid/decision" --request request.json
```

Omit `--request` to read the document from a pipe.

```
generator | bass --endpoint "https://example.invalid/decision"
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

`--endpoint` overrides `BASS_ENDPOINT` in the process environment. The process environment overrides the file. `BASS_TOKEN` in the process environment overrides the file. An empty process value still overrides the file. When no source supplies an endpoint, `bass` exits 2 and writes `An endpoint is required.`

The file is UTF-8 and at most 64 KiB. `bass` splits it on line feeds and removes one carriage return immediately before a line feed. It ignores a line that is empty or contains only spaces and tabs. It ignores a line whose first character, after spaces and tabs, is `#`. Every other line has the form `KEY=value`. The key is the text before the first `=` and contains no spaces. The value is the rest of the line and is literal. Quotes, dollar signs, and backticks stay as characters. `bass` does not expand them and does not execute them. A `#` later in the line does not start a comment. The allowed keys are `BASS_ENDPOINT` and `BASS_TOKEN`. A repeated key is rejected. Any other key is rejected. The model remains a field of the request document.

## Flags

`bass --help` writes the flag list. `--help` and `--version` are sole arguments.

`--endpoint URL` sets the decision URL for one invocation. `--request PATH` reads the document from a file. `--environment-file PATH` selects an environment file. `--timeout-seconds N` stops the exchange after N seconds. The default is 60. The maximum is 600.

## Exit status

- 0 means the decision, the help text, or the version was written.
- 1 means standard output could not be written.
- 2 means the invocation was rejected.
- 3 means the request document was rejected. `bass` does not connect.
- 4 means the exchange failed.
- 5 means the response document was rejected.

Error text is a fixed sentence. It does not include the token, an environment-file value, the response body, or the endpoint.

## Limits

Cleartext HTTP is allowed only for a loopback IP literal such as `127.0.0.1` or `::1`. A hostname, including `localhost`, is rejected. Redirects are not followed. `HTTP_PROXY` and the related variables are not used. Request and response bodies are limited to 1 MiB.

## Publication

Version 0.0.1 on crates.io is the placeholder program. This source replaces that program. Publishing it requires a new version number. This tree does not publish.
