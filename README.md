# bass

`bass` posts one decision request and prints the decision.

The endpoint is the full URL you pass. No host is built into the program. The bearer token is the `BASS_TOKEN` environment variable. `bass` does not accept the token as an argument, and it does not write the token.

## Send a decision

Set the token, then pass the endpoint and a request document.

```
export BASS_TOKEN="replace-with-the-token"
bass --endpoint "https://example.invalid/decision" --request request.json
```

Omit `--request` to read the document from a pipe.

```
generator | bass --endpoint "https://example.invalid/decision"
```

`example.invalid` is a documentation host. It is not a service.

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

## Flags

`bass --help` writes the flag list. `--help` and `--version` are sole arguments.

`--endpoint URL` is required. `--request PATH` reads the document from a file. `--timeout-seconds N` stops the exchange after N seconds. The default is 60. The maximum is 600.

## Exit status

- 0 means the decision, the help text, or the version was written.
- 1 means standard output could not be written.
- 2 means the invocation was rejected.
- 3 means the request document was rejected. `bass` does not connect.
- 4 means the exchange failed.
- 5 means the response document was rejected.

Error text is a fixed sentence. It does not include the token, the response body, or the endpoint.

## Limits

Cleartext HTTP is allowed only for a loopback IP literal such as `127.0.0.1` or `::1`. A hostname, including `localhost`, is rejected. Redirects are not followed. `HTTP_PROXY` and the related variables are not used. Request and response bodies are limited to 1 MiB.

## Publication

Version 0.0.1 on crates.io is the placeholder program. This source replaces that program. Publishing it requires a new version number. This tree does not publish.
