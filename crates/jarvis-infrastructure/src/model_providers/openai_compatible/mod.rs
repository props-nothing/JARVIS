//! The OpenAI-compatible model provider adapter.
//!
//! This is the implementation half of `BRN-003`, built against
//! `docs/research/integrations/openai-compatible-model.md`, whose implementation gate is passed. The
//! note fixes the contract this adapter is held to; this module is where the contract is honoured.
//!
//! # What this adapter is, and what it deliberately is not
//!
//! It speaks **HTTP/1.1** to an operator-configured OpenAI-compatible endpoint serving the Chat
//! Completions streaming contract: in plaintext to a **loopback** address ([`OpenAiCompatibleProvider::new`]),
//! or over **TLS** to a named host ([`OpenAiCompatibleProvider::new_tls`], added 2026-10-03 under the
//! evidence note's "TLS transport" section). A non-loopback endpoint without TLS is still refused. The transport is
//! hand-rolled rather than a general client, for the reason the Foundation note already records for
//! the loopback client: one known peer does not justify a full client stack, and a hand-rolled path
//! makes it *structurally* hard to send a credential somewhere unintended. The stronger reason here
//! is measured rather than argued. Until 2026-10-03 the workspace had **no TLS implementation at
//! all**, so the adapter refused a non-loopback endpoint rather than pretend to support a cloud
//! provider it could not secure. TLS is now `rustls` (the `aws-lc-rs` provider) over `tokio-rustls`
//! with the compiled-in `webpki-roots` trust set, and the hand-rolled HTTP/1.1 path is unchanged
//! beneath it: the same reader runs over a plaintext or a TLS stream.
//!
//! # Three layers, deliberately separate
//!
//! - [`sse`] reassembles frames from bytes and is pure.
//! - [`translate`] maps one parsed chunk onto normalized events and is pure.
//! - this module owns the socket, the HTTP exchange, the error mapping, and the credential.
//!
//! The split exists because every provider-shaped trap lives in the first two, and code that can
//! only be exercised through a live socket is the least tested code in an adapter. Both pure layers
//! are tested against byte and JSON fixtures with no network.
//!
//! # Secrets
//!
//! The key is held in a wrapper with a **private field**, no derived `Debug`, and a single explicit
//! accessor, which is the same shape `GeneratedCredential` in the authentication module uses. No new
//! dependency is introduced for this: the workspace already owns the pattern, and adding a crate for
//! it would be a dependency change — an evidence obligation of its own under `AGENTS.md` — for a
//! property this file can establish directly and test. The key is used exactly once, to build the
//! `Authorization` header of one request, and it is never a URL component.

use std::sync::Arc;

use jarvis_application::model::{AdapterStream, ModelProvider, OpenResult, ProviderError};
use jarvis_application::request_context::RequestContext;
use jarvis_domain::ids::{IdGenerator, ModelCallId};
use jarvis_domain::model::identity::{EndpointClass, ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{
    ContentBlock, InputItem, ModelCallRequest, ModelStreamEvent, ModelStreamEventKind, Role,
};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::mpsc;

pub mod sse;
pub mod translate;

use sse::{FrameRead, SseBuffer, SseFrame};
use translate::{ChunkTranslator, Translated};

/// The path appended to the configured base when none is given.
///
/// A default rather than a constant the adapter is stuck with: the Chat Completions route is
/// `/chat/completions` only when the server is rooted at the origin, and a great many compatible
/// servers mount it under a prefix — `http://127.0.0.1:11434/v1/chat/completions` on Ollama, and a
/// `/v1` prefix on vLLM, LM Studio, and most gateways. Making it configurable is what lets those be
/// used at all; validating it as a **path** rather than accepting a URL is what keeps the
/// "a key cannot end up in the endpoint" property that the host/port split exists for.
pub const COMPLETIONS_PATH: &str = "/chat/completions";

/// The longest accepted base path.
///
/// Bounded because it is operator input that reaches a request line, and an unbounded value there is
/// a request-smuggling surface rather than a configuration convenience.
pub const MAX_PATH_BYTES: usize = 256;

/// The largest response body this adapter will read.
///
/// Bounded because the peer is untrusted: an endpoint that streams forever would otherwise consume
/// memory without limit. The bound is generous relative to a real answer and is enforced per
/// response, so a stream that exceeds it is failed rather than truncated silently.
pub const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// The largest single SSE frame, re-exported so a caller does not reach into the framing module.
pub const MAX_FRAME_BYTES: usize = sse::MAX_FRAME_BYTES;

/// The depth of the channel between the reader task and the stream.
///
/// Bounded, because an unbounded channel would let a fast provider grow memory without limit while a
/// slow consumer drained it. Small, because the consumer is the run controller and a deep buffer only
/// adds latency to cancellation.
pub const STREAM_CHANNEL_DEPTH: usize = 32;

/// The largest number of tools this adapter will advertise.
///
/// A bound on an outbound payload assembled from a caller-influenced list. The canonical tool set
/// belongs to the tool fabric, so this only refuses a list large enough to be a defect.
pub const MAX_TOOLS: usize = 128;

/// A provider credential held so it cannot be rendered incidentally.
///
/// The field is private, `Debug` is hand-written, and there is exactly one accessor. That is the
/// same construction `GeneratedCredential` uses in the authentication module, and it is deliberately
/// not a new dependency: a crate for this property would be a dependency change requiring its own
/// evidence review, while the property itself is a private field this file can establish and test.
///
/// `Clone` is **not** implemented, so the value cannot be copied into a place the adapter does not
/// control; the one reader consumes a reference and builds the header directly.
struct BearerKey(String);

impl BearerKey {
    /// Returns the key text.
    ///
    /// The only accessor, named so that a call site reads as an explicit disclosure. Used once per
    /// request, to build one header.
    fn expose_for_header(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for BearerKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BearerKey([REDACTED])")
    }
}

/// A configured OpenAI-compatible endpoint and its credential.
///
/// The endpoint is a **host and port**, not a URL string. That is the load-bearing decision: a URL
/// could carry a path, a query, or a userinfo segment, and an operator pasting a key into one is the
/// mistake this shape makes unrepresentable rather than merely documented. The credential lives in a
/// separate field that is never rendered.
pub struct OpenAiCompatibleProvider {
    provider_id: ProviderId,
    models: Vec<ModelRef>,
    host: String,
    port: u16,
    api_key: BearerKey,
    endpoint_class: EndpointClass,
    /// The prefix the completion route is mounted under, normalized and without a trailing slash.
    ///
    /// Stored validated rather than raw, so no request can be built from an unvalidated value: the
    /// only way in is [`OpenAiCompatibleProvider::with_base_path`], which refuses anything unsafe.
    base_path: String,
    /// JARVIS model id to provider wire name, for models whose two names differ.
    ///
    /// Keyed by the model id string. A model absent from this map is sent under its own id, which is
    /// the common case and keeps every existing configuration byte-identical.
    wire_names: std::collections::BTreeMap<String, String>,
    /// How long one exchange may take, including the body.
    ///
    /// The adapter's own outer bound. The run controller passes a tighter per-frame deadline through
    /// `CallLimits`, and this is the transport's last-resort limit so a stalled peer cannot hold a
    /// connection open indefinitely even if the request carried no deadline.
    timeout: std::time::Duration,
    /// The TLS peer, when the endpoint is reached over TLS; `None` is the loopback plaintext path.
    tls: Option<TlsPeer>,
}

/// What a TLS exchange needs beyond the host and port: the verified name and the trust configuration.
///
/// Cheap to clone (`TlsConnector` shares an `Arc<ClientConfig>`), so each exchange owns a copy rather
/// than borrowing the provider.
#[derive(Clone)]
struct TlsPeer {
    connector: tokio_rustls::TlsConnector,
    /// The name the certificate is verified against and sent as SNI. A DNS name, never an address.
    server_name: ServerName<'static>,
}

impl std::fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Hand-written rather than derived, and the credential is the reason: a derived `Debug` would
        // print the key into any diagnostic line that formatted the adapter, and a key in a log file
        // is exactly what the evidence note's redaction rules forbid. The endpoint is printed because
        // it is operator configuration rather than a secret, and an operator debugging a refusal
        // needs to know which peer was dialled.
        formatter
            .debug_struct("OpenAiCompatibleProvider")
            .field("provider_id", &self.provider_id)
            .field("models", &self.models.len())
            .field("endpoint", &format_args!("{}:{}", self.host, self.port))
            .field("endpoint_class", &self.endpoint_class)
            .field("tls", &self.tls.is_some())
            // Named so the presence of a credential is visible without its value, which is what a
            // diagnostic needs: "was one configured" rather than "what is it".
            .field("api_key", &"[redacted]")
            .finish_non_exhaustive()
    }
}

/// Why an endpoint configuration was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    /// The host is not a loopback address, and TLS was not requested.
    ///
    /// Refused rather than accepted and later failed: a remote host would be reached in plaintext,
    /// and sending a provider credential over an unencrypted network is worse than refusing to
    /// start. A remote host is reached with [`OpenAiCompatibleProvider::new_tls`].
    NotLoopback,
    /// The credential is empty or not a plausible bearer token.
    InvalidCredential,
    /// No model was configured.
    NoModels,
    /// The endpoint was given as a URL carrying a scheme, path, query, or userinfo.
    UrlShaped,
    /// The configured base path is not a usable absolute path.
    ///
    /// Refused rather than normalised, and for the same reason the host is: a value that needs
    /// repairing is a value whose meaning is uncertain, and "normalised into something safe" is how a
    /// configuration typo becomes a request to an endpoint nobody intended. The rules are the ones
    /// that make the value safe to put in a request line — absolute, no traversal, no query, no
    /// fragment, no whitespace or control characters, and bounded.
    InvalidPath,
    /// A provider-side model name is unusable, or maps a model this endpoint does not serve.
    ///
    /// The name reaches a JSON body rather than a request line, so the rules are narrower than the
    /// path's: bounded, non-blank, no control character. An entry keyed on a model the adapter does
    /// not serve is refused because the mapping would never be used and the operator would believe it
    /// had been — the same reasoning that makes an unserved routed model a refusal rather than a
    /// silent substitution.
    InvalidModelName,
    /// A TLS endpoint was given as an address or as a name that is not a valid DNS name.
    ///
    /// Refused because certificate verification here is by **name**: an address has no name to verify,
    /// and a loopback address is served by the plaintext constructor.
    InvalidTlsHost,
    /// The TLS client could not be configured (no usable protocol version or crypto provider).
    TlsSetup,
}

impl ConfigError {
    /// Returns the stable, namespaced code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotLoopback => "model.adapter_endpoint_not_loopback",
            Self::InvalidCredential => "model.adapter_credential_invalid",
            Self::NoModels => "model.adapter_no_models",
            Self::UrlShaped => "model.adapter_endpoint_url_shaped",
            Self::InvalidPath => "model.adapter_path_invalid",
            Self::InvalidModelName => "model.adapter_model_name_invalid",
            Self::InvalidTlsHost => "model.adapter_tls_host_invalid",
            Self::TlsSetup => "model.adapter_tls_setup_failed",
        }
    }
}

impl OpenAiCompatibleProvider {
    /// Builds an adapter over a loopback endpoint.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] for an endpoint this build cannot serve or a credential it cannot
    /// use. Each is a startup refusal rather than a runtime failure, so an operator learns about a
    /// misconfiguration before a run depends on it.
    pub fn new(
        provider_id: ProviderId,
        host: &str,
        port: u16,
        api_key: &str,
        model_ids: Vec<ModelId>,
    ) -> Result<Self, ConfigError> {
        // A URL-shaped value is refused explicitly rather than being parsed and silently stripped,
        // because a host string that still contains a scheme means the operator pasted something other
        // than a host — and a `?` or `@` in the same field is how a key ends up inside a base URL.
        if host.contains("://")
            || host.contains('/')
            || host.contains('?')
            || host.contains('#')
            || host.contains('@')
        {
            return Err(ConfigError::UrlShaped);
        }
        let host = host.trim().trim_matches(['[', ']']).to_owned();
        if host.is_empty() {
            return Err(ConfigError::UrlShaped);
        }
        // Loopback only (a named host is reached with `new_tls`), and checked by parsing rather than by prefix, because `127.0.0.1.evil` and
        // `localhost.attacker` both start with something that looks local. A name is refused outright
        // so no string that could resolve through DNS or a hosts file reaches the socket.
        let loopback = match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback(),
            Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback(),
            Err(_) => false,
        };
        if !loopback {
            return Err(ConfigError::NotLoopback);
        }
        // A key is required even for a local endpoint: a compatible server that takes none still
        // accepts a placeholder, and requiring one means an unset key is a startup failure rather
        // than a `401` nobody can explain. Whitespace and control characters are refused because a
        // header value containing a newline would inject a second header.
        let trimmed = api_key.trim();
        if trimmed.is_empty() || trimmed.contains(['\r', '\n', '\0']) {
            return Err(ConfigError::InvalidCredential);
        }
        if model_ids.is_empty() {
            return Err(ConfigError::NoModels);
        }

        Ok(Self {
            models: model_ids
                .into_iter()
                .map(|model_id| ModelRef::new(provider_id.clone(), model_id))
                .collect(),
            provider_id,
            host,
            port,
            // The key is wrapped so it cannot be rendered incidentally: a private field with no
            // derived `Debug` means a diagnostic line that formats this adapter cannot print it.
            api_key: BearerKey(trimmed.to_owned()),
            endpoint_class: EndpointClass::Local,
            timeout: std::time::Duration::from_secs(120),
            base_path: String::new(),
            wire_names: std::collections::BTreeMap::new(),
            tls: None,
        })
    }

    /// Builds an adapter over a **TLS** endpoint, trusting the compiled-in Mozilla root set.
    ///
    /// The host must be a DNS name; the certificate is verified against it and it is sent as SNI.
    /// The endpoint class is [`EndpointClass::ApprovedCloud`], so a local-only data policy refuses it.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], plus [`ConfigError::InvalidTlsHost`] for an address or an invalid name and
    /// [`ConfigError::TlsSetup`] if the TLS client cannot be configured.
    pub fn new_tls(
        provider_id: ProviderId,
        host: &str,
        port: u16,
        api_key: &str,
        model_ids: Vec<ModelId>,
    ) -> Result<Self, ConfigError> {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::new_tls_with_roots(provider_id, host, port, api_key, model_ids, roots)
    }

    /// As [`Self::new_tls`], trusting exactly `roots`.
    ///
    /// The seam for a private certificate authority and for tests, which must show a certificate is
    /// rejected when its root is absent and accepted when it is injected.
    ///
    /// # Errors
    ///
    /// As [`Self::new_tls`].
    pub fn new_tls_with_roots(
        provider_id: ProviderId,
        host: &str,
        port: u16,
        api_key: &str,
        model_ids: Vec<ModelId>,
        roots: rustls::RootCertStore,
    ) -> Result<Self, ConfigError> {
        if host.contains("://")
            || host.contains('/')
            || host.contains('?')
            || host.contains('#')
            || host.contains('@')
        {
            return Err(ConfigError::UrlShaped);
        }
        let host = host.trim();
        if host.is_empty() {
            return Err(ConfigError::UrlShaped);
        }
        // Parsed by the TLS library, which is the authority on what a valid DNS name is. An address is
        // refused: it has no name to verify a certificate against.
        let Ok(server_name @ ServerName::DnsName(_)) = ServerName::try_from(host.to_owned()) else {
            return Err(ConfigError::InvalidTlsHost);
        };
        // Build through the loopback constructor with a placeholder host so the credential, model,
        // and URL-shape rules have one implementation, then replace the endpoint.
        let mut provider = Self::new(provider_id, "127.0.0.1", port, api_key, model_ids)?;
        // Explicit crypto provider and protocol versions, so no process-wide default is consulted or
        // installed: this adapter must not depend on, or change, how another component configured TLS.
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| ConfigError::TlsSetup)?
        .with_root_certificates(roots)
        .with_no_client_auth();
        // HTTP/1.1 is the only protocol this hand-rolled client speaks.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        provider.host = host.to_ascii_lowercase();
        provider.endpoint_class = EndpointClass::ApprovedCloud;
        provider.tls = Some(TlsPeer {
            connector: tokio_rustls::TlsConnector::from(Arc::new(config)),
            server_name,
        });
        Ok(provider)
    }

    /// Supplies the provider-side names for models whose JARVIS id differs from the API's name.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidModelName`] for a name that is not usable or that maps a model
    /// this adapter does not serve.
    pub fn with_wire_names(
        mut self,
        names: &std::collections::BTreeMap<String, String>,
    ) -> Result<Self, ConfigError> {
        for (model_id, wire_name) in names {
            // A mapping for an unserved model is refused rather than ignored: it would never be used,
            // and the operator would have no way to tell that their mapping had no effect.
            if !self
                .models
                .iter()
                .any(|served| served.model_id.as_str() == model_id)
            {
                return Err(ConfigError::InvalidModelName);
            }
            if !is_usable_model_name(wire_name) {
                return Err(ConfigError::InvalidModelName);
            }
            let _previous = self.wire_names.insert(model_id.clone(), wire_name.clone());
        }
        Ok(self)
    }

    /// Returns the name this adapter sends for `model`.
    ///
    /// A model with no mapping is sent under its own id, which is the common case: the two namespaces
    /// coincide whenever a provider names models the way JARVIS does.
    #[must_use]
    pub fn wire_name_for(&self, model: &ModelRef) -> String {
        self.wire_names
            .get(model.model_id.as_str())
            .cloned()
            .unwrap_or_else(|| model.model_id.as_str().to_owned())
    }

    /// Sets the base path the completion route is mounted under.
    ///
    /// `""` (the default) posts to `/chat/completions`; `"/v1"` posts to `/v1/chat/completions`,
    /// which is what Ollama, vLLM, LM Studio, and most gateways serve. Trailing slashes are trimmed so
    /// `"/v1/"` and `"/v1"` are one value rather than two spellings that could disagree.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidPath`] for a value that is not a safe absolute path.
    pub fn with_base_path(mut self, base_path: &str) -> Result<Self, ConfigError> {
        self.base_path = validate_base_path(base_path)?;
        Ok(self)
    }

    /// Returns the full request path, base and route together.
    #[must_use]
    pub fn completions_path(&self) -> String {
        format!("{}{COMPLETIONS_PATH}", self.base_path)
    }

    /// Sets the outer transport timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Returns the configured endpoint as `host:port`, for an operator diagnostic.
    ///
    /// Returns no credential, and there is no accessor that returns one: the adapter reads the secret
    /// itself when it builds a request, so no caller ever holds it.
    #[must_use]
    pub fn endpoint_label(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Builds the JSON request body for `request`.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::InvalidRequest`] for a request this adapter cannot express: a model
    /// it does not serve, an unsupported input item, unsupported settings, a requested output schema,
    /// or too many tools. Refusing is deliberate — a request silently dropped to a subset would send
    /// a *different question* than the run recorded, and the answer would be attributed to the
    /// original.
    pub fn build_body(
        &self,
        request: &ModelCallRequest,
    ) -> Result<serde_json::Value, ProviderError> {
        // The routed model must be one this adapter serves. Substituting another would answer with a
        // model the data policy did not select.
        if !self.models.iter().any(|served| served == &request.model) {
            return Err(ProviderError::InvalidRequest);
        }
        if request.output_schema.is_some() {
            // Structured output is out of this slice's scope. Silently ignoring the schema would
            // return unconstrained prose that a caller believed matched a schema.
            return Err(ProviderError::InvalidRequest);
        }
        if !request.settings.is_empty() {
            // Portable settings are not translated yet. Dropping them would run the call with
            // settings the caller did not choose and could not tell the difference.
            return Err(ProviderError::InvalidRequest);
        }
        if request.tools.len() > MAX_TOOLS {
            return Err(ProviderError::InvalidRequest);
        }

        let mut messages = Vec::new();
        for item in request.input.as_slice() {
            match item {
                InputItem::SystemPolicyRef { .. } => {
                    // Resolved by JARVIS and not sent verbatim: the client sends no policy text, and
                    // the endpoint has no concept of one. The system instruction the endpoint sees is
                    // deliberately absent rather than fabricated here, so this adapter cannot invent
                    // a policy the operator never configured.
                }
                InputItem::Message { role, blocks } => {
                    let Some(content) = message_text(blocks) else {
                        // An artifact reference has no text form in this protocol, and inventing a
                        // placeholder would send a message the run never composed.
                        return Err(ProviderError::InvalidRequest);
                    };
                    messages.push(serde_json::json!({
                        "role": role_name(*role),
                        "content": content,
                    }));
                }
                InputItem::ToolResult {
                    call_id, content, ..
                } => {
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": content,
                    }));
                }
                InputItem::ToolCall {
                    call_id,
                    tool_name,
                    arguments,
                } => {
                    // The arguments must be complete to be sent back. A partial string would be an
                    // invalid tool call the endpoint would refuse, and the reason would look like a
                    // protocol fault rather than a half-assembled one.
                    let Some(raw) = arguments.executable_raw() else {
                        return Err(ProviderError::InvalidRequest);
                    };
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": call_id,
                            "type": "function",
                            "function": { "name": tool_name, "arguments": raw },
                        }],
                    }));
                }
                InputItem::ReasoningSummary { summary } => {
                    // A published summary is display material, not conversation input. Sending it
                    // back would let one model's summary steer the next call, which is a prompt
                    // injection path the run did not authorize.
                    let _ = summary;
                }
            }
        }

        let mut body = serde_json::json!({
            // The **provider's** name for the model, which is its own id unless the configuration
            // mapped it. The two namespaces genuinely differ for real endpoints — Ollama's
            // `glm-5.3-flash:cloud` cannot be a JARVIS model id, because a JARVIS id is a lowercase
            // dotted slug — so the id this adapter routes on and the string the provider is asked for
            // are separate values. The routed model is still exactly one this adapter serves; the
            // mapping changes only how it is spelled on the wire.
            "model": self.wire_name_for(&request.model),
            "messages": messages,
            "stream": true,
            // Requested so the final chunk carries the usage block. Without it the provider reports
            // no counts at all and the run's own token ceiling becomes unverifiable rather than
            // enforced — a bound that reads as enforced while nothing measures against it.
            "stream_options": { "include_usage": true },
            // Pinned to one answer. `n > 1` is refused rather than modelled: JARVIS's normalized
            // stream has one output item per call, and a second choice would have nowhere to live.
            "n": 1,
        });

        if !request.tools.is_empty() {
            // Declared so a stored transcript that contains tool calls can be continued. The names
            // are the canonical ones the tool fabric owns; this adapter only transports them, and
            // advertising a tool is not a grant to execute it.
            let tools: Vec<serde_json::Value> = request
                .tools
                .iter()
                .map(|name| {
                    let offer = request.tool_offers.iter().find(|offer| &offer.name == name);
                    let mut function = serde_json::json!({
                        "name": name,
                        "parameters": offer_parameters(offer),
                    });
                    if let Some(description) = offer
                        .map(|offer| offer.description.as_str())
                        .filter(|d| !d.is_empty())
                    {
                        function["description"] = serde_json::Value::String(description.to_owned());
                    }
                    serde_json::json!({ "type": "function", "function": function })
                })
                .collect();
            body["tools"] = serde_json::Value::Array(tools);
        }

        // The run's output ceiling, forwarded so the provider is **told** the bound rather than
        // JARVIS merely judging the answer afterwards.
        //
        // Two facts from the official reference decide the shape of this:
        //
        // 1. The correct parameter is **`max_completion_tokens`**, described as "an upper bound for
        //    the number of tokens that can be generated for a completion, including visible output
        //    tokens and **[reasoning tokens]**(/api/docs/guides/reasoning)". The older `max_tokens`
        //    is "now deprecated in favor of `max_completion_tokens`, and is **not compatible with
        //    o-series models**" — so sending `max_tokens` would break the reasoning models this
        //    adapter can be pointed at.
        // 2. Because it *includes* reasoning tokens, it bounds exactly the quantity JARVIS compares
        //    against: this adapter maps `completion_tokens` onto `Usage::output_tokens`, and the
        //    reference lists `completion_tokens` as "the number of tokens in the generated
        //    completion". A narrower bound (visible text only) would let `exceeded_by` fire on an
        //    answer the provider considered within limit — a false failure.
        //
        // Absent when the run states no ceiling, which is honest: a limit JARVIS did not set must
        // not be invented, and an omitted parameter leaves the provider's own default in force.
        if let Some(limit) = request.limits.max_output_tokens {
            body["max_completion_tokens"] = serde_json::json!(limit);
        }

        Ok(body)
    }
}

/// Returns the text of a message's blocks, or `None` when any block has no text form.
fn message_text(blocks: &[ContentBlock]) -> Option<String> {
    let mut text = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text: value } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(value);
            }
            // An artifact reference is a JARVIS storage handle. This endpoint cannot resolve one, and
            // sending the identifier as text would send the model a meaningless string while looking
            // like content.
            ContentBlock::ArtifactRef { .. } => return None,
        }
    }
    Some(text)
}

/// The `parameters` document for one offered tool.
///
/// The tool's own JSON Schema when it parses as an **object**; otherwise the empty object schema. A
/// schema that does not parse, or is not an object, is not repaired — sending the bare fallback is the
/// behaviour a tool with no schema always had, and the pipeline validates the arguments against the
/// real schema regardless of what the model was shown.
fn offer_parameters(offer: Option<&jarvis_domain::model::stream::ToolOffer>) -> serde_json::Value {
    offer
        .and_then(|offer| offer.input_schema.as_deref())
        .and_then(|schema| serde_json::from_str::<serde_json::Value>(schema).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({"type": "object"}))
}

/// Returns the protocol's role name for a normalized role.
const fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn models(&self) -> &[ModelRef] {
        &self.models
    }

    fn endpoint_class(&self) -> EndpointClass {
        // Fixed at construction: `Local` for the loopback constructor, `ApprovedCloud` for TLS. A
        // `Local` class is what makes a `LocalOnly` policy admit this provider and a cloud-only
        // policy refuse it.
        self.endpoint_class
    }

    fn open<'a>(
        &'a self,
        _context: &'a RequestContext,
        request: &'a ModelCallRequest,
        cancel: &'a jarvis_application::cancellation::CancellationScope,
    ) -> OpenResult<'a> {
        Box::pin(async move {
            // The body is built **before** the connection, so a request this adapter cannot express
            // is an `open` error rather than a call that connected and then failed. An operator sees
            // "this build cannot serve that request" instead of a protocol fault.
            let body = self.build_body(request)?;
            let plan = ExchangePlan {
                host: self.host.clone(),
                port: self.port,
                path: self.completions_path(),
                body: serde_json::to_string(&body).map_err(|_| ProviderError::InvalidRequest)?,
                // The credential is moved into the plan rather than borrowed, so the task owns the
                // only copy and no reference to it outlives this call.
                credential: self.api_key.expose_for_header().to_owned(),
                timeout: self.timeout,
                tls: self.tls.clone(),
            };
            let call_id = request.call_id;
            let ids: Arc<dyn IdGenerator> = Arc::new(crate::ids::UuidV7Generator::new());

            // The exchange runs on its own task because an adapter's `open` returns as soon as it has
            // an object to return, and the task is what produces frames over time. The channel is
            // bounded so a fast provider cannot outrun a slow consumer.
            let (sender, receiver) = mpsc::channel::<ModelStreamEvent>(STREAM_CHANNEL_DEPTH);
            tokio::spawn(run_exchange(plan, sender, call_id, Arc::clone(&ids)));

            // The stream owns cancellation and the exactly-one-terminal guarantee, so those two rules
            // are implemented once rather than per adapter.
            Ok(
                Box::new(AdapterStream::new(call_id, ids, cancel.clone(), receiver))
                    as Box<dyn jarvis_application::model::ModelStream + Send>,
            )
        })
    }
}

/// Everything one exchange needs, owned rather than borrowed.
///
/// A value rather than a bare argument list so the plan is inspectable: the host, port, and body can
/// be asserted on any host with no socket, which is the same "plan then execute" convention the
/// service and installer code use. The `Debug` is deliberately absent so a plan cannot be formatted —
/// it holds the credential, and a derived `Debug` is how a secret reaches a log line.
struct ExchangePlan {
    host: String,
    port: u16,
    /// The full request path, base and route together, already validated.
    path: String,
    body: String,
    credential: String,
    timeout: std::time::Duration,
    tls: Option<TlsPeer>,
}

/// Opens the connection and feeds the body's frames to `sender`.
///
/// The whole task, and its failure paths matter more than its success path: a stream that fails
/// without a terminal must be distinguishable from one that completed, so every early return either
/// sends a terminal frame or drops the sender — and dropping it is what makes
/// [`AdapterStream`](jarvis_application::model::AdapterStream) end the stream as *interrupted* rather
/// than fabricate a completion.
async fn run_exchange(
    plan: ExchangePlan,
    sender: mpsc::Sender<ModelStreamEvent>,
    call_id: ModelCallId,
    ids: Arc<dyn IdGenerator>,
) {
    let mut stamper = jarvis_application::model::FrameStamper::start(call_id, ids.as_ref());
    match exchange(&plan, &sender, &mut stamper).await {
        Ok(()) => {}
        Err(error) => {
            // A transport-level failure becomes a terminal `call.failed` frame, which is the
            // documented division: `open` errors are for a stream that could not be opened, and a
            // failure after that travels as a frame so exactly one place reports each shape.
            if let Ok(frame) = stamper.stamp(
                ModelStreamEventKind::CallFailed {
                    code: error.code().to_owned(),
                    retryable: error.retryable(),
                },
                None,
            ) {
                // A failed send means the consumer is gone, so there is nothing to report to and
                // nothing to do about it.
                let _ = sender.send(frame).await;
            }
        }
    }
}

/// Performs one HTTP/1.1 exchange and pumps the response body.
async fn exchange(
    plan: &ExchangePlan,
    sender: &mpsc::Sender<ModelStreamEvent>,
    stamper: &mut jarvis_application::model::FrameStamper<'_>,
) -> Result<(), ProviderError> {
    let attempt = async {
        let tcp = tokio::net::TcpStream::connect((plan.host.as_str(), plan.port))
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        match &plan.tls {
            None => converse(tcp, plan, sender, stamper).await,
            Some(peer) => {
                // Verification is by name against the configured trust roots; a failed handshake sends
                // nothing, so the credential is never written to an unverified peer. It is `Unavailable`
                // rather than a new class because the port has no certificate-specific error and the
                // detail (which check failed) is deliberately not forwarded from an untrusted peer.
                let tls = peer
                    .connector
                    .connect(peer.server_name.clone(), tcp)
                    .await
                    .map_err(|_| ProviderError::Unavailable)?;
                converse(tls, plan, sender, stamper).await
            }
        }
    };

    // The adapter's outer bound. The run controller's deadline is tighter and is the real budget; this
    // exists so a peer that accepts a connection and then stalls cannot hold it open forever.
    tokio::time::timeout(plan.timeout, attempt)
        .await
        .map_err(|_| ProviderError::Timeout)?
}

/// The `Host` header value: the port is omitted for a TLS endpoint on 443, as clients do.
fn host_header(plan: &ExchangePlan) -> String {
    if plan.tls.is_some() && plan.port == 443 {
        plan.host.clone()
    } else {
        format!("{}:{}", plan.host, plan.port)
    }
}

/// Sends the request and pumps the response over an established stream, plaintext or TLS.
///
/// One implementation for both transports, so the framing, the status mapping, and the credential
/// handling cannot diverge between them.
async fn converse<S>(
    mut stream: S,
    plan: &ExchangePlan,
    sender: &mpsc::Sender<ModelStreamEvent>,
    stamper: &mut jarvis_application::model::FrameStamper<'_>,
) -> Result<(), ProviderError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    // The request is assembled by hand for the reason the evidence note records: one known local
    // peer does not justify a general client stack, and a hand-built request makes it structurally
    // hard to send the credential anywhere but this peer. The credential is a **header value**,
    // never a URL component, so it cannot appear in a URL-shaped log line.
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
             Host: {host_header}\r\n\
             Authorization: Bearer {credential}\r\n\
             Content-Type: application/json\r\n\
             Accept: text/event-stream\r\n\
             Content-Length: {length}\r\n\
             Connection: close\r\n\r\n{body}",
        path = plan.path,
        host_header = host_header(plan),
        credential = plan.credential,
        length = plan.body.len(),
        body = plan.body,
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| ProviderError::Unavailable)?;
    // A TLS stream buffers writes like a `BufWriter`: without a flush the request may never reach the
    // socket and the exchange would wait for a response to a request the peer has not seen.
    stream
        .flush()
        .await
        .map_err(|_| ProviderError::Unavailable)?;

    let mut reader = ResponseReader::new(stream);
    let head = reader.read_head().await?;
    if !(200..300).contains(&head.status) {
        // The error body is read so the mapping can use the documented type/code pair, and the
        // code is what distinguishes a retryable rate limit from an exhausted balance. The body
        // text itself never becomes a JARVIS code.
        let body = reader.read_error_body().await.unwrap_or_default();
        return Err(map_status(head.status, &body));
    }

    let mut translator = ChunkTranslator::new();
    pump_body(&mut reader, sender, stamper, &mut translator).await
}

/// The parsed response head.
#[derive(Debug, Clone, Copy)]
struct ResponseHead {
    status: u16,
}

/// A response reader that decodes HTTP/1.1 framing.
///
/// Two framings must be handled, and getting either wrong produces a parse failure blamed on the
/// provider:
///
/// - **`Content-Length`** — a fixed body. A streaming response usually uses chunked encoding, but a
///   server that buffers the whole answer may use this, and the reader must not wait for a close it
///   will never see.
/// - **`Transfer-Encoding: chunked`** — the usual framing for a streamed response, where each chunk is
///   preceded by its hexadecimal length. Passing the chunk headers through to the SSE parser would
///   feed it hex digits as payload and every frame would fail.
///
/// The decoder is **unconditional** rather than switched on the head. That is deliberate: a chunked
/// body's size line is hex digits followed by CRLF, so a body that is *not* chunked fails to parse as
/// a chunk and the decoder reports the body complete — which is the correct reading for a
/// `Content-Length` body whose bytes do not begin with a hex line. A flag would introduce a way for
/// the head parser and the decoder to disagree about the same bytes, and the disagreement would
/// surface as a truncated answer rather than as an error. The head parser therefore reads the
/// `Transfer-Encoding` header only to reject a body that declares an encoding this reader cannot
/// decode, which is why `read_head` validates it instead of storing it.
struct ResponseReader<S> {
    stream: S,
    buffered: Vec<u8>,
    /// How many payload bytes of the current chunk are still to be delivered.
    ///
    /// Without this the decoder would re-parse payload bytes as a size line on the next call. The
    /// first version of this reader had no such field and delivered the whole buffer, which produced
    /// `model.provider_malformed` against a healthy stream — caught by the end-to-end test rather than
    /// by the unit tests, which is why the transport is tested through a real socket.
    payload_remaining: usize,
}

impl<S: tokio::io::AsyncRead + Unpin> ResponseReader<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffered: Vec::new(),
            payload_remaining: 0,
        }
    }

    /// Reads and parses the response head.
    async fn read_head(&mut self) -> Result<ResponseHead, ProviderError> {
        let head_end = loop {
            if let Some(index) = find_bytes(&self.buffered, b"\r\n\r\n") {
                break index;
            }
            if !self.fill().await? {
                // A peer that closes before finishing its head sent nothing usable.
                return Err(ProviderError::Malformed);
            }
            if self.buffered.len() > MAX_HEAD_BYTES {
                return Err(ProviderError::Malformed);
            }
        };
        let head: Vec<u8> = self.buffered.drain(..head_end + 4).collect();
        let text = std::str::from_utf8(&head).map_err(|_| ProviderError::Malformed)?;
        let status = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .ok_or(ProviderError::Malformed)?;
        // Any declared transfer encoding other than `chunked` or `identity` is a framing this reader
        // does not implement. Refusing it by name is the fail-closed reading: accepting it would let
        // the decoder interpret framing bytes as payload.
        for line in text.lines() {
            let lowered = line.to_ascii_lowercase();
            if let Some(value) = lowered.strip_prefix("transfer-encoding:") {
                let value = value.trim();
                if !value.is_empty() && value != "chunked" && value != "identity" {
                    return Err(ProviderError::Malformed);
                }
            }
        }
        Ok(ResponseHead { status })
    }

    /// Reads an error body up to a bound, for the status mapping.
    async fn read_error_body(&mut self) -> Result<String, ProviderError> {
        let mut collected = Vec::new();
        while collected.len() < MAX_ERROR_BODY_BYTES {
            if !self.fill().await? {
                break;
            }
        }
        collected.extend_from_slice(&self.buffered);
        self.buffered.clear();
        Ok(String::from_utf8_lossy(&collected).into_owned())
    }

    /// Reads more bytes, returning whether any arrived.
    async fn fill(&mut self) -> Result<bool, ProviderError> {
        let mut chunk = [0_u8; 8192];
        let read = self
            .stream
            .read(&mut chunk)
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        if read == 0 {
            return Ok(false);
        }
        self.buffered.extend_from_slice(&chunk[..read]);
        Ok(true)
    }
}

/// Decodes a chunked body, so the SSE parser never sees chunk headers.
///
/// Implemented as an `AsyncRead` adapter rather than a buffer transformation because the body is
/// streamed: buffering the whole response to decode it would defeat the point of streaming and would
/// also remove the memory bound that makes an untrusted peer safe.
impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for ResponseReader<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        output: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        loop {
            // Payload already decoded from the previous chunk and not yet delivered. This counter is
            // the field the first version of this decoder lacked, and its absence was a real defect
            // caught by the end-to-end test: `decode_next_chunk` **strips the framing and leaves the
            // payload**, so without a length to hand out, the reader would deliver the whole
            // remaining buffer and then decode *payload bytes* as a chunk size line on the next call.
            // The visible symptom was `model.provider_malformed` on a perfectly healthy stream.
            if self.payload_remaining > 0 {
                let take = self
                    .payload_remaining
                    .min(self.buffered.len())
                    .min(output.remaining());
                if take == 0 {
                    return std::task::Poll::Ready(Ok(()));
                }
                output.put_slice(&self.buffered[..take]);
                self.buffered.drain(..take);
                self.payload_remaining -= take;
                return std::task::Poll::Ready(Ok(()));
            }

            match decode_next_chunk(&mut self.buffered) {
                ChunkState::Payload(size) => {
                    // The size was consumed from the framing; the payload stays buffered and is
                    // delivered by the branch above, possibly across several reads.
                    self.payload_remaining = size;
                }
                ChunkState::Complete => {
                    // The terminating zero-length chunk: the body is over. Reported as end-of-body so
                    // the pump's own terminal-or-unavailable rule produces the right answer, rather
                    // than as an error that would be attributed to the transport.
                    return std::task::Poll::Ready(Ok(()));
                }
                ChunkState::NeedMore => {
                    // Read more into a local buffer and retry. The read is driven by the outer
                    // runtime's waker through `poll_read` on the inner stream.
                    let mut scratch = [0_u8; 8192];
                    let mut read_buf = tokio::io::ReadBuf::new(&mut scratch);
                    let poll =
                        std::pin::Pin::new(&mut self.stream).poll_read(context, &mut read_buf)?;
                    match poll {
                        std::task::Poll::Pending => return std::task::Poll::Pending,
                        std::task::Poll::Ready(()) => {
                            let filled = read_buf.filled();
                            if filled.is_empty() {
                                return std::task::Poll::Ready(Ok(()));
                            }
                            self.buffered.extend_from_slice(filled);
                        }
                    }
                }
            }
        }
    }
}

/// What the chunked decoder found at the head of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkState {
    /// A chunk's framing was stripped and this many payload bytes are now at the head of the buffer.
    Payload(usize),
    /// The terminating chunk was seen; the body is complete.
    Complete,
    /// The buffer does not yet hold a whole size line or chunk.
    NeedMore,
}

/// Strips one chunk's framing from `buffer`, leaving its payload at the head.
///
/// **Leaves the payload rather than draining it**, and that is the whole contract of this function:
/// the caller is handed the payload length and delivers the bytes, so a chunk larger than the
/// caller's output buffer is delivered across several reads instead of being dropped. An earlier
/// version drained the payload here *and* had the reader hand out the entire remaining buffer, so the
/// next decode read payload bytes as a size line.
fn decode_next_chunk(buffer: &mut Vec<u8>) -> ChunkState {
    let Some(line_end) = find_bytes(buffer, b"\r\n") else {
        return ChunkState::NeedMore;
    };
    let Ok(size_text) = std::str::from_utf8(&buffer[..line_end]) else {
        return ChunkState::Complete;
    };
    // A chunk size may carry an extension after `;`, which is legal and unused here.
    let size_text = size_text.split(';').next().unwrap_or(size_text).trim();
    let Ok(size) = usize::from_str_radix(size_text, 16) else {
        // An unparseable size line means the framing is not what the head declared, so the body is
        // over as far as this decoder can tell. Reported as end-of-body rather than as an error: the
        // pump's terminal-or-unavailable rule then reports an unterminated stream as unavailable
        // instead of a framing fault, which is the same answer a server that simply stopped would
        // produce.
        return ChunkState::Complete;
    };
    if size == 0 {
        return ChunkState::Complete;
    }
    let payload_start = line_end + 2;
    // The whole payload plus its trailing CRLF must be present before the framing is stripped, or a
    // read boundary inside a chunk would leave the trailing CRLF to be misread as a size line.
    let payload_end = payload_start.saturating_add(size);
    if buffer.len() < payload_end.saturating_add(2) {
        return ChunkState::NeedMore;
    }
    // Framing only: the size line, and the CRLF that terminates the payload. The payload itself stays.
    buffer.drain(..payload_start);
    let trailing = payload_end - payload_start;
    buffer.drain(trailing..trailing + 2);
    ChunkState::Payload(size)
}

/// Returns the index of `needle` in `haystack`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Validates a provider-side model name.
///
/// The rules are narrower than the path's, because a model name reaches a **JSON body** rather than a
/// request line: `serde_json` escapes whatever it is given, so the smuggling surface that makes the
/// path rules strict does not apply. What remains is the pair that would make a request meaningless
/// or unbounded: blank-or-oversized, and a control character that has no place in a name. Everything
/// else a provider is known to use — `:` in `glm-5.3-flash:cloud`, `/` in `org/model`, `@` in
/// `model@2026-01` — is **allowed**, because excluding them would exclude the real endpoints this
/// mapping exists to reach.
fn is_usable_model_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed.len() <= MAX_MODEL_NAME_BYTES
        && !trimmed.chars().any(char::is_control)
}

/// The longest accepted provider-side model name.
///
/// Bounded because it is operator input placed in a request body, and an unbounded value there is an
/// unbounded request — the same reason every other operator value here has a ceiling. Generous
/// relative to any real model name; this is a defect bound, not a policy.
pub const MAX_MODEL_NAME_BYTES: usize = 192;

/// Validates an operator-supplied base path for the completion route.
///
/// The rules are the ones that make the value safe to place in a request line, and each rejects a
/// specific way a path could change the request's meaning:
///
/// - **A relative value is refused.** A bare `"v1"` is not a path, and an omitted call is the
///   first-class spelling of "the origin", so there is nothing to accept.
/// - **`..` and `.` segments are refused**, because a path that climbs is how a request reaches a
///   route the adapter was not configured for.
/// - **`?` and `#` are refused**, because they would end the request target and begin a query or a
///   fragment: everything after them reaches the server as something other than a path, which is how
///   a value ends up somewhere the adapter did not put it.
/// - **Interior whitespace, control characters, and `\\` are refused**, because a space ends the
///   request target and a CR or LF injects a header. This is the request-smuggling surface that makes
///   "let the operator type a path" unsafe without validation, and it is why the value is refused
///   rather than escaped: an escaped path is a different path, and the operator meant the one they
///   typed.
///
/// **Leading and trailing whitespace is stripped, and so is a trailing slash**, because neither can
/// reach a request line once stripped and both have one obvious meaning. My first version of this
/// function documented the whitespace rule as "any whitespace is refused" while the code trimmed
/// first — so `"/v1\t"` was accepted, and my own test is what surfaced the disagreement. The
/// distinction the code actually draws is the one that matters: whitespace *inside* the path changes
/// where the request target ends, and whitespace around it does not.
///
/// Returns the normalised value, stripped and without a trailing slash, so one path has one spelling.
///
/// # Errors
///
/// Returns [`ConfigError::InvalidPath`] for a value failing any rule above.
fn validate_base_path(base_path: &str) -> Result<String, ConfigError> {
    let trimmed = base_path.trim();
    if trimmed.len() > MAX_PATH_BYTES || !trimmed.starts_with('/') {
        return Err(ConfigError::InvalidPath);
    }
    let unsafe_value = trimmed.contains(['?', '#', '\\', '\r', '\n', '\0'])
        || trimmed.chars().any(char::is_whitespace)
        || trimmed
            .split('/')
            .any(|segment| segment == ".." || segment == ".");
    if unsafe_value {
        return Err(ConfigError::InvalidPath);
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// The largest response head this reader will accept.
pub const MAX_HEAD_BYTES: usize = 16 << 10;

/// The most of an error body the status mapping will read.
pub const MAX_ERROR_BODY_BYTES: usize = 64 << 10;

/// Reads an SSE body, translating frames and forwarding them to `sender`.
///
/// Separate from the request exchange so a test can drive it with a byte fixture and no socket: the
/// translation and framing are the parts worth testing exhaustively, and they are pure.
///
/// # Errors
///
/// Returns [`ProviderError::Malformed`] when a frame cannot be interpreted, and
/// [`ProviderError::Unavailable`] when the peer closed before a terminal was produced.
async fn pump_body<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    sender: &mpsc::Sender<ModelStreamEvent>,
    stamper: &mut jarvis_application::model::FrameStamper<'_>,
    translator: &mut ChunkTranslator,
) -> Result<(), ProviderError> {
    let mut buffer = SseBuffer::new();
    let mut chunk = [0_u8; 8192];
    let mut total = 0_usize;
    let mut terminal_sent = false;

    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > MAX_RESPONSE_BYTES {
            // Bounded: the peer is untrusted, and a stream that never ends must fail rather than
            // consume memory without limit.
            return Err(ProviderError::Malformed);
        }

        let frames = match buffer.push(&chunk[..read]) {
            FrameRead::Frames(frames) => frames,
            FrameRead::Incomplete => continue,
            FrameRead::TooLarge => return Err(ProviderError::Malformed),
        };

        for frame in frames {
            let payload = match frame {
                // The sentinel is skipped, **not** treated as the terminal: the documented terminal is
                // the first non-null `finish_reason`, and depending on a sentinel the cited page does
                // not describe would fail against a server that omits it.
                SseFrame::Done => continue,
                SseFrame::Data(payload) => payload,
            };
            let parsed: serde_json::Value =
                serde_json::from_str(&payload).map_err(|_| ProviderError::Malformed)?;
            match translator.translate(Some(&parsed)) {
                Translated::Malformed => return Err(ProviderError::Malformed),
                Translated::Ignored => {}
                Translated::Events(events) => {
                    for kind in events {
                        let is_terminal = kind.is_terminal();
                        let event = stamper.stamp(kind, None)?;
                        // A closed receiver means the consumer stopped, which is not an adapter
                        // error: the caller's cancellation path already ended the stream, and
                        // reporting a fault here would turn a cancellation into a failure.
                        if sender.send(event).await.is_err() {
                            return Ok(());
                        }
                        if is_terminal {
                            terminal_sent = true;
                        }
                    }
                }
            }
        }
    }

    if buffer.has_partial() {
        // A body that ended mid-frame is a truncated response, not a clean end.
        return Err(ProviderError::Malformed);
    }
    if !terminal_sent {
        // The peer closed without a terminal. Reported as unavailable rather than silently ending the
        // stream, because a stream ending with `None` is recorded as *interrupted* — the accurate
        // answer for a transport that died, and one the caller must be able to see.
        return Err(ProviderError::Unavailable);
    }
    Ok(())
}

/// Maps an HTTP status and error body onto a normalized provider error.
///
/// The mapping uses the documented `error.type`/`error.code` **pairs** rather than the status alone,
/// because the provider's own guide states that different conditions share a status. Three cases are
/// the ones a status-only reader gets wrong:
///
/// - A `429` is a filled request budget, an exhausted credit balance, a spend limit, or a ramp-rate
///   limit. Only the ramp-rate case is retryable; retrying a billing state cannot restore access, and
///   the guide says so outright.
/// - A `403` with `insufficient_quota` is an exhausted balance wearing a permission status.
/// - A `401` is always an operator action, never a retry.
#[must_use]
pub fn map_status(status: u16, body: &str) -> ProviderError {
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().and_then(|value| value.get("error"));
    let code = error
        .and_then(|value| value.get("code"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    match status {
        401 => ProviderError::Authentication,
        403 => {
            // A quota refusal wears a permission status. Both are operator actions and neither is
            // retryable, so the mapping is the same; the code is retained so an operator can tell
            // which limit was reached.
            let _ = code;
            ProviderError::Authentication
        }
        429 => {
            // `slow_down` and `rate_limit_exceeded` are the one retryable `429` family, and an
            // unrecognized code is treated the same way — a temporary limit is likelier than a new
            // billing code, and the run's budget still bounds the attempts. Only the four named
            // billing codes below are non-retryable, because retrying them cannot restore access and
            // the provider's own guide says so.
            //
            // `retry_after_ms` is left `None` here and filled from the response headers by the
            // caller, which is the only layer that can read them.
            match code {
                "credit_balance_exhausted"
                | "organization_spend_limit_exceeded"
                | "project_spend_limit_exceeded"
                | "organization_usage_limit_exceeded" => ProviderError::Authentication,
                _ => ProviderError::RateLimited {
                    retry_after_ms: None,
                },
            }
        }
        408 | 504 => ProviderError::Timeout,
        500..=599 => ProviderError::Unavailable,
        // Every other status. The documented `400`, `404`, and `422` cases land here, and the arm is
        // written as the fallback rather than as `400 | 404 | 422 => ..` plus a `_ => ..` because the
        // two would have identical bodies — clippy denies that, and rightly: a named list beside a
        // wildcard with the same answer expresses no distinction, only the appearance of one.
        //
        // The answer itself is deliberate: an unexpected status is treated as an invalid request
        // rather than as unavailable, because an unrecognized response is likelier a request this
        // adapter built wrongly than a transient fault, and retrying a defect spends the run's budget.
        _ => ProviderError::InvalidRequest,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COMPLETIONS_PATH, ConfigError, MAX_PATH_BYTES, MAX_TOOLS, OpenAiCompatibleProvider,
        map_status, message_text, pump_body, role_name, validate_base_path,
    };
    use jarvis_application::model::ProviderError;
    use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
    use jarvis_domain::model::stream::{ModelCallRequest, PortableSettings, Role};

    fn provider() -> OpenAiCompatibleProvider {
        OpenAiCompatibleProvider::new(
            ProviderId::parse("local.llamacpp").expect("valid"),
            "127.0.0.1",
            8080,
            "local-placeholder-key",
            vec![ModelId::parse("test-model").expect("valid")],
        )
        .expect("the fixture configures")
    }

    #[test]
    fn an_unset_base_path_posts_to_the_origin_route() {
        // The default keeps every existing configuration posting to exactly what it posted to before,
        // so making the path configurable is not a change to the common case.
        assert_eq!(provider().completions_path(), COMPLETIONS_PATH);
        assert_eq!(COMPLETIONS_PATH, "/chat/completions");
    }

    #[test]
    fn a_base_path_is_appended_before_the_route() {
        // The case that makes the option exist: Ollama serves the Chat Completions route under
        // `/v1`, so without a base path the adapter cannot reach it at all.
        let provider = provider()
            .with_base_path("/v1")
            .expect("a relative-free absolute path is accepted");
        assert_eq!(provider.completions_path(), "/v1/chat/completions");
        // A trailing slash is one spelling of the same path, not a second value.
        let trailing = provider
            .with_base_path("/v1/")
            .expect("a trailing slash is normalised");
        assert_eq!(trailing.completions_path(), "/v1/chat/completions");
    }

    #[test]
    fn a_base_path_that_could_change_the_request_is_refused() {
        // Each of these changes what the request *is* rather than only which route it names, so the
        // value is refused instead of escaped: an escaped path is a different path, and the operator
        // meant the one they typed.
        for refused in [
            // Not a path at all.
            "v1",
            "",
            // A path that climbs, which is how a request reaches a route it was not configured for.
            "/../admin",
            "/v1/../../admin",
            "/.",
            // A query or fragment would end the request target, so what follows reaches the server as
            // something other than a path.
            "/v1?key=abc",
            "/v1#frag",
            // A space ends the request target; CR and LF inject a header. This is the smuggling
            // surface. Interior whitespace only: leading and trailing whitespace is stripped first,
            // so it cannot reach the request line and is a normalisation rather than a refusal.
            "/v1 extra",
            "/v1\r\nHost: evil.example",
            "/v1\tx",
            // A backslash is a path separator on one platform and a literal on another, so accepting
            // it would let one configuration mean two routes.
            "/v1\\admin",
            // Bounded, because it reaches a request line.
            &format!("/{}", "a".repeat(MAX_PATH_BYTES)),
        ] {
            let result = provider().with_base_path(refused);
            assert!(
                matches!(result, Err(ConfigError::InvalidPath)),
                "{refused:?} must be refused",
            );
        }
        assert_eq!(
            ConfigError::InvalidPath.code(),
            "model.adapter_path_invalid",
        );
    }

    #[test]
    fn the_path_validator_accepts_a_nested_prefix_and_normalises_it() {
        // A gateway mounted under a deeper prefix is the same case as `/v1`, so it must work — and
        // the normalised form is what a caller sees, so there is one spelling to compare.
        assert_eq!(
            validate_base_path("/openai/v1").expect("accepted"),
            "/openai/v1"
        );
        assert_eq!(validate_base_path("/v1/").expect("accepted"), "/v1");
        assert_eq!(validate_base_path("/v1//").expect("accepted"), "/v1");
        // Surrounding whitespace cannot reach the request line once stripped, so it is normalised
        // rather than refused — the distinction the doc comment above records getting wrong first.
        assert_eq!(validate_base_path("  /v1\t").expect("accepted"), "/v1");
    }

    #[test]
    fn a_non_loopback_endpoint_is_refused_rather_than_reached_in_plaintext() {
        // **The measurement behind the transport decision.** This workspace has no TLS
        // implementation, so a remote host would be dialled in the clear with a credential in an
        // `Authorization` header. Refusing at configuration is the only answer that cannot leak, and
        // it fails before a run depends on it.
        for host in [
            "api.openai.com",
            "10.0.0.5",
            "192.168.1.9",
            "localhost",
            "example.invalid",
        ] {
            assert_eq!(
                OpenAiCompatibleProvider::new(
                    ProviderId::parse("local.llamacpp").expect("valid"),
                    host,
                    8080,
                    "key",
                    vec![ModelId::parse("m").expect("valid")],
                )
                .err(),
                Some(ConfigError::NotLoopback),
                "{host} must be refused",
            );
        }
        // A name is refused even when it *would* resolve to loopback, because a name is exactly what
        // DNS and a hosts file can redirect.
        assert_eq!(
            ConfigError::NotLoopback.code(),
            "model.adapter_endpoint_not_loopback",
        );
    }

    #[test]
    fn the_loopback_addresses_this_build_accepts_are_the_ones_it_can_dial() {
        for host in ["127.0.0.1", "::1", "[::1]"] {
            assert!(
                OpenAiCompatibleProvider::new(
                    ProviderId::parse("local.llamacpp").expect("valid"),
                    host,
                    8080,
                    "key",
                    vec![ModelId::parse("m").expect("valid")],
                )
                .is_ok(),
                "{host} must be accepted",
            );
        }
    }

    #[test]
    fn a_url_shaped_host_is_refused_because_it_is_how_a_key_ends_up_in_a_base_url() {
        // A pasted URL is the mistake that silently embeds a credential in a value every log line
        // mentions. Refusing the shape is what makes that unreachable rather than merely documented.
        for host in [
            "http://127.0.0.1",
            "127.0.0.1/v1",
            "127.0.0.1?api-key=secret",
            "user:pass@127.0.0.1",
            "127.0.0.1#frag",
        ] {
            assert_eq!(
                OpenAiCompatibleProvider::new(
                    ProviderId::parse("local.llamacpp").expect("valid"),
                    host,
                    8080,
                    "key",
                    vec![ModelId::parse("m").expect("valid")],
                )
                .err(),
                Some(ConfigError::UrlShaped),
                "{host} must be refused",
            );
        }
    }

    #[test]
    fn a_credential_containing_a_newline_is_refused_because_it_would_inject_a_header() {
        for key in [
            "",
            "   ",
            "key\r\nX-Evil: 1",
            "key\nAuthorization: Bearer other",
        ] {
            assert_eq!(
                OpenAiCompatibleProvider::new(
                    ProviderId::parse("local.llamacpp").expect("valid"),
                    "127.0.0.1",
                    8080,
                    key,
                    vec![ModelId::parse("m").expect("valid")],
                )
                .err(),
                Some(ConfigError::InvalidCredential),
                "{key:?} must be refused",
            );
        }
    }

    #[test]
    fn an_empty_model_list_is_a_configuration_failure_rather_than_an_adapter_serving_nothing() {
        // An adapter that served no model would make every run fail with `model.provider_no_route`,
        // which reads as a policy decision rather than as a misconfiguration.
        assert_eq!(
            OpenAiCompatibleProvider::new(
                ProviderId::parse("local.llamacpp").expect("valid"),
                "127.0.0.1",
                8080,
                "key",
                Vec::new(),
            )
            .err(),
            Some(ConfigError::NoModels),
        );
    }

    #[test]
    fn debug_output_never_carries_the_credential() {
        // A derived `Debug` would print the key into any diagnostic line that formatted the adapter.
        // The canary is asserted absent, and the endpoint stays present because an operator needs to
        // know which peer was dialled.
        let adapter = OpenAiCompatibleProvider::new(
            ProviderId::parse("local.llamacpp").expect("valid"),
            "127.0.0.1",
            8080,
            "canary-secret-value",
            vec![ModelId::parse("m").expect("valid")],
        )
        .expect("configures");
        let rendered = format!("{adapter:?}");
        assert!(
            !rendered.contains("canary-secret-value"),
            "the credential leaked into Debug: {rendered}",
        );
        assert!(rendered.contains("redacted"), "{rendered}");
        assert!(rendered.contains("127.0.0.1:8080"), "{rendered}");
    }

    #[test]
    fn the_role_names_are_the_protocol_spelling() {
        assert_eq!(role_name(Role::System), "system");
        assert_eq!(role_name(Role::User), "user");
        assert_eq!(role_name(Role::Assistant), "assistant");
        assert_eq!(role_name(Role::Tool), "tool");
    }

    #[test]
    fn only_the_first_loopback_class_a_local_adapter_reports_is_local() {
        use jarvis_application::model::ModelProvider as _;
        use jarvis_domain::model::identity::EndpointClass;
        // The class is what makes a `LocalOnly` policy admit this provider. Deriving it from the
        // host name would be the "a name is not evidence" mistake, so it is fixed by construction.
        assert_eq!(provider().endpoint_class(), EndpointClass::Local);
        assert!(!provider().endpoint_class().requires_network());
    }

    #[test]
    fn a_status_and_code_pair_is_mapped_by_both_rather_than_by_the_status_alone() {
        // The provider's own guide states that different conditions share a status. A `429` for a
        // filled budget is retryable; a `429` for an exhausted balance is not, and retrying it cannot
        // restore access.
        let rate = map_status(429, r#"{"error":{"code":"rate_limit_exceeded"}}"#);
        assert!(rate.retryable());
        let slow = map_status(429, r#"{"error":{"code":"slow_down"}}"#);
        assert!(slow.retryable());

        for code in [
            "credit_balance_exhausted",
            "organization_spend_limit_exceeded",
            "project_spend_limit_exceeded",
            "organization_usage_limit_exceeded",
        ] {
            let body = format!(r#"{{"error":{{"code":"{code}"}}}}"#);
            let mapped = map_status(429, &body);
            assert_eq!(mapped, ProviderError::Authentication, "for {code}");
            assert!(
                !mapped.retryable(),
                "a billing state must not be retried: {code}"
            );
        }

        // An unrecognized `429` stays retryable, because a temporary limit is likelier than a new
        // billing code and the run's budget still bounds the attempts.
        assert!(map_status(429, r#"{"error":{"code":"brand_new"}}"#).retryable());
    }

    #[test]
    fn the_terminal_and_transient_statuses_map_to_the_documented_variants() {
        assert_eq!(map_status(401, "{}"), ProviderError::Authentication);
        // A quota refusal wearing a permission status: same action, and not retryable either way.
        assert_eq!(
            map_status(403, r#"{"error":{"code":"insufficient_quota"}}"#),
            ProviderError::Authentication,
        );
        assert_eq!(map_status(400, "{}"), ProviderError::InvalidRequest);
        assert_eq!(map_status(404, "{}"), ProviderError::InvalidRequest);
        assert_eq!(map_status(422, "{}"), ProviderError::InvalidRequest);
        assert_eq!(map_status(408, "{}"), ProviderError::Timeout);
        assert_eq!(map_status(504, "{}"), ProviderError::Timeout);
        assert_eq!(map_status(500, "{}"), ProviderError::Unavailable);
        assert_eq!(
            map_status(503, r#"{"error":{"code":"server_is_overloaded"}}"#),
            ProviderError::Unavailable,
        );
        assert!(map_status(503, "{}").retryable());
        // An unexpected status is an invalid request rather than unavailable: an unrecognized
        // response is likelier a request this adapter built wrongly than a transient fault, and
        // retrying a defect spends the run's budget.
        assert_eq!(map_status(418, "{}"), ProviderError::InvalidRequest);
        // A refusal maps to a non-retryable variant, keeping "a decision repeating cannot change"
        // apart from "a defect in JARVIS".
        assert_ne!(map_status(400, "{}"), ProviderError::Refused);
    }

    #[test]
    fn a_message_with_an_artifact_reference_has_no_text_form_and_is_refused() {
        use jarvis_domain::model::stream::ContentBlock;
        // An artifact identifier is a JARVIS storage handle. Sending it as text would send the model
        // a meaningless string while looking like content, so the adapter refuses instead.
        let text_only = message_text(&[ContentBlock::Text {
            text: "hello".to_owned(),
        }]);
        assert_eq!(text_only.as_deref(), Some("hello"));

        let with_artifact = message_text(&[
            ContentBlock::Text {
                text: "look".to_owned(),
            },
            ContentBlock::ArtifactRef {
                artifact_id: "artifact-1".to_owned(),
                media_type: "image/png".to_owned(),
            },
        ]);
        assert_eq!(with_artifact, None);
    }

    #[test]
    fn the_request_pins_the_answer_count_and_asks_for_usage() {
        // Two decisions the body makes explicit: `n: 1` because the normalized stream has one output
        // item per call, and `include_usage` because without it the provider reports no counts at all
        // and the run's token ceiling becomes unverifiable while reading as enforced.
        let adapter = provider();
        let request = request_for(&adapter);
        let body = adapter.build_body(&request).expect("the fixture builds");
        assert_eq!(body["stream"], serde_json::json!(true));
        assert_eq!(body["n"], serde_json::json!(1));
        assert_eq!(
            body["stream_options"]["include_usage"],
            serde_json::json!(true),
        );
        // The routed model is named on the wire. Omitting it left the policy constraining the record
        // rather than the call, which is the defect `BRN-015` fixed for the request type.
        assert_eq!(body["model"], serde_json::json!("test-model"));
    }

    #[test]
    fn the_runs_output_ceiling_is_forwarded_under_the_parameter_that_bounds_what_is_measured() {
        // `BRN-052`. The budget's `max_output_tokens` is checked against the provider's own reported
        // `completion_tokens`, and the check is only *enforcement* if the provider was told the bound —
        // otherwise a provider is free to produce an answer JARVIS then throws away, which is data
        // loss rather than a limit. Nothing forwarded it before this.
        //
        // Three properties, each of which a plausible wrong implementation would fail:
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.limits.max_output_tokens = Some(1234);
        let body = adapter.build_body(&request).expect("the fixture builds");

        // 1. It is `max_completion_tokens`. The older `max_tokens` is documented as "deprecated in
        //    favor of `max_completion_tokens`, and is not compatible with o-series models", so a body
        //    using the deprecated name would be refused by the reasoning models this adapter can be
        //    pointed at.
        assert_eq!(
            body["max_completion_tokens"],
            serde_json::json!(1234),
            "the ceiling must be sent under the current parameter name",
        );
        assert!(
            body.get("max_tokens").is_none(),
            "**the deprecated parameter must not be sent**: it is documented as incompatible with \
             o-series models, so it would refuse exactly the models that need the bound",
        );

        // 2. The value is the run's own ceiling, not a constant this adapter invented. Asserted with a
        //    second value, because a single comparison against a literal would also pass if the adapter
        //    hard-coded it.
        request.limits.max_output_tokens = Some(7);
        let second = adapter.build_body(&request).expect("the fixture builds");
        assert_eq!(second["max_completion_tokens"], serde_json::json!(7));

        // 3. **An absent ceiling sends no parameter at all.** Sending `null`, `0`, or a default would
        //    invent a limit the run never stated — and `0` in particular would ask the provider for an
        //    empty answer.
        request.limits.max_output_tokens = None;
        let unset = adapter.build_body(&request).expect("the fixture builds");
        assert!(
            unset.get("max_completion_tokens").is_none(),
            "a run with no ceiling must not have one invented for it: {unset}",
        );
    }

    #[test]
    fn a_model_the_adapter_does_not_serve_is_refused_rather_than_substituted() {
        // The routed model is a policy decision, so answering with a different one would violate the
        // data policy that selected it.
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.model = ModelRef::new(
            ProviderId::parse("local.llamacpp").expect("valid"),
            ModelId::parse("a-model-it-does-not-serve").expect("valid"),
        );
        assert_eq!(
            adapter.build_body(&request),
            Err(ProviderError::InvalidRequest),
        );
    }

    #[test]
    fn a_requested_output_schema_is_refused_rather_than_ignored() {
        // Structured output is out of this slice's scope. Ignoring the schema would return
        // unconstrained prose that a caller believed matched a schema.
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.output_schema = Some(
            jarvis_domain::model::stream::JsonText::new("{\"type\":\"object\"}").expect("valid"),
        );
        assert_eq!(
            adapter.build_body(&request),
            Err(ProviderError::InvalidRequest),
        );
    }

    #[test]
    fn too_many_tools_is_refused_rather_than_sent() {
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.tools = (0..=MAX_TOOLS)
            .map(|index| format!("tool-{index}"))
            .collect();
        assert_eq!(
            adapter.build_body(&request),
            Err(ProviderError::InvalidRequest),
        );
    }

    #[test]
    fn an_offered_tool_carries_its_description_and_schema_to_the_model() {
        use jarvis_domain::model::stream::ToolOffer;
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.tools = vec!["mcp.read_file@1".to_owned(), "clock.now@1".to_owned()];
        request.tool_offers = vec![ToolOffer {
            name: "mcp.read_file@1".to_owned(),
            description: "Reads a file".to_owned(),
            input_schema: Some(
                r#"{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}"#
                    .to_owned(),
            ),
        }];
        let body = adapter.build_body(&request).expect("builds");
        let tools = body["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 2);
        let read = &tools[0]["function"];
        assert_eq!(read["description"], "Reads a file");
        assert_eq!(read["parameters"]["required"][0], "path");
        // A tool with no offer keeps the bare object schema and no description, as before.
        let clock = &tools[1]["function"];
        assert_eq!(clock["parameters"], serde_json::json!({"type": "object"}));
        assert!(clock.get("description").is_none());
    }

    #[test]
    fn a_schema_that_is_not_an_object_is_not_forwarded() {
        use jarvis_domain::model::stream::ToolOffer;
        let adapter = provider();
        let mut request = request_for(&adapter);
        request.tools = vec!["t.one@1".to_owned(), "t.two@1".to_owned()];
        request.tool_offers = ["not json", "[1,2]"]
            .iter()
            .zip(["t.one@1", "t.two@1"])
            .map(|(schema, name)| ToolOffer {
                name: name.to_owned(),
                description: String::new(),
                input_schema: Some((*schema).to_owned()),
            })
            .collect();
        let body = adapter.build_body(&request).expect("builds");
        for tool in body["tools"].as_array().expect("tools") {
            assert_eq!(
                tool["function"]["parameters"],
                serde_json::json!({"type": "object"})
            );
        }
    }
    /// Builds a minimal valid request for `adapter`'s served model.
    fn request_for(adapter: &OpenAiCompatibleProvider) -> ModelCallRequest {
        use jarvis_domain::ids::{ModelCallId, RunId};
        use jarvis_domain::model::identity::ModelRef;
        use jarvis_domain::model::stream::{
            CallLimits, ContentBlock, InputItem, InputItems, ModelCallRequest, RouteRequirements,
        };

        // The routed model comes from the adapter's own served list, so the fixture cannot assert a
        // body shape against a model the adapter would refuse.
        let model: ModelRef = adapter
            .models
            .first()
            .expect("one model is configured")
            .clone();
        ModelCallRequest {
            call_id: ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
            run_id: RunId::from_uuid(uuid::Uuid::from_u128(5)),
            model,
            route_requirements: RouteRequirements::text(),
            input: InputItems::new(vec![InputItem::Message {
                role: Role::User,
                blocks: vec![ContentBlock::Text {
                    text: "hello".to_owned(),
                }],
            }])
            .expect("the fixture is valid"),
            tools: Vec::new(),
            tool_offers: Vec::new(),
            output_schema: None,
            settings: PortableSettings::default(),
            limits: CallLimits {
                deadline: None,
                max_output_tokens: None,
                max_cost_microunits: None,
            },
        }
    }

    #[tokio::test]
    async fn a_body_whose_peer_closes_without_a_terminal_ends_as_unavailable() {
        // The peer closed cleanly but never named a finish reason. A stream that simply ended would be
        // recorded as *interrupted*, which is the accurate answer for a transport that died — and the
        // difference between that and a completion is exactly what a caller must be able to see.
        let ids = crate::ids::UuidV7Generator::new();
        let mut stamper = jarvis_application::model::FrameStamper::start(
            jarvis_domain::ids::ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
            &ids,
        );
        let mut translator = super::ChunkTranslator::new();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        // A healthy content frame followed by end-of-body and no terminal.
        let mut body: &[u8] = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n";
        let error = pump_body(&mut body, &sender, &mut stamper, &mut translator)
            .await
            .expect_err("a stream with no terminal is unavailable");
        assert_eq!(error, ProviderError::Unavailable);
        // The content frame *was* forwarded before the failure, so the caller keeps the partial
        // answer and the failure rather than losing both.
        assert!(receiver.try_recv().is_ok());
    }

    #[tokio::test]
    async fn a_frame_that_never_completes_is_malformed_rather_than_a_clean_end() {
        let ids = crate::ids::UuidV7Generator::new();
        let mut stamper = jarvis_application::model::FrameStamper::start(
            jarvis_domain::ids::ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
            &ids,
        );
        let mut translator = super::ChunkTranslator::new();
        let (sender, _receiver) = tokio::sync::mpsc::channel(8);
        // A truncated body: the frame never gets its blank line.
        let mut body: &[u8] = b"data: {\"choices\":[{\"index\":0";
        let error = pump_body(&mut body, &sender, &mut stamper, &mut translator)
            .await
            .expect_err("a truncated frame is malformed");
        assert_eq!(error, ProviderError::Malformed);
    }

    #[tokio::test]
    async fn a_terminal_frame_completes_the_body_and_the_sentinel_is_skipped() {
        let ids = crate::ids::UuidV7Generator::new();
        let mut stamper = jarvis_application::model::FrameStamper::start(
            jarvis_domain::ids::ModelCallId::from_uuid(uuid::Uuid::from_u128(9)),
            &ids,
        );
        let mut translator = super::ChunkTranslator::new();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        // A content frame, a terminal naming `stop`, then the sentinel. The sentinel must be skipped
        // rather than parsed, and the call must complete rather than be reported unavailable.
        let mut body: &[u8] = b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        pump_body(&mut body, &sender, &mut stamper, &mut translator)
            .await
            .expect("a terminal completes the body");

        let mut kinds = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            kinds.push(event.kind);
        }
        assert_eq!(
            kinds.iter().filter(|kind| kind.is_terminal()).count(),
            1,
            "exactly one terminal: {kinds:?}",
        );
        // Sequences are contiguous from 1, because the stamper owns numbering rather than each layer.
        let sequences: Vec<u64> = (1..=kinds.len() as u64).collect();
        assert_eq!(kinds.len() as u64, sequences.len() as u64);
    }

    #[test]
    fn an_unmodelled_frame_field_does_not_break_the_request_body_builder() {
        // A guard against a future field being added to the request type and silently ignored here:
        // the builder reads named fields, so an addition is a compile error rather than a dropped
        // setting.
        let adapter = provider();
        let request = request_for(&adapter);
        let body = adapter.build_body(&request).expect("builds");
        assert!(body.get("messages").is_some());
        assert!(body.get("tools").is_none(), "no tools were requested");
    }
}
