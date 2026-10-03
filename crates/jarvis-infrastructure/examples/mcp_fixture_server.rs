//! A minimal MCP server over stdio, used as a **real child process** by the discovery tests.
//!
//! It exists because discovery's load-bearing behavior is the spawn: the child must be a separate process
//! reached over a pipe, and an in-process fixture cannot falsify an implementation that drops the transport,
//! negotiates the wrong lifecycle, or never passes its configured environment through `env_clear`.
//!
//! # Why an example rather than a `[[bin]]`
//!
//! The `server` half of `rmcp` is a **dev-dependency** of this crate, so a `[[bin]]` target — which is built
//! with normal dependencies — could not link it. Examples are built with dev-dependencies, so this is the one
//! target kind that can both exist and speak the server protocol.
//!
//! # Why it matters that the harness is off
//!
//! Protocol framing is newline-delimited JSON **on stdout**. libtest writes its own progress banner to
//! stdout, which would interleave with the framing and make the child look like a server emitting garbage.
//! Cargo's `harness` **defaults to `false` for examples** (unlike a `[[test]]` target), so this program's
//! stdout already carries only protocol frames and no manifest flag is needed. Stated here because the
//! property is load-bearing and invisible: turning this example into a test target would break framing
//! without changing a line of it.
//!
//! # Which fixture it is, is chosen by the caller
//!
//! The parent passes [`FIXTURE_ENV`] in the launch spec's environment. That is deliberate: `build_command`
//! calls `env_clear()`, so nothing reaches this child that the parent did not list — which means a test that
//! sets this variable is also testing that the configured environment is actually delivered.

#![allow(clippy::panic)]

use std::borrow::Cow;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, CreateTaskResult,
    Implementation, InputRequiredResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, ServerConfig, TextContent, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, serve_server};

/// The environment variable naming which fixture to be.
pub const FIXTURE_ENV: &str = "JARVIS_MCP_FIXTURE";

/// The variable's value for the ordinary fixture: offers one usable tool, speaks the target version.
pub const FIXTURE_STANDARD: &str = "standard";

/// The variable's value for a peer that offers only one usable tool and an **older** protocol version.
///
/// This is the fixture that falsifies the claim that discovery refuses a legacy peer. It does not: discovery
/// negotiates downward within the versions both sides list, so this peer is a legitimate session at
/// `2025-06-18`.
pub const FIXTURE_LEGACY: &str = "legacy";

/// The variable's value for a peer whose only tool cannot be normalized.
///
/// The tool name contains a space, which is not a usable canonical name segment — so the normalizer refuses
/// it and the discovery must report an all-refused catalog rather than an empty one.
pub const FIXTURE_ALL_REFUSED: &str = "all-refused";

/// The variable's value for a peer that lists **one usable tool and one unusable one**.
///
/// This is the *partial* refusal, and it exists because the other fixtures cannot produce it: a server whose
/// only tool is refused takes the whole-server path, while a server with any usable tool takes none. Without a
/// fixture that offers both, the case where a malformed tool is silently dropped from an otherwise healthy
/// server has no detector at all.
pub const FIXTURE_PARTIAL: &str = "partial";

/// The variable's value for a peer that **lists and accepts a call but never answers it**.
///
/// This exists so a timeout can be produced deterministically. A timeout is the one class that reaches the
/// `ToolExecutor` port as `Ambiguous`, and that arm is the most safety-relevant decision in the executor: a
/// class meaning "an effect may exist" reported as a plain failure would let the recovery pass treat a
/// possibly-executed call as one that provably did nothing. A fixture that answers promptly cannot exercise
/// it, and no unit test can, because the mapping is inside `execute`.
pub const FIXTURE_UNANSWERED: &str = "unanswered";

/// How long the unanswered fixture withholds its answer.
///
/// Far longer than any test's call bound, so the timeout is not a race: the point is that the call does not
/// complete, not that it completes slowly.
pub const FIXTURE_UNANSWERED_DELAY_MS: u64 = 30_000;

/// The variable's value for a peer that answers a call **after a bounded delay** rather than never.
///
/// **It exists to tell a short bound from a generous one by the OUTCOME rather than by the clock.** A fixture
/// that never answers times out under any bound, so a test cannot distinguish "the request's 250 ms bound was
/// enforced" from "the executor used its own 10 s fallback": both produce a timeout, one ten seconds later. A
/// delayed answer separates them — a short bound yields `Ambiguous` while a generous one yields the fixture's
/// text — so the assertion is a different *kind* of result rather than a shorter wait.
pub const FIXTURE_SLOW: &str = "slow";

/// How long the slow fixture withholds its answer before sending it.
///
/// Between the two bounds the executor tests use (250 ms and 10 s), so exactly one of them can elapse
/// first. Chosen well inside that gap so neither side is a race.
pub const FIXTURE_SLOW_DELAY_MS: u64 = 1_500;

/// The variable's value for a peer whose only tool answers with **the child's own working directory**.
///
/// **This is how a launch directory is provable.** Nothing else can show that `current_dir` was applied: a
/// specification field can be asserted, a built command can be inspected, but only a running process can say
/// where it actually is — and "the argument was threaded through" is a different claim from "the child ran
/// there". The tool reports the directory the operating system gave it rather than one it was told about.
pub const FIXTURE_CWD: &str = "cwd";

/// The variable's value for a peer that answers a call with **a task handle** rather than a result.
///
/// **It exists to show that JARVIS's own answer to a task handle is never the one a caller meets.** The
/// adapter has a considered arm for this: `call.rs` refuses a task handle with its own code,
/// `mcp.response_not_complete`, on the reading that answering one is polling a lifecycle this build does not
/// implement. That arm is correct and it is **unreachable**, for two independent reasons — the SDK's client
/// helper converts the response to `ServiceError::UnexpectedResponse` before any JARVIS decision function is
/// consulted, and the SDK's *server* half refuses to emit a task handle at all unless the client declared the
/// tasks extension capability, which `JarvisClient` deliberately does not. A fixture that can produce the
/// response is the only way to observe which answer actually arrives.
///
/// The tool is the ordinary one, so nothing else about the session differs: only the answer's shape is the
/// variable under test.
pub const FIXTURE_TASK: &str = "task";

/// The variable's value for a peer that answers a call with **`input_required`, forever**.
///
/// **It exists to pin which bound actually ends an exhausted round.** `MAX_MRTR_ROUNDS` is JARVIS's cap and
/// is passed to the SDK's loop as `max_rounds`, so the loop stops at JARVIS's number — but the *class* a
/// caller receives is neither JARVIS's `mcp.round_limit_exceeded` (which no longer exists) nor the class
/// `call.rs` would assign an input round (which is never consulted). It is `tool.limit_exceeded`, because
/// the SDK reports `InputRequiredRoundsExceeded` and `classify_service_error` maps it.
///
/// The answer carries a `request_state`, and that is load-bearing: a round with **no** requests and no state is
/// rejected by the SDK's own loop as `UnexpectedResponse` before any retry, so a fixture without one would pin
/// a different defect than the round cap.
pub const FIXTURE_INPUT_REQUIRED: &str = "input-required";

/// The variable's value for a peer that answers every attempt with **a `request_state` and no request**.
///
/// **This is the only shape that reaches the round cap, and that is the finding.** A round that *names* a
/// request is answered by JARVIS's handler with `-32602`, which propagates out of the SDK's loop as an error
/// before the next round — so the cap is never reached for a server that actually asks a question. A round
/// carrying only a `request_state` consults no handler at all, so the loop retries until `max_rounds` and the
/// cap is what ends the call.
///
/// So `FIXTURE_INPUT_REQUIRED` and this fixture exist as a **pair**: together they show that a bound JARVIS
/// describes as its own is reachable only through a shape that asks JARVIS nothing.
pub const FIXTURE_STATE_ONLY: &str = "state-only";

/// The tool name the standard and legacy fixtures offer.
pub const FIXTURE_TOOL_NAME: &str = "read_file";

/// The tool name the all-refused fixture offers.
pub const FIXTURE_UNUSABLE_TOOL_NAME: &str = "Read File";

/// The tool name the cwd fixture offers.
pub const FIXTURE_CWD_TOOL_NAME: &str = "where_am_i";

/// The text a fixture returns from a successful call.
///
/// Exported so a test asserts against the same constant the fixture sends, rather than against a literal that
/// could drift from it — the failure mode a shared fixture constant exists to prevent.
pub const FIXTURE_CALL_TEXT: &str = "fixture result for read_file";

/// The version the legacy fixture reports, and the only one it reports.
pub const FIXTURE_LEGACY_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// How a fixture answers a call, for the shapes that are not a completed result.
///
/// An enum rather than two booleans, because only one of these can apply to a call and a pair of flags would
/// make the impossible combination representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoundAnswer {
    /// Answer with a completed result, which is what most fixtures do.
    Complete,
    /// Answer `input_required` naming a request, which JARVIS refuses by kind.
    Refuses,
    /// Answer `input_required` carrying only a `request_state`, so no handler is consulted.
    StateOnly,
    /// Answer with a task handle rather than a result.
    Task,
}

/// An MCP server that behaves as the selected fixture.
#[derive(Debug, Clone)]
struct FixtureServer {
    /// The versions this fixture reports during discovery.
    supported: Vec<ProtocolVersion>,
    /// The tools this fixture offers, in order.
    ///
    /// A slice rather than one name so a fixture can offer a **mix**, which is the only way to produce a
    /// partial normalization refusal: a server with one usable tool and one refused one.
    tools: &'static [&'static str],
    /// Whether a call is accepted and then **never answered**, so the client's bound elapses.
    withhold_answer: bool,
    /// How long to wait before answering a call, when the fixture is the slow one.
    ///
    /// `None` answers immediately. A duration rather than a flag beside `withhold_answer`, because the two are
    /// different behaviours — one never answers, the other answers late — and a single boolean would have to
    /// encode both.
    answer_after_ms: Option<u64>,
    /// Whether a call answers with this process's own working directory.
    ///
    /// The child reports what the operating system gave it, so a test asserting the text is asserting where
    /// the process actually is rather than what it was told.
    report_cwd: bool,
    /// Whether a call answers with a **task handle** rather than a completed result.
    ///
    /// The multi-round-trip and task shapes are one field because they are one decision: what *kind* of
    /// answer this fixture gives. Two booleans would let a caller set a combination that cannot happen.
    answer: RoundAnswer,
}

impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("jarvis-mcp-fixture", "1.0.0"))
    }

    /// Overridden because the default reports only the SDK's newest revision, and the legacy fixture exists
    /// precisely to report something else.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(self.supported.clone())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + '_ {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"],
            "additionalProperties": false,
        });
        // `Tool::new` wants an `Arc<JsonObject>`, so the schema is converted from the `json!` value rather
        // than built as a map literal — the same shape the unit tests use.
        let map = match schema.as_object() {
            Some(map) => map.clone(),
            None => return std::future::ready(Ok(ListToolsResult::with_all_items(Vec::new()))),
        };
        let tools = self
            .tools
            .iter()
            .map(|name| Tool::new(*name, "Read a file.", map.clone()))
            .collect();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    /// Answers a call for the tool this fixture listed, and refuses anything else.
    ///
    /// **Implementing this is what makes the fixture usable for the executor suite, and its absence was a real
    /// defect in the fixture rather than in the code under test.** Without an override the SDK's default
    /// handler answers every call with `METHOD_NOT_FOUND`, so a client exercising the call path saw "no such
    /// tool" for a tool the fixture had just listed — which reads exactly like a name-mapping bug and is not
    /// one. A fixture that lists a tool must be able to serve it, or anything built on it is testing the
    /// default rather than the adapter.
    ///
    /// The refusal is kept for a name this fixture did not list, so the **wrong-name** case is still
    /// observable as a protocol error: the executor suite relies on the peer being the thing that complains.
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + '_ {
        // Copied out of `self` before the block, so the returned future holds no borrow of the server — the
        // shape the SDK's own handlers use, and what lets `+ '_` stay honest.
        let name = request.name;
        let tools = self.tools;
        let withhold = self.withhold_answer;
        let answer_after_ms = self.answer_after_ms;
        let report_cwd = self.report_cwd;
        let answer = self.answer;
        async move {
            if !tools.contains(&name.as_ref()) {
                return Err(rmcp::ErrorData::new(
                    rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                    "no such tool",
                    None,
                ));
            }
            // The name is right, so the tool is *accepted* — and then the answer is withheld. Nothing is
            // refused: the client's own bound is what ends the call, which is what makes this a timeout
            // rather than a protocol error.
            if withhold {
                tokio::time::sleep(std::time::Duration::from_millis(
                    FIXTURE_UNANSWERED_DELAY_MS,
                ))
                .await;
            }
            // Or answered **late**: accepted, waited on, and then served. Which of the two happens is what
            // lets a test tell a short bound from a generous one by the result rather than by the clock.
            if let Some(delay) = answer_after_ms {
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            // The two multi-round-trip shapes, and the difference between them is the whole finding: naming a
            // request makes JARVIS's handler answer `-32602`, which ends the call at round one, while a
            // `request_state` alone consults no handler and retries until the cap.
            match answer {
                RoundAnswer::Refuses => {
                    let Ok(required) = input_required(Some(serde_json::json!({
                        "roots": {"method": "roots/list", "params": {}},
                    }))) else {
                        return Err(fixture_build_failure());
                    };
                    return Ok(CallToolResponse::InputRequired(required));
                }
                RoundAnswer::StateOnly => {
                    let Ok(required) = input_required(None) else {
                        return Err(fixture_build_failure());
                    };
                    return Ok(CallToolResponse::InputRequired(required));
                }
                RoundAnswer::Task => {
                    // Built from the **wire form** rather than a constructor, because `CreateTaskResult` has
                    // no public constructor here — deserializing the documented shape is the only way this
                    // fixture can produce the response a real server sends.
                    let task: CreateTaskResult = match serde_json::from_value(serde_json::json!({
                        "resultType": "task",
                        "taskId": "fixture-task",
                        "status": "working",
                        "createdAt": "2026-10-03T00:00:00Z",
                        "lastUpdatedAt": "2026-10-03T00:00:00Z",
                        "ttlMs": null,
                    })) {
                        Ok(task) => task,
                        Err(_) => return Err(fixture_build_failure()),
                    };
                    return Ok(CallToolResponse::Task(task));
                }
                RoundAnswer::Complete => {}
            }
            // The process reports where it **is**, which is the only way to prove `current_dir` reached it.
            let text = if report_cwd {
                std::env::current_dir().map_or_else(
                    |_| "cwd-unavailable".to_owned(),
                    |directory| directory.display().to_string(),
                )
            } else {
                FIXTURE_CALL_TEXT.to_owned()
            };
            Ok(CallToolResponse::Complete(CallToolResult::success(vec![
                ContentBlock::Text(TextContent::new(text)),
            ])))
        }
    }
}

/// The one failure a fixture can report about **its own** construction.
///
/// A construction site rather than three, so a broken literal reads the same wherever it is built. A fixture
/// that could not build its own answer has no protocol-level explanation, so this is an internal error.
fn fixture_build_failure() -> rmcp::ErrorData {
    rmcp::ErrorData::new(
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        "the fixture answer did not build",
        None,
    )
}

/// Builds an `input_required` answer, with a request when one is named.
///
/// The two shapes are one function because they differ by exactly one field, and the *field* is what the
/// caller has to decide — passing `None` produces the state-only round that reaches the round cap, while a
/// request produces the round JARVIS answers with `-32602`.
fn input_required(
    input_requests: Option<serde_json::Value>,
) -> Result<InputRequiredResult, serde_json::Error> {
    let mut answer = serde_json::json!({
        "resultType": "input_required",
        "requestState": "fixture-state",
    });
    if let Some(requests) = input_requests {
        answer["inputRequests"] = requests;
    }
    serde_json::from_value(answer)
}

/// Selects the fixture named by the environment, or `None` when the variable is absent.
///
/// `None` is **not** an error: it is how this program behaves when Cargo runs it as an example build, where
/// exiting immediately is the correct outcome rather than serving a protocol to nobody.
fn selected_fixture() -> Option<FixtureServer> {
    let name = std::env::var(FIXTURE_ENV).ok()?;
    let both = vec![ProtocolVersion::V_2026_07_28, FIXTURE_LEGACY_VERSION];
    let server = match name.as_str() {
        FIXTURE_STANDARD => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_LEGACY => FixtureServer {
            supported: vec![FIXTURE_LEGACY_VERSION],
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_ALL_REFUSED => FixtureServer {
            supported: both,
            tools: &[FIXTURE_UNUSABLE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_PARTIAL => FixtureServer {
            supported: both,
            // The usable one **first**, so a bug that served only the first entry would still look healthy to
            // a test that checked for a callable tool rather than for the refusal beside it.
            tools: &[FIXTURE_TOOL_NAME, FIXTURE_UNUSABLE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_UNANSWERED => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: true,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_SLOW => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: Some(FIXTURE_SLOW_DELAY_MS),
            report_cwd: false,
            answer: RoundAnswer::Complete,
        },
        FIXTURE_CWD => FixtureServer {
            supported: both,
            tools: &[FIXTURE_CWD_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: true,
            answer: RoundAnswer::Complete,
        },
        // The ordinary tool and the ordinary timing, so the **answer's shape** is the only variable — which is
        // what makes the resulting class attributable to the task handle rather than to anything else.
        FIXTURE_TASK => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Task,
        },
        // The ordinary tool again, so the only variable is that every answer is a further round.
        FIXTURE_INPUT_REQUIRED => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::Refuses,
        },
        // And the same answer **without a request**, which is the only shape that reaches the round cap.
        FIXTURE_STATE_ONLY => FixtureServer {
            supported: both,
            tools: &[FIXTURE_TOOL_NAME],
            withhold_answer: false,
            answer_after_ms: None,
            report_cwd: false,
            answer: RoundAnswer::StateOnly,
        },
        _ => return None,
    };
    Some(server)
}

fn main() {
    let Some(server) = selected_fixture() else {
        // Nothing was asked of this program, so there is nothing to do. Exiting zero keeps this target
        // harmless when Cargo compiles or runs it as an example rather than as a fixture.
        return;
    };

    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        // A runtime that cannot be built is not something a fixture can report over a protocol that needs a
        // runtime to speak, so the exit code is the report.
        std::process::exit(2);
    };

    let exit = runtime.block_on(async move {
        // `stdio()` gives the child the two pipes the parent created; the protocol runs on stdout and the
        // example harness is off by default, so nothing else writes there.
        match serve_server(server, rmcp::transport::io::stdio()).await {
            Ok(service) => {
                // Held, not discarded: `let _ =` would drop the `RunningService`, and its `Drop` cancels the
                // connection — the failure mode that made three tests in `mcp_conversation.rs` fail with
                // `TransportClosed` after their first exchange.
                let completion = service.waiting().await;
                if completion.is_ok() { 0 } else { 3 }
            }
            Err(_) => 4,
        }
    });
    std::process::exit(exit);
}
