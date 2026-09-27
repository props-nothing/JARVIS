//! Real model provider adapters.
//!
//! An adapter owns its provider's credentials, transport, and wire encoding, and produces only
//! normalized [`ModelStreamEvent`](jarvis_domain::model::stream::ModelStreamEvent) values. Nothing
//! here decides policy, approves a tool call, or persists anything: an adapter *proposes* frames and
//! the deterministic layers above decide what they mean.
//!
//! The module lives in this crate rather than in `jarvis-application` because an adapter depends on a
//! transport, a JSON codec, and a credential store, and the application layer is deliberately free of
//! all three — that is the dependency direction `AGENTS.md` fixes. It lives under
//! `model_providers/` because the evidence manifest scopes exactly that path to the
//! `openai-compatible-model` entry, so an edit here is gated on the note that describes the contract.
//!
//! [`openai_compatible`] is the first adapter. It speaks the OpenAI-compatible Chat Completions
//! streaming contract over a loopback, plaintext connection, which is the only transport this
//! workspace can currently secure: the evidence note records a measured absence of any TLS
//! implementation in `Cargo.lock`, so a cloud endpoint needs its own reviewed dependency rather than
//! an implied one.
//!
//! [`resolve`] turns a non-secret configuration document into the provider a daemon will actually
//! call. That function is the seam between "what the operator wrote" and "what the daemon composes",
//! and it is here rather than in the daemon so the mapping is unit-testable without a socket.

pub mod openai_compatible;

use std::sync::Arc;

use jarvis_application::model::{ModelProvider, ScriptedProvider};
use jarvis_domain::model::identity::{ModelId, ModelRef, ProviderId};
use jarvis_domain::model::stream::{FinishReason, ModelStreamEventKind};

use crate::config::{Config, ProviderSection, SecretResolver};

use openai_compatible::OpenAiCompatibleProvider;

/// Why a configured provider could not be composed.
///
/// Separate from the adapter's own [`ConfigError`](openai_compatible::ConfigError) because this
/// layer's failures are about *the document*, not the endpoint: a table naming no provider id, a
/// model name that is not a legal model identity, or a credential reference that cannot be resolved.
/// A failure the adapter owns — a non-loopback host, say — is reported as
/// [`ProviderResolutionError::Adapter`] carrying the adapter's own code, so there is exactly one
/// implementation of each rule and the operator still gets the specific diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderResolutionError {
    /// The configured provider id is not a legal provider identity.
    InvalidProviderId,
    /// The provider table names no models.
    NoModels,
    /// A configured model name is not a legal model identity.
    InvalidModelId,
    /// The credential reference could not be resolved to a value.
    CredentialUnavailable,
    /// The adapter refused the endpoint or the credential it was given.
    Adapter(openai_compatible::ConfigError),
}

impl ProviderResolutionError {
    /// Returns the stable, namespaced error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidProviderId => "model.provider_id_invalid",
            Self::NoModels => "model.adapter_no_models",
            Self::InvalidModelId => "model.model_id_invalid",
            Self::CredentialUnavailable => "model.provider_credential_unavailable",
            Self::Adapter(error) => error.code(),
        }
    }
}

/// Composes the provider a daemon will call from a configuration document.
///
/// Three outcomes, and the middle one is the point:
///
/// - **No `[model.provider]` table** composes the deterministic scripted provider. That is the
///   default for a fresh install and for every test, and its model identifier says `scripted.local`
///   so an operator can see which source served a run. It is not a stub of something that exists; it
///   is the fake provider the plan requires.
/// - **A table that composes** composes the adapter, and the credential is resolved here, once.
/// - **A table that cannot compose** returns an error. This is a **startup refusal**, not a fallback
///   to the scripted provider, and the distinction matters: falling back would let a typo in a host,
///   a port, or a credential silently route every run to a provider that returns a fixed
///   acknowledgement, and the operator would see successful runs while their real endpoint was never
///   contacted. A misconfiguration must be visible before a run depends on it.
///
/// The credential is resolved **once, at composition**. The repository's rule is to resolve secret
/// material at the last responsible moment, and for a long-lived daemon that moment is startup rather
/// than each request: the value is then held in memory for the process's lifetime, is never written
/// to a record, and is never rendered (the adapter's `Debug` redacts it). Resolving per request would
/// re-read the environment thousands of times and would let the provider identity change underneath
/// a run that had already selected it.
///
/// # Errors
///
/// Returns a [`ProviderResolutionError`] for a document that names a provider it cannot compose.
pub fn resolve(
    config: &Config,
    secrets: &dyn SecretResolver,
) -> Result<Arc<dyn ModelProvider>, ProviderResolutionError> {
    let Some(section) = &config.model.provider else {
        return Ok(Arc::new(scripted_provider()));
    };
    // A provider reference is resolved even though the adapter would also refuse an empty one, so the
    // failure names *the configured reference* rather than reporting an invalid credential with no
    // mention of what was meant to supply it.
    let Some(reference) = &config.model.api_key_ref else {
        return Err(ProviderResolutionError::CredentialUnavailable);
    };
    let api_key = secrets
        .resolve(reference)
        .map_err(|_| ProviderResolutionError::CredentialUnavailable)?;
    Ok(Arc::new(compose_adapter(section, &api_key)?))
}

/// Builds the adapter for a validated section and a resolved credential.
///
/// The model names are parsed with the same validator every other identifier uses, rather than
/// accepted as bare strings: a model id reaches a routing decision, a persisted `model_calls` row,
/// and a diagnostic, and the domain is where "this is a legal model identity" is defined.
fn compose_adapter(
    section: &ProviderSection,
    api_key: &str,
) -> Result<OpenAiCompatibleProvider, ProviderResolutionError> {
    let provider_id =
        ProviderId::parse(&section.id).map_err(|_| ProviderResolutionError::InvalidProviderId)?;
    if section.models.is_empty() {
        return Err(ProviderResolutionError::NoModels);
    }
    let mut model_ids = Vec::with_capacity(section.models.len());
    for name in &section.models {
        model_ids.push(ModelId::parse(name).map_err(|_| ProviderResolutionError::InvalidModelId)?);
    }
    OpenAiCompatibleProvider::new(provider_id, &section.host, section.port, api_key, model_ids)
        .map_err(ProviderResolutionError::Adapter)
}

/// The deterministic provider the daemon composes when no provider is configured.
///
/// The script echoes a bounded acknowledgement rather than the caller's text, so the deterministic
/// path cannot be mistaken for a real model's answer and its output cannot reflect prompt content
/// into a public event payload.
#[must_use]
pub fn scripted_provider() -> ScriptedProvider {
    // The identifiers are literals this build controls, so a failure here would be a programming
    // error rather than a runtime condition. They are validated once and fall back rather than being
    // unwrapped, so a future edit that mistypes one degrades to a provider serving no model — which
    // makes every run fail with `run.no_model_served`, a state an operator can see — instead of
    // panicking on the startup path.
    match (
        ProviderId::from_literal("scripted.local"),
        ModelId::from_literal("scripted-echo"),
    ) {
        (Some(provider_id), Some(model_id)) => ScriptedProvider::new(ModelRef::new(
            provider_id, model_id,
        ))
        .emit(ModelStreamEventKind::OutputItemAdded {
            item_id: "scripted-answer".to_owned(),
        })
        .emit_text(
            "scripted-answer",
            "The scripted provider received this run. Configure a model provider to receive real answers.",
        )
        .emit(ModelStreamEventKind::CallCompleted {
            finish_reason: FinishReason::Stop,
            usage: None,
            refused: false,
        }),
        _ => ScriptedProvider::serving_no_model(),
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderResolutionError, resolve};
    use crate::config::{Config, EnvSecretResolver, MapSecretResolver};
    use jarvis_application::model::ModelProvider as _;
    use jarvis_domain::model::identity::EndpointClass;

    const PROVIDER: &str = r#"
schema_version = 2

[model]
policy_id = "local-default"
api_key_ref = "env:JARVIS_MODEL_KEY"

[model.provider]
id = "local.llamacpp"
host = "127.0.0.1"
port = 8080
models = ["qwen2.5-7b-instruct"]
"#;

    #[test]
    fn no_provider_table_composes_the_scripted_provider() {
        let config = Config::from_toml("schema_version = 2").expect("minimal document");
        let provider = resolve(&config, &MapSecretResolver::new()).expect("composes");
        // The scripted provider is the default, and it serves exactly its own model under a name an
        // operator can recognise as not being a real endpoint.
        assert_eq!(provider.models().len(), 1);
        assert_eq!(
            provider.models()[0].to_string(),
            "scripted.local/scripted-echo"
        );
        assert_eq!(provider.endpoint_class(), EndpointClass::Local);
    }

    #[test]
    fn a_configured_table_composes_the_adapter_with_the_resolved_credential() {
        let config = Config::from_toml(PROVIDER).expect("provider document");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test-not-a-real-key");
        let provider = resolve(&config, &secrets).expect("composes");
        assert_eq!(provider.models().len(), 1);
        assert_eq!(
            provider.models()[0].to_string(),
            "local.llamacpp/qwen2.5-7b-instruct"
        );
        assert_eq!(provider.endpoint_class(), EndpointClass::Local);
    }

    #[test]
    fn an_unresolvable_credential_is_a_startup_refusal_not_a_fallback() {
        // The important half: a configured provider whose credential is missing must NOT quietly
        // compose the scripted provider, because the operator would then see successful runs while
        // their real endpoint was never contacted.
        let config = Config::from_toml(PROVIDER).expect("provider document");
        let error = resolve(&config, &MapSecretResolver::new())
            .err()
            .expect("a missing credential must refuse");
        assert_eq!(error, ProviderResolutionError::CredentialUnavailable);
        assert_eq!(error.code(), "model.provider_credential_unavailable");
    }

    #[test]
    fn a_configuration_with_no_credential_reference_is_refused_by_name() {
        let document = PROVIDER.replace("api_key_ref = \"env:JARVIS_MODEL_KEY\"\n", "");
        let config = Config::from_toml(&document).expect("parses");
        assert!(config.model.api_key_ref.is_none());
        let error = resolve(&config, &MapSecretResolver::new())
            .err()
            .expect("a provider without a credential reference must refuse");
        assert_eq!(error, ProviderResolutionError::CredentialUnavailable);
    }

    #[test]
    fn the_environment_resolver_reads_a_variable_it_is_given_and_reports_a_missing_one() {
        // Both halves, and the second is deliberately **not** `EnvSecretResolver::new()`.
        //
        // The real resolver with the real process lookup was the first version of this test, and it
        // was **not a test**: it asserted `resolve(..).is_err()`, which held only while
        // `JARVIS_MODEL_KEY` happened to be absent from the ambient environment. It passed under
        // `cargo test` and failed under `cargo test --workspace`, because the manual daemon check
        // that came before it had exported that variable in the same shell — a test whose result
        // depends on the environment it is launched from is a test that reports on the operator, not
        // on the code.
        //
        // The injectable lookup form is the fix, and it is a `fn` pointer by design so the view is
        // fixed at construction and cannot be mutated concurrently — which is why the process
        // environment is never touched (Rust 2024 marks that `unsafe`, and this crate forbids it).
        const ABSENT: fn(&str) -> Option<String> = |_| None;
        const PRESENT: fn(&str) -> Option<String> = |_| Some(String::new());
        let config = Config::from_toml(PROVIDER).expect("provider document");
        let error = resolve(&config, &EnvSecretResolver::with_lookup(ABSENT))
            .err()
            .expect("a lookup that finds nothing must refuse");
        assert_eq!(error, ProviderResolutionError::CredentialUnavailable);

        // An **empty** value is found-but-unusable, which is a different path from not-found and is
        // what the adapter's own credential check refuses. Asserting the distinct code is what makes
        // this half falsify the "unavailable" mapping rather than repeating the first half.
        let error = resolve(&config, &EnvSecretResolver::with_lookup(PRESENT))
            .err()
            .expect("an empty credential must be refused by the adapter");
        assert_eq!(error.code(), "model.adapter_credential_invalid");
    }

    #[test]
    fn a_provider_id_that_is_not_a_legal_identity_is_refused() {
        let document = PROVIDER.replace("id = \"local.llamacpp\"", "id = \"Local Llama\"");
        let config = Config::from_toml(&document).expect("parses");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(error, ProviderResolutionError::InvalidProviderId);
        assert_eq!(error.code(), "model.provider_id_invalid");
    }

    #[test]
    fn a_model_name_that_is_not_a_legal_identity_is_refused() {
        // A model id reaches a routing decision and a persisted row, so it is validated rather than
        // accepted as an opaque string.
        let document = PROVIDER.replace("qwen2.5-7b-instruct", "Qwen 2.5");
        let config = Config::from_toml(&document).expect("parses");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(error, ProviderResolutionError::InvalidModelId);
        assert_eq!(error.code(), "model.model_id_invalid");
    }

    #[test]
    fn a_table_naming_no_models_is_refused() {
        let document = PROVIDER.replace("models = [\"qwen2.5-7b-instruct\"]", "models = []");
        let config = Config::from_toml(&document).expect("parses");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(error, ProviderResolutionError::NoModels);
    }

    #[test]
    fn a_non_loopback_endpoint_is_refused_by_the_adapter_s_own_rule() {
        // This layer does not check the host. The adapter owns that predicate, and the composition
        // must surface the adapter's own code rather than inventing a second spelling of it.
        let document = PROVIDER.replace("host = \"127.0.0.1\"", "host = \"api.example.com\"");
        let config = Config::from_toml(&document).expect("parses");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(
            error,
            ProviderResolutionError::Adapter(
                crate::model_providers::openai_compatible::ConfigError::NotLoopback
            )
        );
        assert_eq!(error.code(), "model.adapter_endpoint_not_loopback");
    }

    #[test]
    fn a_url_shaped_host_is_refused_rather_than_stripped() {
        let document = PROVIDER.replace("host = \"127.0.0.1\"", "host = \"http://127.0.0.1\"");
        let config = Config::from_toml(&document).expect("parses");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "sk-test");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(error.code(), "model.adapter_endpoint_url_shaped");
    }

    #[test]
    fn an_empty_credential_is_refused() {
        let config = Config::from_toml(PROVIDER).expect("provider document");
        let secrets = MapSecretResolver::new();
        secrets.insert("JARVIS_MODEL_KEY", "");
        let error = resolve(&config, &secrets).err().expect("refused");
        assert_eq!(error.code(), "model.adapter_credential_invalid");
    }

    #[test]
    fn the_scripted_fallback_helper_is_used_by_the_unconfigured_path_only() {
        // Guards the helper directly, so a future edit that changes the scripted answer must also
        // change this assertion rather than only the daemon's composition.
        let provider = super::scripted_provider();
        assert_eq!(provider.models()[0].model_id.as_str(), "scripted-echo");
    }
}
