//! Supervising launched MCP servers: noticing a dead child and restarting it under a bound.
//!
//! Composition launches each declared server once. Before this module a child that died *after* a
//! successful start stayed dead for the life of the daemon: the executor refused a dispatch to it
//! (`Unavailable`, settled and safe) and `McpComposition::health` reported it, but nothing acted.
//!
//! # What a restart may and may not change
//!
//! A restart replaces the **session** behind an existing [`McpToolExecutor`]; it never changes what the
//! executor may call. The new child is discovered, normalized, and registered into a *scratch* registry
//! exactly like a first start, and the restart is accepted only when the identities it admits are
//! **exactly** the set the executor was built with. A server that comes back offering a different catalog —
//! a re-schemaed tool, an added tool, a removed one — is a different thing from the one the operator's
//! grants and approvals were written against, so it is **quarantined** rather than silently substituted.
//! Quarantine is visible (`McpSupervisionEvent::Quarantined`, logged at `warn`) and ends only when the
//! daemon is restarted, which is when an operator's review of the changed catalog happens.
//!
//! # Restarts are bounded, because an unbounded restart loop is a denial of service on the machine
//!
//! Each attempt, successful or not, counts toward `max_consecutive_failures`, and the count resets only
//! after a restarted server has stayed open for `stable_window`. A server that starts and dies at once
//! therefore exhausts its budget instead of being relaunched for ever, and the delay between attempts
//! doubles from `base_delay` up to `max_delay`.
//!
//! # What this does not do
//!
//! It does not retry a **call**: a call that raced the death is still reported unsettled by the executor,
//! because the request may have been delivered and executed. It does not detect a *hung* child that is
//! still open — liveness here is the session's own `is_closed`, so heartbeat probing remains outstanding.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use jarvis_domain::tool::registry::ToolRegistry;
use tokio::time::Instant;

use super::composition::{McpComposition, compose_discovered, launch_spec_for};
use super::discovery::discover_stdio_server;
use super::executor::McpToolExecutor;
use crate::config::mcp::McpServerDeclaration;
use crate::config::secret::SecretResolver;

/// The bounds on restarting one server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpRestartPolicy {
    /// The wait after the first attempt; doubled for each further consecutive attempt.
    pub base_delay: Duration,
    /// The longest wait between attempts.
    pub max_delay: Duration,
    /// How many consecutive attempts are made before the server is quarantined.
    pub max_consecutive_failures: u32,
    /// How long a restarted server must stay open before its attempt count is forgotten.
    pub stable_window: Duration,
}

impl Default for McpRestartPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_consecutive_failures: 5,
            stable_window: Duration::from_secs(60),
        }
    }
}

impl McpRestartPolicy {
    /// The wait that follows the `attempt`th consecutive attempt (1-based), capped at `max_delay`.
    #[must_use]
    pub fn delay_after(&self, attempt: u32) -> Duration {
        let shift = attempt.saturating_sub(1).min(31);
        self.base_delay
            .saturating_mul(1_u32 << shift)
            .min(self.max_delay)
    }
}

/// One thing the supervisor did, for the caller to log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSupervisionEvent {
    /// A dead server was relaunched and its session replaced.
    Restarted {
        /// The declared server name.
        server: String,
        /// Which consecutive attempt this was.
        attempt: u32,
    },
    /// A restart attempt failed in a way a later attempt might fix.
    AttemptFailed {
        /// The declared server name.
        server: String,
        /// The stable code of the refusal.
        code: &'static str,
        /// Which consecutive attempt this was.
        attempt: u32,
        /// How long before the next attempt is permitted.
        retry_in: Duration,
    },
    /// The server will not be restarted again until the daemon is.
    Quarantined {
        /// The declared server name.
        server: String,
        /// The stable code of the reason.
        code: &'static str,
    },
}

/// The code recorded when a restarted server offers a different set of tools.
pub const IDENTITY_CHANGED_CODE: &str = "mcp.restart_identity_changed";

/// The code recorded when the restart budget is spent.
pub const BUDGET_EXHAUSTED_CODE: &str = "mcp.restart_budget_exhausted";

/// The code recorded when a dead server has no declaration to relaunch from.
pub const NOT_DECLARED_CODE: &str = "mcp.restart_not_declared";

/// What the supervisor remembers about one server.
#[derive(Debug, Default)]
struct ServerState {
    /// Consecutive attempts since the server was last observed stable.
    attempts: u32,
    /// No attempt is permitted before this instant.
    not_before: Option<Instant>,
    /// When a restarted server counts as stable, if one has been restarted and not yet proven.
    stable_at: Option<Instant>,
    /// Whether the server has been given up on.
    quarantined: bool,
}

/// Restarts dead servers under a [`McpRestartPolicy`].
///
/// A value rather than a task so a test can drive one pass at a chosen instant; [`supervise_composition`] is
/// the loop the daemon runs.
#[derive(Debug)]
pub struct McpSupervisor {
    declarations: BTreeMap<String, McpServerDeclaration>,
    policy: McpRestartPolicy,
    state: BTreeMap<String, ServerState>,
}

impl McpSupervisor {
    /// Builds a supervisor over the declarations a profile's servers were launched from.
    #[must_use]
    pub fn new(declarations: Vec<McpServerDeclaration>, policy: McpRestartPolicy) -> Self {
        Self {
            declarations: declarations
                .into_iter()
                .map(|declaration| (declaration.name.clone(), declaration))
                .collect(),
            policy,
            state: BTreeMap::new(),
        }
    }

    /// Returns whether `server` has been given up on.
    #[must_use]
    pub fn is_quarantined(&self, server: &str) -> bool {
        self.state
            .get(server)
            .is_some_and(|state| state.quarantined)
    }

    /// Runs one pass over `servers`, restarting each closed one that is due.
    ///
    /// `now` is a parameter so the schedule is a function of its input; a test supplies the instants and
    /// never sleeps.
    pub async fn supervise_once(
        &mut self,
        servers: &[(String, Arc<McpToolExecutor>)],
        secrets: &dyn SecretResolver,
        now: Instant,
    ) -> Vec<McpSupervisionEvent> {
        let mut events = Vec::new();
        for (name, executor) in servers {
            let state = self.state.entry(name.clone()).or_default();
            if !executor.is_closed() {
                // Open: forget the attempts only once it has *stayed* open for the stable window, so a
                // server that dies straight after each restart cannot reset its own budget.
                if state.stable_at.is_some_and(|stable_at| now >= stable_at) {
                    *state = ServerState::default();
                }
                continue;
            }
            if state.quarantined || state.not_before.is_some_and(|due| now < due) {
                continue;
            }
            let Some(declaration) = self.declarations.get(name) else {
                state.quarantined = true;
                events.push(McpSupervisionEvent::Quarantined {
                    server: name.clone(),
                    code: NOT_DECLARED_CODE,
                });
                continue;
            };
            state.attempts += 1;
            let attempt = state.attempts;
            if attempt > self.policy.max_consecutive_failures {
                state.quarantined = true;
                events.push(McpSupervisionEvent::Quarantined {
                    server: name.clone(),
                    code: BUDGET_EXHAUSTED_CODE,
                });
                continue;
            }
            let retry_in = self.policy.delay_after(attempt);
            state.not_before = Some(now + retry_in);
            match restart_one(declaration, executor, secrets).await {
                Ok(()) => {
                    state.stable_at = Some(now + self.policy.stable_window);
                    events.push(McpSupervisionEvent::Restarted {
                        server: name.clone(),
                        attempt,
                    });
                }
                Err(failure) if failure.permanent => {
                    state.quarantined = true;
                    events.push(McpSupervisionEvent::Quarantined {
                        server: name.clone(),
                        code: failure.code,
                    });
                }
                Err(failure) => events.push(McpSupervisionEvent::AttemptFailed {
                    server: name.clone(),
                    code: failure.code,
                    attempt,
                    retry_in,
                }),
            }
        }
        events
    }
}

/// Why one restart attempt did not produce a live replacement.
struct RestartFailure {
    code: &'static str,
    permanent: bool,
}

/// Relaunches one server and swaps its session in, if it still offers exactly the tools it did.
async fn restart_one(
    declaration: &McpServerDeclaration,
    executor: &McpToolExecutor,
    secrets: &dyn SecretResolver,
) -> Result<(), RestartFailure> {
    // Secrets are resolved again here, at the last responsible moment, and are not kept between attempts.
    let spec = launch_spec_for(declaration, secrets).map_err(|refusal| RestartFailure {
        code: refusal.code(),
        permanent: refusal.is_permanent(),
    })?;
    let discovered = discover_stdio_server(&spec, &declaration.name)
        .await
        .map_err(|refusal| RestartFailure {
            code: refusal.code(),
            permanent: refusal.is_permanent(),
        })?;
    // A scratch registry: the daemon's own admitted these identities at startup, and re-registering into it
    // would be a second mutable authority. What is needed from registration is only the *set it admits*.
    let mut scratch = ToolRegistry::new();
    let replacement =
        compose_discovered(&mut scratch, discovered).map_err(|refusal| RestartFailure {
            code: refusal.code(),
            permanent: refusal.is_permanent(),
        })?;
    let session = replacement.session();
    if !executor.offers_exactly(replacement.admitted()) {
        // Stopped explicitly rather than left to `Drop`, so the child is asked to terminate through the
        // protocol before its transport is reaped.
        session.cancellation_token().cancel();
        return Err(RestartFailure {
            code: IDENTITY_CHANGED_CODE,
            permanent: true,
        });
    }
    // The previous session is closed; dropping it releases its transport.
    drop(executor.replace_session(session));
    Ok(())
}

/// The snapshot a pass needs from a composition: each server's name and executor.
async fn snapshot(
    holder: &tokio::sync::Mutex<Option<McpComposition>>,
) -> Option<Vec<(String, Arc<McpToolExecutor>)>> {
    let guard = holder.lock().await;
    let composition = guard.as_ref()?;
    Some(
        composition
            .servers()
            .iter()
            .map(|server| {
                (
                    server.server().as_str().to_owned(),
                    Arc::clone(server.executor()),
                )
            })
            .collect(),
    )
}

/// Runs the supervisor over the daemon's held composition until it is taken for the drain or `stop` fires.
///
/// The composition is locked only to snapshot executors, never across a relaunch, so a drain is never
/// blocked behind a slow child. A restart that finishes *after* the drain took the composition is undone
/// here — the drain stopped the old session, and a replacement it never saw would be an orphaned child.
pub async fn supervise_composition(
    holder: Arc<tokio::sync::Mutex<Option<McpComposition>>>,
    mut supervisor: McpSupervisor,
    secrets: Arc<dyn SecretResolver>,
    interval: Duration,
    stop: Arc<tokio::sync::Notify>,
) {
    loop {
        tokio::select! {
            () = stop.notified() => return,
            () = tokio::time::sleep(interval) => {}
        }
        let Some(servers) = snapshot(&holder).await else {
            return;
        };
        let events = supervisor
            .supervise_once(&servers, secrets.as_ref(), Instant::now())
            .await;
        let draining = holder.lock().await.is_none();
        for event in &events {
            log_event(event);
        }
        if draining {
            for (_, executor) in &servers {
                executor.session().cancellation_token().cancel();
            }
            return;
        }
    }
}

fn log_event(event: &McpSupervisionEvent) {
    match event {
        McpSupervisionEvent::Restarted { server, attempt } => {
            log::info!("mcp server restarted: server={server} attempt={attempt}");
        }
        McpSupervisionEvent::AttemptFailed {
            server,
            code,
            attempt,
            retry_in,
        } => log::warn!(
            "mcp server restart failed: server={server} code={code} attempt={attempt} retry_in_ms={}",
            retry_in.as_millis()
        ),
        McpSupervisionEvent::Quarantined { server, code } => {
            log::warn!(
                "mcp server quarantined until the daemon restarts: server={server} code={code}"
            );
        }
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
