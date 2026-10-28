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
}
