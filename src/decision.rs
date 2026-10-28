use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

const MIN_QUESTIONS: usize = 1;
const MAX_QUESTIONS: usize = 64;
const MIN_CHOICES: usize = 2;
const MAX_CHOICES: usize = 255;
const MIN_SCORE_LEVELS: usize = 2;
const MAX_SCORE_LEVELS: usize = 10;
const MAX_ID_LEN: usize = 100;
const MAX_LEGEND_BYTES: usize = 65536;

#[derive(Serialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
struct Model(String);

#[derive(Serialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
struct QuestionId(String);

#[derive(Serialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
struct ChoiceId(String);

#[derive(Serialize)]
#[serde(transparent)]
struct Probability(f64);

#[derive(Serialize)]
#[serde(transparent)]
struct Finite(f64);

#[derive(Serialize)]
#[serde(transparent)]
struct State(Value);

#[derive(Serialize)]
pub(super) struct Request {
    model: Model,
    state: State,
    questions: BTreeMap<QuestionId, Question>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Question {
    #[serde(rename = "noul")]
    Noul {
        instructions: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    #[serde(rename = "choice")]
    Choice {
        instructions: String,
        criteria: BTreeMap<ChoiceId, String>,
    },
    #[serde(rename = "score")]
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Serialize)]
struct NoulCriteria {
    #[serde(rename = "true")]
    when_true: String,
    #[serde(rename = "false")]
    when_false: String,
}

#[derive(Serialize)]
pub(super) struct Decision {
    model: Model,
    answers: BTreeMap<QuestionId, Answer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<Usage>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Answer {
    #[serde(rename = "noul")]
    Noul { noul: Probability },
    #[serde(rename = "choice")]
    Choice {
        choice: ChoiceId,
        probabilities: BTreeMap<String, Probability>,
        confidence: Probability,
    },
    #[serde(rename = "score")]
    Score {
        score: Finite,
        legend: BTreeMap<String, String>,
        probabilities: BTreeMap<String, Probability>,
        confidence: Probability,
    },
}

#[derive(Serialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}

pub(super) enum RequestError {
    NotJson,
    NotObject,
    UnknownField,
    MissingField,
    WrongType,
    EmptyModel,
    BadQuestionId,
    QuestionCount,
    ChoiceCount,
    ScoreLevelCount,
    UnknownQuestionType,
    NullState,
    BadCriteria,
    EmptyInstructions,
    BadChoiceId,
    BodyTooLarge,
    DuplicateKey,
    IntegerRange,
    Depth,
    TokenInDocument,
}

pub(super) enum ResponseError {
    NotJson,
    NotObject,
    UnknownField,
    MissingField,
    WrongType,
    ModelMismatch,
    MissingAnswer,
    ExtraAnswer,
    KindMismatch,
    ChoiceNotAllowed,
    ProbabilityOutOfRange,
    BadUsage,
    NotFinite,
    LegendValue,
    DuplicateKey,
    Depth,
    LegendLength,
    LegendCount,
    ProbabilityCount,
    ProbabilityKey,
    ScoreRange,
    LegendKey,
    ScoreProbabilityKey,
}

pub(super) fn parse_request(bytes: &[u8]) -> Result<Request, RequestError> {
    let value = parse_json(bytes, crate::json::IntegerPolicy::RejectOversized)?;
    let mut map = expect_object(value).ok_or(RequestError::NotObject)?;
    let model = Model::new(take_string(&mut map, "model")?)?;
    let state = State::new(take_value(&mut map, "state")?)?;
    let questions = parse_questions(take_value(&mut map, "questions")?)?;
    if !map.is_empty() {
        return Err(RequestError::UnknownField);
    }
    Ok(Request {
        model,
        state,
        questions,
    })
}

pub(super) fn parse_decision(bytes: &[u8]) -> Result<Decision, ResponseError> {
    let value = match crate::json::parse(bytes, crate::json::IntegerPolicy::WidenToFloat) {
        Ok(value) => value,
        Err(crate::json::Error::DuplicateKey) => return Err(ResponseError::DuplicateKey),
        Err(crate::json::Error::Depth) => return Err(ResponseError::Depth),
        Err(crate::json::Error::Syntax | crate::json::Error::IntegerRange) => {
            return Err(ResponseError::NotJson);
        }
    };
    let mut map = expect_object(value).ok_or(ResponseError::NotObject)?;
    let model = match take_string_response(&mut map, "model")? {
        text if text.is_empty() => return Err(ResponseError::MissingField),
        text => Model(text),
    };
    let answers = parse_answers(take_value_response(&mut map, "answers")?)?;
    let usage = match map.remove("usage") {
        None => None,
        Some(value) => Some(parse_usage(value)?),
    };
    Ok(Decision {
        model,
        answers,
        usage,
    })
}

pub(super) fn check_decision(request: &Request, decision: &Decision) -> Result<(), ResponseError> {
    if request.model != decision.model {
        return Err(ResponseError::ModelMismatch);
    }
    for id in request.questions.keys() {
        if !decision.answers.contains_key(id) {
            return Err(ResponseError::MissingAnswer);
        }
    }
    for id in decision.answers.keys() {
        if !request.questions.contains_key(id) {
            return Err(ResponseError::ExtraAnswer);
        }
    }
    for (id, question) in &request.questions {
        let answer = &decision.answers[id];
        if !same_kind(question, answer) {
            return Err(ResponseError::KindMismatch);
        }
        match (question, answer) {
            (
                Question::Choice { criteria, .. },
                Answer::Choice {
                    choice,
                    probabilities,
                    ..
                },
            ) => {
                if !criteria.contains_key(choice) {
                    return Err(ResponseError::ChoiceNotAllowed);
                }
                if probabilities.len() > criteria.len() {
                    return Err(ResponseError::ProbabilityCount);
                }
                for key in probabilities.keys() {
                    let allowed = criteria.keys().any(|candidate| candidate.0 == *key);
                    if !allowed {
                        return Err(ResponseError::ProbabilityKey);
                    }
                }
            }
            (
                Question::Score { criteria, .. },
                Answer::Score {
                    score,
                    legend,
                    probabilities,
                    ..
                },
            ) => {
                let levels = criteria.len();
                if !score_in_anchor_range(score.0, levels) {
                    return Err(ResponseError::ScoreRange);
                }
                if legend.len() > levels {
                    return Err(ResponseError::LegendCount);
                }
                for (key, value) in legend {
                    if key.len() > MAX_LEGEND_BYTES || value.len() > MAX_LEGEND_BYTES {
                        return Err(ResponseError::LegendLength);
                    }
                    if !is_anchor_index(key, levels) {
                        return Err(ResponseError::LegendKey);
                    }
                }
                if probabilities.len() > levels {
                    return Err(ResponseError::ProbabilityCount);
                }
                for key in probabilities.keys() {
                    if !is_anchor_index(key, levels) {
                        return Err(ResponseError::ScoreProbabilityKey);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_json(bytes: &[u8], integers: crate::json::IntegerPolicy) -> Result<Value, RequestError> {
    match crate::json::parse(bytes, integers) {
        Ok(value) => Ok(value),
        Err(crate::json::Error::Syntax) => Err(RequestError::NotJson),
        Err(crate::json::Error::DuplicateKey) => Err(RequestError::DuplicateKey),
        Err(crate::json::Error::IntegerRange) => Err(RequestError::IntegerRange),
        Err(crate::json::Error::Depth) => Err(RequestError::Depth),
    }
}

pub(super) fn render_request(request: &Request) -> Vec<u8> {
    render_json(request)
}

pub(super) fn render_decision(decision: &Decision) -> Vec<u8> {
    render_json(decision)
}

const RESPONSE_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

const TOKEN_COUNT_DESCRIPTION: &str = "Unsigned integer from 0 through 18446744073709551615. bass accepts a JSON number only when it parses as an unsigned integer. A whole number written with a decimal point or an exponent, such as 1.0 or 1e2, is rejected by bass. JSON Schema integer cannot express that spelling.";

pub(super) fn response_schema(request: &Request) -> Vec<u8> {
    serde_json::to_vec_pretty(&response_schema_value(request)).expect("response schema serializes")
}

fn response_schema_value(request: &Request) -> Value {
    let mut answer_properties = Map::new();
    let mut required_answers = Vec::new();
    for (id, question) in &request.questions {
        required_answers.push(Value::String(id.0.clone()));
        answer_properties.insert(id.0.clone(), answer_schema(question));
    }
    json_object([
        (
            "$schema",
            Value::String(RESPONSE_SCHEMA_DIALECT.to_string()),
        ),
        ("additionalProperties", Value::Bool(true)),
        (
            "description",
            Value::String(
                "Response bass accepts for this request. Unknown fields at the top level are accepted. bass drops them from the printed decision. This schema checks the response document and cannot require that drop. bass rejects a repeated object key. This schema cannot see a repeated key, because a JSON parser keeps one value, so it is looser there. bass rejects nesting deeper than 512. Draft 2020-12 has no depth keyword. A document this schema accepts is shallower than 512, so depth does not separate the two checks on an accepted response. Where a field is looser or stricter than bass, that field's description says so.".to_string(),
            ),
        ),
        (
            "properties",
            json_object([
                (
                    "answers",
                    json_object([
                        ("additionalProperties", Value::Bool(false)),
                        (
                            "description",
                            Value::String(
                                "One answer for each question id, and no other id.".to_string(),
                            ),
                        ),
                        ("properties", Value::Object(answer_properties)),
                        ("required", Value::Array(required_answers)),
                        ("type", json_string("object")),
                    ]),
                ),
                (
                    "model",
                    json_object([
                        ("const", Value::String(request.model.0.clone())),
                        (
                            "description",
                            Value::String("Equal to the request model.".to_string()),
                        ),
                        ("type", json_string("string")),
                    ]),
                ),
                ("usage", usage_schema()),
            ]),
        ),
        (
            "required",
            Value::Array(vec![json_string("answers"), json_string("model")]),
        ),
        ("type", json_string("object")),
    ])
}

fn answer_schema(question: &Question) -> Value {
    match question {
        Question::Noul { .. } => noul_answer_schema(),
        Question::Choice { criteria, .. } => choice_answer_schema(criteria),
        Question::Score { criteria, .. } => score_answer_schema(criteria.len()),
    }
}

fn noul_answer_schema() -> Value {
    json_object([
        ("additionalProperties", Value::Bool(true)),
        (
            "description",
            Value::String(
                "Yes-or-no probability from 0 to 1, inclusive. Unknown fields are accepted. bass drops them from the printed decision. This schema cannot require that drop.".to_string(),
            ),
        ),
        (
            "properties",
            json_object([
                ("noul", probability_schema()),
                ("type", const_string("noul")),
            ]),
        ),
        (
            "required",
            Value::Array(vec![json_string("noul"), json_string("type")]),
        ),
        ("type", json_string("object")),
    ])
}

fn choice_answer_schema(criteria: &BTreeMap<ChoiceId, String>) -> Value {
    let mut probability_properties = Map::new();
    let mut option_ids = Vec::new();
    for id in criteria.keys() {
        option_ids.push(Value::String(id.0.clone()));
        probability_properties.insert(id.0.clone(), probability_schema());
    }
    json_object([
        ("additionalProperties", Value::Bool(true)),
        (
            "description",
            Value::String(
                "choice is one option id from this question. probabilities keys are a subset of those option ids, and the empty object is allowed. confidence is a probability from 0 to 1. Unknown fields are accepted. bass drops them from the printed decision. This schema cannot require that drop. The probabilities description states where that object matches bass.".to_string(),
            ),
        ),
        (
            "properties",
            json_object([
                (
                    "choice",
                    json_object([
                        ("enum", Value::Array(option_ids)),
                        ("type", json_string("string")),
                    ]),
                ),
                ("confidence", probability_schema()),
                (
                    "probabilities",
                    json_object([
                        ("additionalProperties", Value::Bool(false)),
                        (
                            "description",
                            Value::String(
                                "Keys are a subset of the option ids. additionalProperties is false and maxProperties equals the option count. A key outside the option ids is rejected, and the entry count cannot exceed the option count. This matches bass. The schema is neither looser nor stricter on this object.".to_string(),
                            ),
                        ),
                        (
                            "maxProperties",
                            Value::from(u64::try_from(criteria.len()).expect("option count fits")),
                        ),
                        ("properties", Value::Object(probability_properties)),
                        ("type", json_string("object")),
                    ]),
                ),
                ("type", const_string("choice")),
            ]),
        ),
        (
            "required",
            Value::Array(vec![
                json_string("choice"),
                json_string("confidence"),
                json_string("probabilities"),
                json_string("type"),
            ]),
        ),
        ("type", json_string("object")),
    ])
}

fn score_answer_schema(level_count: usize) -> Value {
    let entry_limit = Value::from(u64::try_from(level_count).expect("level count fits"));
    let last_index = Value::from(u64::try_from(level_count - 1).expect("level count"));
    let legend_limit = Value::from(MAX_LEGEND_BYTES as u64);
    let anchors = Value::Array(
        (0..level_count)
            .map(|index| Value::String(index.to_string()))
            .collect(),
    );
    json_object([
        ("additionalProperties", Value::Bool(true)),
        (
            "description",
            Value::String(
                "score is a finite number from 0 through one less than the number of levels. minimum and maximum match that range. legend and probabilities keys are the index strings for those levels. Unknown fields are accepted. bass drops them from the printed decision. This schema cannot require that drop. The legend description states the byte-versus-character gap on legend values.".to_string(),
            ),
        ),
        (
            "properties",
            json_object([
                ("confidence", probability_schema()),
                (
                    "legend",
                    json_object([
                        (
                            "additionalProperties",
                            json_object([
                                ("maxLength", legend_limit),
                                ("type", json_string("string")),
                            ]),
                        ),
                        (
                            "description",
                            Value::String(format!(
                                "At most as many entries as the rubric has levels. Keys are the index strings from 0 through the last index, with no leading zero. propertyNames enum lists those strings, so the schema matches bass on keys. bass also limits each key and each value to {MAX_LEGEND_BYTES} bytes. Every allowed key is a short ASCII string, so the byte limit on keys does not separate the checks. maxLength on values counts Unicode characters, not UTF-8 bytes. The schema is looser for a value character that encodes as more than one byte. It is not stricter: every string of at most {MAX_LEGEND_BYTES} bytes has at most {MAX_LEGEND_BYTES} characters."
                            )),
                        ),
                        ("maxProperties", entry_limit.clone()),
                        (
                            "propertyNames",
                            json_object([("enum", anchors.clone())]),
                        ),
                        ("type", json_string("object")),
                    ]),
                ),
                (
                    "probabilities",
                    json_object([
                        ("additionalProperties", probability_schema()),
                        (
                            "description",
                            Value::String(
                                "At most as many entries as the rubric has levels. Keys are the same index strings as legend. propertyNames enum lists them, so the schema matches bass on keys. Values are probabilities from 0 to 1. The schema is neither looser nor stricter on this object.".to_string(),
                            ),
                        ),
                        ("maxProperties", entry_limit),
                        (
                            "propertyNames",
                            json_object([("enum", anchors)]),
                        ),
                        ("type", json_string("object")),
                    ]),
                ),
                (
                    "score",
                    json_object([
                        (
                            "description",
                            Value::String(
                                "Finite anchor index from 0 through one less than the number of levels. minimum and maximum match bass. A JSON number is finite, so this schema is neither looser nor stricter on finiteness.".to_string(),
                            ),
                        ),
                        ("maximum", last_index),
                        ("minimum", Value::from(0)),
                        ("type", json_string("number")),
                    ]),
                ),
                ("type", const_string("score")),
            ]),
        ),
        (
            "required",
            Value::Array(vec![
                json_string("confidence"),
                json_string("legend"),
                json_string("probabilities"),
                json_string("score"),
                json_string("type"),
            ]),
        ),
        ("type", json_string("object")),
    ])
}

fn usage_schema() -> Value {
    json_object([
        ("additionalProperties", Value::Bool(false)),
        (
            "description",
            Value::String(
                "Optional token counts. Unknown fields inside usage are rejected. This matches bass. usage itself may be omitted.".to_string(),
            ),
        ),
        (
            "properties",
            json_object([
                ("input_tokens", token_count_schema()),
                ("output_tokens", token_count_schema()),
            ]),
        ),
        (
            "required",
            Value::Array(vec![
                json_string("input_tokens"),
                json_string("output_tokens"),
            ]),
        ),
        ("type", json_string("object")),
    ])
}

fn probability_schema() -> Value {
    json_object([
        ("maximum", Value::from(1)),
        ("minimum", Value::from(0)),
        ("type", json_string("number")),
    ])
}

fn token_count_schema() -> Value {
    json_object([
        (
            "description",
            Value::String(TOKEN_COUNT_DESCRIPTION.to_string()),
        ),
        ("maximum", Value::from(u64::MAX)),
        ("minimum", Value::from(0)),
        ("type", json_string("integer")),
    ])
}

fn const_string(value: &str) -> Value {
    json_object([
        ("const", Value::String(value.to_string())),
        ("type", json_string("string")),
    ])
}

fn json_string(value: &str) -> Value {
    Value::String(value.to_string())
}

fn json_object<const N: usize>(entries: [(&str, Value); N]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

fn render_json(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("decision document serializes")
}

fn parse_questions(value: Value) -> Result<BTreeMap<QuestionId, Question>, RequestError> {
    let map = expect_object(value).ok_or(RequestError::WrongType)?;
    if !(MIN_QUESTIONS..=MAX_QUESTIONS).contains(&map.len()) {
        return Err(RequestError::QuestionCount);
    }
    let mut questions = BTreeMap::new();
    for (key, value) in map {
        let id = QuestionId::new(key).ok_or(RequestError::BadQuestionId)?;
        questions.insert(id, parse_question(value)?);
    }
    Ok(questions)
}

fn parse_question(value: Value) -> Result<Question, RequestError> {
    let mut map = expect_object(value).ok_or(RequestError::WrongType)?;
    let type_name = take_string(&mut map, "type")?;
    let instructions = take_string(&mut map, "instructions")?;
    if instructions.is_empty() {
        return Err(RequestError::EmptyInstructions);
    }
    match type_name.as_str() {
        "noul" => {
            let criteria = match map.remove("criteria") {
                None => None,
                Some(value) => Some(parse_noul_criteria(value)?),
            };
            if !map.is_empty() {
                return Err(RequestError::UnknownField);
            }
            Ok(Question::Noul {
                instructions,
                criteria,
            })
        }
        "choice" => {
            let criteria = parse_choice_criteria(take_value(&mut map, "criteria")?)?;
            if !map.is_empty() {
                return Err(RequestError::UnknownField);
            }
            Ok(Question::Choice {
                instructions,
                criteria,
            })
        }
        "score" => {
            let criteria = parse_score_levels(take_value(&mut map, "criteria")?)?;
            if !map.is_empty() {
                return Err(RequestError::UnknownField);
            }
            Ok(Question::Score {
                instructions,
                criteria,
            })
        }
        _ => Err(RequestError::UnknownQuestionType),
    }
}

fn parse_noul_criteria(value: Value) -> Result<NoulCriteria, RequestError> {
    let mut map = expect_object(value).ok_or(RequestError::BadCriteria)?;
    let when_true = take_string(&mut map, "true")?;
    let when_false = take_string(&mut map, "false")?;
    if !map.is_empty() {
        return Err(RequestError::UnknownField);
    }
    if when_true.is_empty() || when_false.is_empty() {
        return Err(RequestError::BadCriteria);
    }
    Ok(NoulCriteria {
        when_true,
        when_false,
    })
}

fn parse_choice_criteria(value: Value) -> Result<BTreeMap<ChoiceId, String>, RequestError> {
    let map = expect_object(value).ok_or(RequestError::BadCriteria)?;
    if !(MIN_CHOICES..=MAX_CHOICES).contains(&map.len()) {
        return Err(RequestError::ChoiceCount);
    }
    let mut criteria = BTreeMap::new();
    for (key, value) in map {
        let id = ChoiceId::new(key).ok_or(RequestError::BadChoiceId)?;
        let description = match value {
            Value::String(text) if !text.is_empty() => text,
            _ => return Err(RequestError::BadCriteria),
        };
        criteria.insert(id, description);
    }
    Ok(criteria)
}

fn parse_score_levels(value: Value) -> Result<Vec<String>, RequestError> {
    let levels = match value {
        Value::Array(levels) => levels,
        _ => return Err(RequestError::BadCriteria),
    };
    if !(MIN_SCORE_LEVELS..=MAX_SCORE_LEVELS).contains(&levels.len()) {
        return Err(RequestError::ScoreLevelCount);
    }
    levels
        .into_iter()
        .map(|level| match level {
            Value::String(text) if !text.is_empty() => Ok(text),
            _ => Err(RequestError::BadCriteria),
        })
        .collect()
}

fn parse_answers(value: Value) -> Result<BTreeMap<QuestionId, Answer>, ResponseError> {
    let map = expect_object(value).ok_or(ResponseError::WrongType)?;
    let mut answers = BTreeMap::new();
    for (key, value) in map {
        let id = QuestionId::new(key).ok_or(ResponseError::ExtraAnswer)?;
        answers.insert(id, parse_answer(value)?);
    }
    Ok(answers)
}

fn parse_answer(value: Value) -> Result<Answer, ResponseError> {
    let mut map = expect_object(value).ok_or(ResponseError::WrongType)?;
    let type_name = take_string_response(&mut map, "type")?;
    match type_name.as_str() {
        "noul" => {
            let noul = expect_probability(&take_value_response(&mut map, "noul")?)?;
            Ok(Answer::Noul { noul })
        }
        "choice" => {
            let choice = ChoiceId::new(take_string_response(&mut map, "choice")?)
                .ok_or(ResponseError::ChoiceNotAllowed)?;
            let probabilities =
                parse_probability_map(take_value_response(&mut map, "probabilities")?)?;
            let confidence = expect_probability(&take_value_response(&mut map, "confidence")?)?;
            Ok(Answer::Choice {
                choice,
                probabilities,
                confidence,
            })
        }
        "score" => {
            let score = Finite(expect_finite(&take_value_response(&mut map, "score")?)?);
            let legend = parse_legend(take_value_response(&mut map, "legend")?)?;
            let probabilities =
                parse_probability_map(take_value_response(&mut map, "probabilities")?)?;
            let confidence = expect_probability(&take_value_response(&mut map, "confidence")?)?;
            Ok(Answer::Score {
                score,
                legend,
                probabilities,
                confidence,
            })
        }
        _ => Err(ResponseError::WrongType),
    }
}

fn parse_probability_map(value: Value) -> Result<BTreeMap<String, Probability>, ResponseError> {
    let map = expect_object(value).ok_or(ResponseError::WrongType)?;
    let mut probabilities = BTreeMap::new();
    for (key, value) in map {
        if key.is_empty() {
            return Err(ResponseError::WrongType);
        }
        probabilities.insert(key, expect_probability(&value)?);
    }
    Ok(probabilities)
}

fn parse_legend(value: Value) -> Result<BTreeMap<String, String>, ResponseError> {
    let map = expect_object(value).ok_or(ResponseError::WrongType)?;
    let mut legend = BTreeMap::new();
    for (key, value) in map {
        match value {
            Value::String(text) => {
                legend.insert(key, text);
            }
            _ => return Err(ResponseError::LegendValue),
        }
    }
    Ok(legend)
}

fn parse_usage(value: Value) -> Result<Usage, ResponseError> {
    let mut map = expect_object(value).ok_or(ResponseError::BadUsage)?;
    let input_tokens = take_u64(&mut map, "input_tokens")?;
    let output_tokens = take_u64(&mut map, "output_tokens")?;
    if !map.is_empty() {
        return Err(ResponseError::UnknownField);
    }
    Ok(Usage {
        input_tokens,
        output_tokens,
    })
}

fn score_in_anchor_range(score: f64, levels: usize) -> bool {
    if levels == 0 {
        return false;
    }
    let last = (levels - 1) as f64;
    (0.0..=last).contains(&score)
}

fn is_anchor_index(key: &str, levels: usize) -> bool {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if key.len() > 1 && key.starts_with('0') {
        return false;
    }
    match key.parse::<usize>() {
        Ok(index) => index < levels,
        Err(_) => false,
    }
}

fn same_kind(question: &Question, answer: &Answer) -> bool {
    matches!(
        (question, answer),
        (Question::Noul { .. }, Answer::Noul { .. })
            | (Question::Choice { .. }, Answer::Choice { .. })
            | (Question::Score { .. }, Answer::Score { .. })
    )
}

fn expect_object(value: Value) -> Option<Map<String, Value>> {
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

fn take_value(map: &mut Map<String, Value>, key: &str) -> Result<Value, RequestError> {
    map.remove(key).ok_or(RequestError::MissingField)
}

fn take_value_response(map: &mut Map<String, Value>, key: &str) -> Result<Value, ResponseError> {
    map.remove(key).ok_or(ResponseError::MissingField)
}

fn take_string(map: &mut Map<String, Value>, key: &str) -> Result<String, RequestError> {
    match take_value(map, key)? {
        Value::String(text) => Ok(text),
        _ => Err(RequestError::WrongType),
    }
}

fn take_string_response(map: &mut Map<String, Value>, key: &str) -> Result<String, ResponseError> {
    match take_value_response(map, key)? {
        Value::String(text) => Ok(text),
        _ => Err(ResponseError::WrongType),
    }
}

fn take_u64(map: &mut Map<String, Value>, key: &str) -> Result<u64, ResponseError> {
    match take_value_response(map, key)? {
        Value::Number(number) => number.as_u64().ok_or(ResponseError::BadUsage),
        _ => Err(ResponseError::BadUsage),
    }
}

fn expect_finite(value: &Value) -> Result<f64, ResponseError> {
    match value {
        Value::Number(number) => match number.as_f64() {
            Some(number) if number.is_finite() => Ok(number),
            _ => Err(ResponseError::NotFinite),
        },
        _ => Err(ResponseError::WrongType),
    }
}

fn expect_probability(value: &Value) -> Result<Probability, ResponseError> {
    let number = expect_finite(value)?;
    if (0.0..=1.0).contains(&number) {
        Ok(Probability(number))
    } else {
        Err(ResponseError::ProbabilityOutOfRange)
    }
}

impl Model {
    fn new(raw: String) -> Result<Self, RequestError> {
        if raw.is_empty() {
            Err(RequestError::EmptyModel)
        } else {
            Ok(Model(raw))
        }
    }
}

impl QuestionId {
    fn new(raw: String) -> Option<Self> {
        let allowed = (1..=MAX_ID_LEN).contains(&raw.len())
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
        if allowed {
            Some(QuestionId(raw))
        } else {
            None
        }
    }
}

impl ChoiceId {
    fn new(raw: String) -> Option<Self> {
        let allowed = (1..=MAX_ID_LEN).contains(&raw.len())
            && raw.bytes().all(|byte| (0x20..0x7f).contains(&byte));
        if allowed {
            Some(ChoiceId(raw))
        } else {
            None
        }
    }
}

impl State {
    fn new(raw: Value) -> Result<Self, RequestError> {
        if raw.is_null() {
            Err(RequestError::NullState)
        } else {
            Ok(State(raw))
        }
    }
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            RequestError::NotJson => "The request is not valid JSON.",
            RequestError::NotObject => "The request is not a JSON object.",
            RequestError::UnknownField => "The request has an unknown field.",
            RequestError::MissingField => "The request is missing a field.",
            RequestError::WrongType => "The request has a field of the wrong type.",
            RequestError::EmptyModel => "The model is empty.",
            RequestError::BadQuestionId => "A question id is outside the allowed form.",
            RequestError::QuestionCount => "The question count is outside 1 to 64.",
            RequestError::ChoiceCount => "A choice question is outside 2 to 255 options.",
            RequestError::ScoreLevelCount => "A score question is outside 2 to 10 levels.",
            RequestError::UnknownQuestionType => "A question type is unknown.",
            RequestError::NullState => "The state is null.",
            RequestError::BadCriteria => "A criteria value has the wrong shape.",
            RequestError::EmptyInstructions => "The instructions are empty.",
            RequestError::BadChoiceId => "A choice id is outside the allowed form.",
            RequestError::BodyTooLarge => "The request is too large.",
            RequestError::DuplicateKey => "The request repeats an object key.",
            RequestError::IntegerRange => "An integer is outside the 64-bit range.",
            RequestError::Depth => "The request nests too deeply.",
            RequestError::TokenInDocument => "The request contains the token.",
        })
    }
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            ResponseError::NotJson => "The response is not valid JSON.",
            ResponseError::NotObject => "The response is not a JSON object.",
            ResponseError::UnknownField => "The response has an unknown field.",
            ResponseError::MissingField => "The response is missing a field.",
            ResponseError::WrongType => "The response has a field of the wrong type.",
            ResponseError::ModelMismatch => "The response model differs from the request.",
            ResponseError::MissingAnswer => "A question has no answer.",
            ResponseError::ExtraAnswer => "An answer id is not in the request.",
            ResponseError::KindMismatch => "An answer kind differs from its question.",
            ResponseError::ChoiceNotAllowed => "The choice is outside the question criteria.",
            ResponseError::ProbabilityOutOfRange => "A probability is outside 0 to 1.",
            ResponseError::BadUsage => "The usage counts are missing or not integers.",
            ResponseError::NotFinite => "A response number is not finite.",
            ResponseError::LegendValue => "A legend value is not a string.",
            ResponseError::DuplicateKey => "The response repeats an object key.",
            ResponseError::Depth => "The response nests too deeply.",
            ResponseError::LegendLength => "A legend string exceeds 65536 bytes.",
            ResponseError::LegendCount => "A legend has more entries than the question allows.",
            ResponseError::ProbabilityCount => {
                "A probability map has more entries than the question allows."
            }
            ResponseError::ProbabilityKey => "A probability key is outside the question criteria.",
            ResponseError::ScoreRange => "The score is outside the anchor index range.",
            ResponseError::LegendKey => "A legend key is outside the anchor index range.",
            ResponseError::ScoreProbabilityKey => {
                "A score probability key is outside the anchor index range."
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_message(document: &str) -> String {
        match parse_request(document.as_bytes()) {
            Ok(_) => panic!("expected a request error"),
            Err(error) => error.to_string(),
        }
    }

    fn must_request(document: &[u8]) -> Request {
        match parse_request(document) {
            Ok(request) => request,
            Err(error) => panic!("{error}"),
        }
    }

    fn must_decision(document: &[u8]) -> Decision {
        match parse_decision(document) {
            Ok(decision) => decision,
            Err(error) => panic!("{error}"),
        }
    }

    fn check_message(request: &Request, decision: &Decision) -> String {
        match check_decision(request, decision) {
            Ok(()) => String::new(),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn rejects_question_ids_and_counts() {
        assert_eq!(
            request_message(
                r#"{"model":"demo","state":"text","questions":{"bad id":{"type":"noul","instructions":"Yes?"}}}"#
            ),
            "A question id is outside the allowed form."
        );
        assert_eq!(
            request_message(r#"{"model":"demo","state":"text","questions":{}}"#),
            "The question count is outside 1 to 64."
        );
    }

    #[test]
    fn rejects_choice_and_score_bounds() {
        assert_eq!(
            request_message(
                r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"only":"one"}}}}"#
            ),
            "A choice question is outside 2 to 255 options."
        );
        assert_eq!(
            request_message(
                r#"{"model":"demo","state":"text","questions":{"rank":{"type":"score","instructions":"How high?","criteria":["low"]}}}"#
            ),
            "A score question is outside 2 to 10 levels."
        );
    }

    #[test]
    fn renders_choice_and_score_documents() {
        let choice = must_request(
            br#"{"state":{"ticket":1},"model":"demo","questions":{"team":{"criteria":{"technical":"Outages","billing":"Payments"},"instructions":"Which team?","type":"choice"}}}"#,
        );
        assert_eq!(
            render_request(&choice),
            br#"{"model":"demo","state":{"ticket":1},"questions":{"team":{"type":"choice","instructions":"Which team?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#
        );

        let score = must_request(
            br#"{"model":"demo","state":["row"],"questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#,
        );
        assert_eq!(
            render_request(&score),
            br#"{"model":"demo","state":["row"],"questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#
        );
    }

    #[test]
    fn checks_the_response_against_the_request() {
        let request = must_request(
            br#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        );
        let mismatch =
            must_decision(br#"{"model":"other","answers":{"eligible":{"type":"noul","noul":1}}}"#);
        assert_eq!(
            check_message(&request, &mismatch),
            "The response model differs from the request."
        );

        let kind = must_decision(
            br#"{"model":"demo","answers":{"eligible":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1}}}"#,
        );
        assert_eq!(
            check_message(&request, &kind),
            "An answer kind differs from its question."
        );

        let decision = must_decision(
            br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#,
        );
        assert_eq!(check_message(&request, &decision), "");
        assert_eq!(
            render_decision(&decision),
            br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#
        );
    }

    #[test]
    fn rejects_a_probability_above_one() {
        let message = match parse_decision(
            br#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1.5}}}"#,
        ) {
            Ok(_) => panic!("expected a probability error"),
            Err(error) => error.to_string(),
        };
        assert_eq!(message, "A probability is outside 0 to 1.");
    }

    #[test]
    fn choice_probability_keys_are_a_subset_of_the_option_ids() {
        let request = must_request(
            br#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#,
        );
        let subset = must_decision(
            br#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1}}}"#,
        );
        assert_eq!(check_message(&request, &subset), "");
        let empty = must_decision(
            br#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{},"confidence":1}}}"#,
        );
        assert_eq!(check_message(&request, &empty), "");
        let outside = must_decision(
            br#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"other":1},"confidence":1}}}"#,
        );
        assert_eq!(
            check_message(&request, &outside),
            "A probability key is outside the question criteria."
        );
        let too_many = must_decision(
            br#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0,"other":0},"confidence":1}}}"#,
        );
        assert_eq!(
            check_message(&request, &too_many),
            "A probability map has more entries than the question allows."
        );
    }

    #[test]
    fn response_schemas_match_golden_files() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden/response_schema");
        for name in ["noul", "choice", "score", "mixed"] {
            let document = std::fs::read(root.join(format!("{name}.request.json")))
                .unwrap_or_else(|_| panic!("read {name} request"));
            let request = must_request(&document);
            let mut actual = response_schema(&request);
            assert!(
                actual.ends_with(b"}"),
                "{name} schema does not end at the object"
            );
            actual.push(b'\n');
            let schema_path = root.join(format!("{name}.schema.json"));
            let expected =
                std::fs::read(&schema_path).unwrap_or_else(|_| panic!("read {name} schema"));
            assert_eq!(actual, expected, "{name} response schema");
            let parsed: Value = serde_json::from_slice(&actual).expect("golden schema is JSON");
            assert_eq!(
                parsed["$schema"],
                "https://json-schema.org/draft/2020-12/schema"
            );
            let mut again = serde_json::to_vec_pretty(&parsed).expect("schema reserializes");
            again.push(b'\n');
            assert_eq!(actual, again, "{name} keys are not in sorted order");
            assert_eq!(response_schema(&request), &actual[..actual.len() - 1]);
        }
    }

    #[test]
    fn choice_schema_lists_option_ids_in_order() {
        let request = must_request(
            br#"{"model":"demo","state":"text","questions":{"pick":{"type":"choice","instructions":"Which?","criteria":{"b b":"second","a/a":"first","say\"hi":"quoted"}}}}"#,
        );
        let value: Value = serde_json::from_slice(&response_schema(&request)).expect("schema");
        let choice = &value["properties"]["answers"]["properties"]["pick"]["properties"]["choice"];
        assert_eq!(choice["enum"], serde_json::json!(["a/a", "b b", "say\"hi"]));
        let probabilities =
            &value["properties"]["answers"]["properties"]["pick"]["properties"]["probabilities"];
        let keys: Vec<&str> = probabilities["properties"]
            .as_object()
            .expect("probability properties")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["a/a", "b b", "say\"hi"]);
        assert!(probabilities.get("required").is_none());
        assert_eq!(probabilities["maxProperties"], 3);
    }

    fn assert_response_agreement(request_document: &str, cases: &[(&str, &str, bool)]) {
        let request = must_request(request_document.as_bytes());
        let schema: Value =
            serde_json::from_slice(&response_schema(&request)).expect("schema is JSON");
        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .expect("response schema compiles");
        for (name, document, accepted) in cases {
            let bass_ok = match parse_decision(document.as_bytes()) {
                Ok(decision) => check_decision(&request, &decision).is_ok(),
                Err(_) => false,
            };
            assert_eq!(bass_ok, *accepted, "{name} rejected by bass unexpectedly");
            let schema_ok = match serde_json::from_str::<Value>(document) {
                Ok(value) => validator.is_valid(&value),
                Err(_) => false,
            };
            if schema_ok != bass_ok {
                let detail = match serde_json::from_str::<Value>(document) {
                    Ok(value) => match validator.validate(&value) {
                        Ok(()) => String::new(),
                        Err(errors) => errors
                            .map(|error| error.to_string())
                            .collect::<Vec<_>>()
                            .join("; "),
                    },
                    Err(error) => error.to_string(),
                };
                panic!("{name}: schema accepted {schema_ok}, bass accepted {bass_ok}: {detail}");
            }
        }
    }

    #[test]
    fn noul_response_schema_agrees_with_validation() {
        let usage_max = format!(
            r#"{{"model":"demo","answers":{{"eligible":{{"type":"noul","noul":1}}}},"usage":{{"input_tokens":{},"output_tokens":0}}}}"#,
            u64::MAX
        );
        let mut cases = vec![
            (
                "zero",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0}}}"#,
                true,
            ),
            (
                "one",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}}}"#,
                true,
            ),
            (
                "fraction",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0.25}}}"#,
                true,
            ),
            (
                "one with a decimal",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1.0}}}"#,
                true,
            ),
            (
                "usage zero",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0}},"usage":{"input_tokens":0,"output_tokens":0}}"#,
                true,
            ),
            ("usage maximum", usage_max.as_str(), true),
            (
                "wrong model",
                r#"{"model":"other","answers":{"eligible":{"type":"noul","noul":1}}}"#,
                false,
            ),
            (
                "empty model",
                r#"{"model":"","answers":{"eligible":{"type":"noul","noul":1}}}"#,
                false,
            ),
            ("missing answer", r#"{"model":"demo","answers":{}}"#, false),
            (
                "extra answer",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1},"other":{"type":"noul","noul":1}}}"#,
                false,
            ),
            (
                "wrong type",
                r#"{"model":"demo","answers":{"eligible":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1}}}"#,
                false,
            ),
            (
                "above one",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1.1}}}"#,
                false,
            ),
            (
                "below zero",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":-0.1}}}"#,
                false,
            ),
            (
                "noul string",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":"1"}}}"#,
                false,
            ),
            (
                "extra answer field",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1,"note":"x"}}}"#,
                true,
            ),
            (
                "negative usage",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":-1,"output_tokens":0}}"#,
                false,
            ),
            (
                "fractional usage",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":1.5,"output_tokens":0}}"#,
                false,
            ),
            (
                "usage missing a field",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":1}}"#,
                false,
            ),
            (
                "usage extra field",
                r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#,
                false,
            ),
            ("not json", "not-json", false),
            ("not an object", "[]", false),
        ];
        let above = format!(
            r#"{{"model":"demo","answers":{{"eligible":{{"type":"noul","noul":1}}}},"usage":{{"input_tokens":{},"output_tokens":0}}}}"#,
            (u64::MAX as u128) + 1
        );
        cases.push(("usage above u64", above.as_str(), false));
        let deep = format!("{}{}", "[".repeat(600), "]".repeat(600));
        cases.push(("nesting deeper than 512", deep.as_str(), false));
        cases.push((
            "unknown field",
            r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"note":1}"#,
            true,
        ));
        assert_response_agreement(
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
            &cases,
        );
    }

    #[test]
    fn choice_response_schema_agrees_with_validation() {
        assert_response_agreement(
            r#"{"model":"demo","state":"text","questions":{"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#,
            &[
                (
                    "both bounds",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":0,"technical":1},"confidence":1}}}"#,
                    true,
                ),
                (
                    "reordered keys",
                    r#"{"model":"demo","answers":{"team":{"confidence":0,"probabilities":{"technical":0.25,"billing":0.25},"choice":"technical","type":"choice"}}}"#,
                    true,
                ),
                (
                    "probabilities need not sum to one",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":1},"confidence":1.0}}}"#,
                    true,
                ),
                (
                    "choice outside options",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"other","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "empty choice",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "subset of option ids",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1}}}"#,
                    true,
                ),
                (
                    "extra probability key",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0,"other":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "probability key outside the options",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"other":1},"confidence":1}}}"#,
                    false,
                ),
                (
                    "empty probabilities",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{},"confidence":1}}}"#,
                    true,
                ),
                (
                    "probability out of range",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1.5,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "confidence out of range",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":2}}}"#,
                    false,
                ),
                (
                    "wrong type",
                    r#"{"model":"demo","answers":{"team":{"type":"noul","noul":1}}}"#,
                    false,
                ),
                (
                    "unknown answer field",
                    r#"{"model":"demo","answers":{"team":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1,"note":"x"}}}"#,
                    true,
                ),
            ],
        );
    }

    #[test]
    fn score_response_schema_agrees_with_validation() {
        assert_response_agreement(
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#,
            &[
                (
                    "empty legend and probabilities",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":0,"legend":{},"probabilities":{},"confidence":0}}}"#,
                    true,
                ),
                (
                    "negative score",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":-10,"legend":{},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "empty legend key",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":0,"legend":{"":"note"},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "large finite score",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1e308,"legend":{},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "score above the last index",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":2,"legend":{},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "fractional score inside the anchor range",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":0.5,"legend":{"0":"low"},"probabilities":{"1":0},"confidence":0.5}}}"#,
                    true,
                ),
                (
                    "legend and probabilities at the rubric length",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"minor","1":"major"},"probabilities":{"0":0.25,"1":0.75},"confidence":0.5}}}"#,
                    true,
                ),
                (
                    "leading zero is not an index",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"01":"note"},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "probability key outside the index strings",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{"not-a-level":0.5},"confidence":1}}}"#,
                    false,
                ),
                (
                    "unknown answer field",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1,"note":"x"}}}"#,
                    true,
                ),
                (
                    "too many legend entries",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":"a","1":"b","2":"c"},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "too many probability entries",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{"0":0.2,"1":0.3,"2":0.5},"confidence":1}}}"#,
                    false,
                ),
                (
                    "score string",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":"high","legend":{},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "legend value number",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{"0":1},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "empty probability key",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{"":0.2},"confidence":1}}}"#,
                    false,
                ),
                (
                    "probability out of range",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{"0":1.2},"confidence":1}}}"#,
                    false,
                ),
                (
                    "confidence below zero",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":-0.01}}}"#,
                    false,
                ),
                (
                    "missing score",
                    r#"{"model":"demo","answers":{"severity":{"type":"score","legend":{},"probabilities":{},"confidence":1}}}"#,
                    false,
                ),
                (
                    "wrong type",
                    r#"{"model":"demo","answers":{"severity":{"type":"noul","noul":1}}}"#,
                    false,
                ),
            ],
        );
    }

    #[test]
    fn mixed_response_schema_agrees_with_validation() {
        assert_response_agreement(
            r#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"},"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]},"team":{"type":"choice","instructions":"Which?","criteria":{"billing":"Payments","technical":"Outages"}}}}"#,
            &[
                (
                    "all three",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    true,
                ),
                (
                    "missing answer id",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "extra answer id",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"other":{"type":"noul","noul":1},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "wrong type for an id",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"severity":{"type":"noul","noul":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "choice outside options",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"legal","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
                (
                    "choice probabilities are a subset",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1},"confidence":1}}}"#,
                    true,
                ),
                (
                    "noul answer has an extra field",
                    r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":0,"note":"x"},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    true,
                ),
                (
                    "wrong model",
                    r#"{"model":"other","answers":{"eligible":{"type":"noul","noul":0},"severity":{"type":"score","score":1,"legend":{},"probabilities":{},"confidence":1},"team":{"type":"choice","choice":"billing","probabilities":{"billing":1,"technical":0},"confidence":1}}}"#,
                    false,
                ),
            ],
        );
    }

    #[test]
    fn usage_whole_numbers_with_a_fraction_or_exponent_diverge() {
        let request = must_request(
            br#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        );
        let schema: Value =
            serde_json::from_slice(&response_schema(&request)).expect("schema is JSON");
        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .expect("response schema compiles");
        for document in [
            r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":1.0,"output_tokens":0}}"#,
            r#"{"model":"demo","answers":{"eligible":{"type":"noul","noul":1}},"usage":{"input_tokens":1e2,"output_tokens":2e1}}"#,
        ] {
            assert!(
                parse_decision(document.as_bytes()).is_err(),
                "bass accepted a lexical non-integer token count: {document}"
            );
            let value: Value = serde_json::from_str(document).expect("document is JSON");
            assert!(
                validator.is_valid(&value),
                "schema rejected a whole-valued token count JSON Schema cannot spell as an integer: {document}"
            );
        }
    }

    #[test]
    fn score_legend_length_agrees_on_ascii() {
        let at_limit = "x".repeat(MAX_LEGEND_BYTES);
        let over_limit = "x".repeat(MAX_LEGEND_BYTES + 1);
        let value_at = format!(
            r#"{{"model":"demo","answers":{{"severity":{{"type":"score","score":1,"legend":{{"0":"{at_limit}"}},"probabilities":{{}},"confidence":1}}}}}}"#
        );
        let value_over = format!(
            r#"{{"model":"demo","answers":{{"severity":{{"type":"score","score":1,"legend":{{"0":"{over_limit}"}},"probabilities":{{}},"confidence":1}}}}}}"#
        );
        let key_over = format!(
            r#"{{"model":"demo","answers":{{"severity":{{"type":"score","score":1,"legend":{{"{over_limit}":"note"}},"probabilities":{{}},"confidence":1}}}}}}"#
        );
        assert_response_agreement(
            r#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#,
            &[
                ("legend value of 65536 bytes", value_at.as_str(), true),
                ("legend value of 65537 bytes", value_over.as_str(), false),
                (
                    "legend key outside the index strings",
                    key_over.as_str(),
                    false,
                ),
            ],
        );
    }

    #[test]
    fn response_schema_diverges_where_it_cannot_match_bass() {
        let noul = must_request(
            br#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        );
        let noul_schema: Value =
            serde_json::from_slice(&response_schema(&noul)).expect("schema is JSON");
        let noul_validator = jsonschema::draft202012::options()
            .build(&noul_schema)
            .expect("response schema compiles");
        let duplicate =
            r#"{"model":"demo","model":"demo","answers":{"eligible":{"type":"noul","noul":1}}}"#;
        assert!(
            parse_decision(duplicate.as_bytes()).is_err(),
            "bass accepted a repeated response key"
        );
        let collapsed: Value = serde_json::from_str(duplicate).expect("parser keeps one key");
        assert!(
            noul_validator.is_valid(&collapsed),
            "repeated object key: the schema accepted the collapsed document, and it cannot see the repeat"
        );

        let score = must_request(
            br#"{"model":"demo","state":"text","questions":{"severity":{"type":"score","instructions":"How severe?","criteria":["low","high"]}}}"#,
        );
        let score_schema: Value =
            serde_json::from_slice(&response_schema(&score)).expect("schema is JSON");
        let score_validator = jsonschema::draft202012::options()
            .build(&score_schema)
            .expect("response schema compiles");
        let wide = "é".repeat(MAX_LEGEND_BYTES / 2 + 1);
        assert!(wide.len() > MAX_LEGEND_BYTES);
        assert!(wide.chars().count() <= MAX_LEGEND_BYTES);
        let document = format!(
            r#"{{"model":"demo","answers":{{"severity":{{"type":"score","score":1,"legend":{{"0":"{wide}"}},"probabilities":{{}},"confidence":1}}}}}}"#
        );
        let decision = parse_decision(document.as_bytes())
            .unwrap_or_else(|error| panic!("legend value did not parse: {error}"));
        assert_eq!(
            check_message(&score, &decision),
            "A legend string exceeds 65536 bytes."
        );
        let value: Value = serde_json::from_str(&document).expect("document is JSON");
        assert!(
            score_validator.is_valid(&value),
            "maxLength counts characters, so the schema accepted a legend value bass rejects for its byte length"
        );
    }

    #[test]
    fn schema_cannot_require_that_unknown_fields_are_dropped() {
        let request = must_request(
            br#"{"model":"demo","state":"text","questions":{"eligible":{"type":"noul","instructions":"Eligible?"}}}"#,
        );
        let document = r#"{"model":"demo","note":"drop-me","answers":{"eligible":{"type":"noul","noul":1,"extra":"also-drop"}}}"#;
        let decision = must_decision(document.as_bytes());
        assert_eq!(check_message(&request, &decision), "");
        let printed = String::from_utf8(render_decision(&decision)).expect("printed decision");
        assert!(!printed.contains("drop-me"));
        assert!(!printed.contains("also-drop"));
        assert!(!printed.contains("\"note\""));
        assert!(!printed.contains("\"extra\""));
        let schema: Value =
            serde_json::from_slice(&response_schema(&request)).expect("schema is JSON");
        let validator = jsonschema::draft202012::options()
            .build(&schema)
            .expect("response schema compiles");
        let original: Value = serde_json::from_str(document).expect("document is JSON");
        assert!(
            validator.is_valid(&original),
            "the schema rejected unknown fields that bass accepts"
        );
        let stripped: Value = serde_json::from_str(&printed).expect("printed decision is JSON");
        assert!(
            validator.is_valid(&stripped),
            "the schema rejected the printed decision"
        );
        assert_ne!(
            original, stripped,
            "the schema cannot tell the response that still contains unknown fields from the printed decision that drops them"
        );
    }
}
