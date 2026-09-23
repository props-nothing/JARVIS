//! Run resource and event-stream wire types.
//!
//! These are the versioned shapes the local control API defines for creating,
//! inspecting, cancelling, and streaming a native run (see
//! `docs/contracts/local-control-api.md`). They live in `jarvis-protocol` rather
//! than in the HTTP adapter so the contract has one serialization definition that
//! a client, the daemon, and a fixture test all share â€” a type defined inside the
//! server would let the two sides drift while both still compiled.
//!
//! Three decisions are deliberate:
//!
//! - **Identifiers are carried as strings.** The contract requires lowercase
//!   canonical UUID text, and the daemon parses them into the domain's typed IDs at
//!   the boundary. Parsing here would mean this crate depends on the identifier
//!   implementation, and a boundary that cannot express "a client sent a
//!   non-UUID" is a boundary that cannot refuse it with a typed error.
//! - **Every command struct denies unknown fields.** The contract states that
//!   unknown JSON fields are rejected on commands, so a client cannot smuggle a
//!   field this build ignores into a request the daemon accepts.
//! - **Event payloads are per-type structs, not a map.** A free-form payload would
//!   let any field name reach the wire, including one holding prompt text or a
//!   secret, and the contract forbids both in a public payload. Naming the fields
//!   makes the forbidden ones unrepresentable.

use serde::{Deserialize, Serialize};

/// The contract version stamped into every event frame.
pub const RUN_CONTRACT_VERSION: &str = "0.1.0";

/// The largest accepted run input, in bytes, after UTF-8 decoding.
///
/// The contract bounds the input text to 32 KiB independently of the 64 KiB body
/// cap, because a request can be within the body limit and still carry input larger
/// than a run may accept.
pub const MAX_RUN_INPUT_BYTES: usize = 32 * 1024;

/// The largest accepted cancellation reason, in bytes.
pub const MAX_CANCEL_REASON_BYTES: usize = 512;

/// The runtime a run was created for.
pub const NATIVE_RUNTIME: &str = "jarvis-native";

/// A create-run command.
///
/// `runtime` is a string rather than an enum because the contract treats it as a
/// negotiable identifier: an unknown runtime should be refused with a typed
/// semantic error naming what is supported, not fail to parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    /// An existing conversation to continue, or `None` to start one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// The public input.
    pub input: RunInput,
    /// The requested runtime.
    pub runtime: String,
    /// The model policy to resolve and authorize.
    ///
    /// **Optional**, and the reason is structural rather than lenient: the policy identifier is
    /// derived from the workspace, and the workspace is resolved **server-side** from the
    /// authenticated client. A client therefore cannot name its own workspace's active policy
    /// without first reading it, and requiring the field would make every run uncreatable until an
    /// operator had configured a policy â€” including on a fresh installation.
    ///
    /// Absent means "govern this run by the workspace's active policy", which is the only
    /// resolution a client could have named. Present means the caller pinned a version, and it is
    /// honoured exactly: a named version that does not exist is refused rather than falling back to
    /// the active one, because falling back would apply rules the caller did not name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<ModelPolicyRef>,
}

/// The public input of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunInput {
    /// Plain text input.
    Text {
        /// The input text.
        text: String,
    },
}

impl RunInput {
    /// Builds a text input.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Returns the input text.
    #[must_use]
    pub fn text_value(&self) -> &str {
        match self {
            Self::Text { text } => text,
        }
    }
}

/// A reference to a model policy version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicyRef {
    /// The policy identifier.
    pub policy_id: String,
    /// The policy version.
    pub version: u32,
}

/// A cancellation command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelRunRequest {
    /// Why the caller is cancelling.
    pub reason: String,
}

/// The links a client follows for a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunLinks {
    /// The run's own resource path.
    #[serde(rename = "self")]
    pub self_path: String,
    /// The run's event-stream path.
    pub events: String,
}

/// The response to a successful run creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunResponse {
    /// The created run.
    pub run_id: String,
    /// The conversation it belongs to.
    pub conversation_id: String,
    /// Its client-visible state.
    pub state: String,
    /// When it was created.
    pub created_at: String,
    /// The links a client follows next.
    pub links: RunLinks,
}

/// An authenticated read of one run.
///
/// Deliberately bounded: it carries state, identity, the optimistic version, and
/// timestamps. It does **not** carry the prompt, the assembled context, provider
/// internals, or hidden reasoning, all of which the contract forbids on this
/// surface. The final answer is *not* included either: it is already durable as the
/// run's `run.output_text.delta` events, and the events endpoint replays them, so
/// duplicating it here would create a second place for the same text to be wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunView {
    /// The run identifier.
    pub run_id: String,
    /// The conversation it belongs to.
    pub conversation_id: String,
    /// Its client-visible state.
    pub state: String,
    /// The monotonically increasing resource version.
    pub version: u64,
    /// When it was created.
    pub created_at: String,
    /// When it started, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When it last changed.
    pub updated_at: String,
    /// When it reached a terminal state, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The normalized error code, when it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// The data body of one streamed run event.
///
/// The event *type* travels in the SSE `event:` line, so this shape carries only the
/// data the contract's example shows. Field names are fixed per payload rather than
/// free-form, which is what keeps prompt text and secret material out of a public
/// payload by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventFrame {
    /// The contract version this frame was produced under.
    pub contract_version: String,
    /// The globally unique event identifier.
    pub event_id: String,
    /// The run the event belongs to.
    pub run_id: String,
    /// The position in the run's stream, starting at 1.
    pub sequence: u64,
    /// When the event occurred.
    pub occurred_at: String,
    /// The type-specific, already-redacted payload.
    pub payload: serde_json::Value,
}

/// One rendered server-sent event.
///
/// Keeps the SSE framing rules â€” a fixed field order, an `id`, and a terminating
/// blank line â€” in one place, so a writer cannot produce a frame a client's parser
/// will not accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The SSE `id:` value, which a client echoes back as `Last-Event-ID`.
    pub id: String,
    /// The SSE `event:` value.
    pub event_type: String,
    /// The serialized [`RunEventFrame`].
    pub data: String,
}

impl SseEvent {
    /// Renders the event as a complete SSE frame.
    ///
    /// A comment (keepalive) is `: text\n\n` and carries no `id`, which is why the
    /// contract can state that keepalives do not consume sequence numbers.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "id: {id}\nevent: {event_type}\ndata: {data}\n\n",
            id = self.id,
            event_type = self.event_type,
            data = self.data,
        )
    }
}

/// Renders a keepalive comment frame.
#[must_use]
pub fn keepalive_frame() -> String {
    ": keepalive\n\n".to_owned()
}

/// The event types the first slice must publish.
///
/// Named constants rather than literals at each call site, because the wire string
/// is a contract field a client switches on: a typo would be invisible here and
/// break a consumer.
pub mod event_type {
    /// The run was created.
    pub const RECEIVED: &str = "run.received";
    /// Context is being resolved.
    pub const CONTEXT_BUILDING: &str = "run.context_building";
    /// A decision was taken.
    pub const PLANNING: &str = "run.planning";
    /// A model call started.
    pub const MODEL_STARTED: &str = "run.model_started";
    /// A chunk of output text.
    pub const OUTPUT_TEXT_DELTA: &str = "run.output_text.delta";
    /// Provider-reported usage.
    pub const USAGE: &str = "run.usage";
    /// The final answer is being produced.
    pub const RESPONDING: &str = "run.responding";
    /// The run completed.
    pub const COMPLETED: &str = "run.completed";
    /// The run failed.
    pub const FAILED: &str = "run.failed";
    /// The run was cancelled.
    pub const CANCELLED: &str = "run.cancelled";
}

/// The client-visible run states this contract exposes.
///
/// `RunView.state` carries one of these, and the contract states the set twice â€” the initial
/// four and the three terminal ones â€” so a client switches on a value that is fixed here
/// rather than at each call site. Same reasoning as [`event_type`]: a typo is invisible in
/// this build and breaks a consumer.
///
/// **Why this module did not exist, and why that mattered.** The projection that produces
/// these strings lives in the HTTP adapter (`jarvis_infrastructure::http::runs::wire_state`)
/// because `docs/architecture/agent-runtime.md` forbids domain state names doubling as wire
/// strings â€” so the adapter is the right *place* for the mapping. But the adapter is not a
/// place the contract can be compared against: `jarvis-application` cannot depend on
/// `jarvis-protocol`, and the adapter's own test could only count how many distinct strings
/// the projection produced. It asserted the image's **cardinality** (seven) and three
/// terminal names, so renaming any one of the seven kept the count, kept the terminals, and
/// passed every gate and every journey â€” the projection and the contract could disagree
/// about the client-visible state set with nothing failing.
///
/// With the vocabulary owned here, the adapter spells its arms from these constants and this
/// crate â€” which reads the contract document â€” asserts the set against it in both directions.
pub mod run_state {
    /// The run exists and has not started work.
    pub const RECEIVED: &str = "received";
    /// Identity, policy, and context are being resolved, or a decision is being taken.
    pub const CONTEXT_BUILDING: &str = "context_building";
    /// Work is outstanding: a model call, an approval, a tool, or a parked dependency.
    pub const MODEL_RUNNING: &str = "model_running";
    /// The final answer is being produced.
    pub const RESPONDING: &str = "responding";
    /// The run finished with a result.
    pub const COMPLETED: &str = "completed";
    /// The run ended in a normalized error.
    pub const FAILED: &str = "failed";
    /// The run ended because cancellation was requested.
    pub const CANCELLED: &str = "cancelled";
}

/// Builds the payload for an output-text delta.
#[must_use]
pub fn output_text_delta_payload(item_id: &str, delta: &str) -> serde_json::Value {
    serde_json::json!({ "item_id": item_id, "delta": delta })
}

/// Builds the payload for a normalized failure.
#[must_use]
pub fn failed_payload(code: &str, retryable: bool) -> serde_json::Value {
    serde_json::json!({ "code": code, "retryable": retryable })
}

/// Builds the payload for a cancellation.
#[must_use]
pub fn cancelled_payload(reason_code: &str) -> serde_json::Value {
    serde_json::json!({ "reason": reason_code })
}

/// Builds the payload for provider-reported usage.
#[must_use]
pub fn usage_payload(input_tokens: u64, output_tokens: u64) -> serde_json::Value {
    serde_json::json!({ "input_tokens": input_tokens, "output_tokens": output_tokens })
}

/// Builds the links for a run.
#[must_use]
pub fn run_links(run_id: &str) -> RunLinks {
    RunLinks {
        self_path: format!("/api/v1/runs/{run_id}"),
        events: format!("/api/v1/runs/{run_id}/events"),
    }
}

#[cfg(test)]
#[path = "run_contract_tests.rs"]
mod contract_tests;

#[cfg(test)]
mod tests {
    use super::{
        CancelRunRequest, CreateRunRequest, MAX_RUN_INPUT_BYTES, NATIVE_RUNTIME, RunInput,
        RunLinks, RunView, SseEvent, keepalive_frame, output_text_delta_payload, run_links,
    };

    #[test]
    fn a_create_request_parses_the_contract_example() {
        let json = r#"{
            "conversation_id": null,
            "input": {"type":"text","text":"hello"},
            "runtime": "jarvis-native",
            "model_policy": {"policy_id":"scripted-test","version":1}
        }"#;
        let request: CreateRunRequest = serde_json::from_str(json).expect("parses");
        assert_eq!(request.conversation_id, None);
        assert_eq!(request.input.text_value(), "hello");
        assert_eq!(request.runtime, NATIVE_RUNTIME);
        let policy = request.model_policy.expect("a pinned policy is carried");
        assert_eq!(policy.policy_id, "scripted-test");
        assert_eq!(policy.version, 1);
    }

    #[test]
    fn a_create_request_without_a_policy_is_the_workspace_active_one() {
        // The field is optional because only the daemon can resolve a workspace's active policy:
        // the identifier is derived from the workspace, which is resolved server-side, so a client
        // cannot name it without first reading it. Omitting the field is therefore the ordinary
        // case â€” and it must parse, because requiring it would make every run uncreatable until an
        // operator had configured a policy.
        let json = r#"{
            "conversation_id": null,
            "input": {"type":"text","text":"hello"},
            "runtime": "jarvis-native"
        }"#;
        let request: CreateRunRequest = serde_json::from_str(json).expect("parses");
        assert!(
            request.model_policy.is_none(),
            "an absent policy means the active one, not a parse failure",
        );
    }

    #[test]
    fn an_unknown_command_field_is_rejected() {
        // A client that sends a field this build ignores must learn so, rather than
        // have the field silently dropped from a request the daemon then accepts.
        let json = r#"{
            "input": {"type":"text","text":"hello"},
            "runtime": "jarvis-native",
            "model_policy": {"policy_id":"p","version":1},
            "provider_api_key": "secret"
        }"#;
        assert!(serde_json::from_str::<CreateRunRequest>(json).is_err());
    }

    #[test]
    fn an_unknown_input_field_is_rejected() {
        let json = r#"{
            "input": {"type":"text","text":"hello","max_tokens":100000},
            "runtime": "jarvis-native",
            "model_policy": {"policy_id":"p","version":1}
        }"#;
        assert!(serde_json::from_str::<CreateRunRequest>(json).is_err());
    }

    #[test]
    fn an_unknown_input_type_is_rejected() {
        // The tag is a closed set in this contract version, so an unsupported input
        // kind is a parse failure rather than a run created with empty input.
        let json = r#"{
            "input": {"type":"image","url":"https://example.invalid/x.png"},
            "runtime": "jarvis-native",
            "model_policy": {"policy_id":"p","version":1}
        }"#;
        assert!(serde_json::from_str::<CreateRunRequest>(json).is_err());
    }

    #[test]
    fn a_cancel_reason_round_trips() {
        let request: CancelRunRequest =
            serde_json::from_str(r#"{"reason":"user_requested"}"#).expect("parses");
        assert_eq!(request.reason, "user_requested");
        assert!(
            serde_json::from_str::<CancelRunRequest>(r#"{"reason":"x","force":true}"#).is_err()
        );
    }

    #[test]
    fn the_run_view_omits_absent_optional_fields() {
        let view = RunView {
            run_id: "0195f4f0-4c13-7bf4-89fb-f067adac13ee".to_owned(),
            conversation_id: "0195f4f0-4c13-7bf4-89fb-f067adac13ef".to_owned(),
            state: "received".to_owned(),
            version: 1,
            created_at: "2026-09-20T12:35:10Z".to_owned(),
            started_at: None,
            updated_at: "2026-09-20T12:35:10Z".to_owned(),
            completed_at: None,
            error_code: None,
        };
        let json = serde_json::to_string(&view).expect("serializes");
        assert!(json.contains(r#""version":1"#), "{json}");
        // An absent optional field is omitted, so a client can distinguish "not set"
        // from "set to an empty value" without a null check.
        assert!(!json.contains("completed_at"), "{json}");
        assert!(!json.contains("error_code"), "{json}");
        assert!(!json.contains("prompt"), "{json}");
    }

    #[test]
    fn the_self_link_serializes_under_its_contract_name() {
        let links = RunLinks {
            self_path: "/api/v1/runs/abc".to_owned(),
            events: "/api/v1/runs/abc/events".to_owned(),
        };
        let json = serde_json::to_string(&links).expect("serializes");
        assert!(json.contains(r#""self":"/api/v1/runs/abc""#), "{json}");
        assert!(
            json.contains(r#""events":"/api/v1/runs/abc/events""#),
            "{json}"
        );
    }

    #[test]
    fn run_links_are_derived_from_the_run_id() {
        let links = run_links("0195f4f0-4c13-7bf4-89fb-f067adac13ee");
        assert_eq!(
            links.self_path,
            "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee"
        );
        assert_eq!(
            links.events,
            "/api/v1/runs/0195f4f0-4c13-7bf4-89fb-f067adac13ee/events"
        );
    }

    #[test]
    fn an_sse_frame_matches_the_contract_framing() {
        let event = SseEvent {
            id: "0195f4f1-0475-7613-a92c-edf01183e909".to_owned(),
            event_type: "run.output_text.delta".to_owned(),
            data: r#"{"contract_version":"0.1.0"}"#.to_owned(),
        };
        let rendered = event.render();
        assert_eq!(
            rendered,
            "id: 0195f4f1-0475-7613-a92c-edf01183e909\n\
             event: run.output_text.delta\n\
             data: {\"contract_version\":\"0.1.0\"}\n\n",
        );
        // A frame ends with a blank line, which is what terminates it for a parser.
        assert!(rendered.ends_with("\n\n"));
    }

    #[test]
    fn a_keepalive_is_a_comment_and_consumes_no_sequence() {
        let frame = keepalive_frame();
        assert!(frame.starts_with(':'), "{frame}");
        assert!(!frame.contains("id:"), "{frame}");
    }

    #[test]
    fn a_delta_payload_carries_only_text() {
        let payload = output_text_delta_payload("out-1", "Hello");
        assert_eq!(payload["delta"], "Hello");
        assert_eq!(payload["item_id"], "out-1");
        // The payload is a fixed shape, so no other key can appear.
        assert_eq!(payload.as_object().expect("object").len(), 2);
    }

    #[test]
    fn the_input_bound_is_the_contract_value() {
        assert_eq!(MAX_RUN_INPUT_BYTES, 32 * 1024);
        assert_eq!(RunInput::text("x").text_value(), "x");
    }
}
