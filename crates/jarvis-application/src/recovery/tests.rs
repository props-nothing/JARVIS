//! Tests for startup reconciliation.
//!
//! These drive the pass against the in-memory repository double, so an assertion is
//! about what was **persisted** — the state each run settled in and the event that
//! explains it — rather than about which method was called. The important cases are the
//! ones where the pass must *not* act: a terminal run, and a run that finished between
//! the read and the write.

use std::sync::Arc;

use jarvis_domain::ids::{ConversationId, PrincipalId, RunActivityEventId, RunId, WorkspaceId};
use jarvis_domain::run::state::{RunState, RunVersion, TransitionActor, TransitionReason};
use jarvis_domain::time::UtcTimestamp;
use uuid::Uuid;

use super::{RecoveryError, reconcile};
use crate::repository::conversation::{ConversationRepository, NewConversation};
use crate::repository::run::{
    MAX_INCOMPLETE_RUNS, NewActivityEvent, NewRun, RunRepository, RunWrite, run_received_event,
};
use crate::testing::InMemoryRepositories;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn now() -> UtcTimestamp {
    UtcTimestamp::parse("2026-09-22T12:00:00Z").expect("valid")
}

fn workspace() -> WorkspaceId {
    WorkspaceId::from_uuid(id(1))
}

fn conversation() -> ConversationId {
    ConversationId::from_uuid(id(2))
}

async fn seed_conversation(repositories: &InMemoryRepositories) {
    repositories
        .create_conversation(
            NewConversation::new(
                conversation(),
                workspace(),
                PrincipalId::from_uuid(id(3)),
                None,
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("the conversation is created");
}

/// Creates a run and drives it to `state` along a legal path.
///
/// The state is reached through real transitions rather than written directly, so the
/// run the pass sees is one the machine could actually produce — a run forged into an
/// unreachable state would test a situation that cannot occur.
async fn run_in(repositories: &InMemoryRepositories, run_id: RunId, state: RunState) {
    repositories
        .create(
            NewRun::new(
                run_id,
                workspace(),
                conversation(),
                PrincipalId::from_uuid(id(3)),
                None,
                now(),
            )
            .expect("valid"),
            run_received_event(run_id, now()),
        )
        .await
        .expect("the run is created");
    if state == RunState::Received {
        return;
    }

    // Every path to a non-terminal state, so the fixture cannot claim a state the
    // machine cannot reach.
    let path: &[(RunState, RunState)] = match state {
        RunState::ContextBuilding => &[(RunState::Received, RunState::ContextBuilding)],
        RunState::Planning => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
        ],
        RunState::AwaitingModel => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
        ],
        RunState::Responding => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
            (RunState::AwaitingModel, RunState::Responding),
        ],
        RunState::ExecutingTool => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
            (RunState::AwaitingModel, RunState::ExecutingTool),
        ],
        RunState::Observing => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
            (RunState::AwaitingModel, RunState::ExecutingTool),
            (RunState::ExecutingTool, RunState::Observing),
        ],
        RunState::Waiting => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
            (RunState::AwaitingModel, RunState::ExecutingTool),
            (RunState::ExecutingTool, RunState::Observing),
            (RunState::Observing, RunState::Waiting),
        ],
        RunState::AwaitingApproval => &[
            (RunState::Received, RunState::ContextBuilding),
            (RunState::ContextBuilding, RunState::Planning),
            (RunState::Planning, RunState::AwaitingModel),
            (RunState::AwaitingModel, RunState::AwaitingApproval),
        ],
        other => unreachable!("no fixture path to {other}"),
    };

    let mut version = RunVersion::FIRST;
    for (index, (from, to)) in path.iter().enumerate() {
        let transition = jarvis_domain::run::lifecycle::RunTransition::new(
            *from,
            *to,
            version,
            TransitionActor::Controller,
            TransitionReason::new("step").expect("valid"),
            now(),
        );
        let write = RunWrite::new(
            &transition,
            NewActivityEvent {
                run_id,
                sequence: u64::try_from(index).expect("small") + 2,
                event_type: "run.step".to_owned(),
                payload_json: None,
                visibility: crate::repository::run::EventVisibility::Public,
                occurred_at: now(),
            },
        );
        let write = if to.is_waiting() {
            write.waiting_on(
                crate::repository::run::WaitingOn::new("timer", "wake-1").expect("valid"),
            )
        } else {
            write
        };
        version = repositories
            .transition(workspace(), write)
            .await
            .expect("the fixture path is legal")
            .version;
    }
}

#[tokio::test]
async fn an_interrupted_run_is_recovered_to_a_terminal_state() {
    // The core requirement: a run that the daemon left mid-flight must not stay
    // non-terminal, or a client polls it forever for an answer that will never come.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(10));
    run_in(&repositories, run_id, RunState::AwaitingModel).await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = reconcile(&runs, now()).await.expect("reconciliation runs");
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.summary.abandoned, 1);
    assert_eq!(report.summary.parked, 0);
    assert_eq!(report.examined(), 1);

    let stored = repositories.load(workspace(), run_id).await.expect("loads");
    assert_eq!(stored.state, RunState::Failed);
    assert!(stored.is_terminal());
    assert_eq!(
        stored.completed_at,
        Some(now()),
        "a recovered run must record when it finished",
    );
}

#[tokio::test]
async fn every_interrupted_working_state_is_recovered() {
    // Exhaustive over the working states, because a state left out would be a run that
    // stays non-terminal forever — the defect this pass exists to fix.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let states = [
        RunState::Received,
        RunState::ContextBuilding,
        RunState::Planning,
        RunState::AwaitingModel,
        RunState::ExecutingTool,
        RunState::Observing,
        RunState::Responding,
    ];
    for (index, state) in states.iter().enumerate() {
        run_in(
            &repositories,
            RunId::from_uuid(id(100 + u128::try_from(index).expect("small"))),
            *state,
        )
        .await;
    }
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = reconcile(&runs, now()).await.expect("runs");
    assert!(report.is_complete(), "{report:?}");
    assert_eq!(report.summary.abandoned, 7, "{report:?}");

    // And nothing is left non-terminal.
    let remaining = repositories.incomplete_runs().await.expect("readable");
    assert!(remaining.runs.is_empty(), "{remaining:?}");
    assert!(
        !remaining.bounded,
        "the store held nothing more: {remaining:?}"
    );
}

#[tokio::test]
async fn a_parked_run_is_reported_as_parked_rather_than_as_lost_work() {
    // The distinction is the whole point of classifying separately: a run waiting on a
    // dependency lost nothing, so telling an operator it "was working" would be wrong.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    for (index, state) in [RunState::Waiting, RunState::AwaitingApproval]
        .iter()
        .enumerate()
    {
        run_in(
            &repositories,
            RunId::from_uuid(id(200 + u128::try_from(index).expect("small"))),
            *state,
        )
        .await;
    }
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = reconcile(&runs, now()).await.expect("runs");
    assert_eq!(report.summary.parked, 2, "{report:?}");
    assert_eq!(report.summary.abandoned, 0, "{report:?}");

    // The recorded reason distinguishes the case, which is where the difference has to
    // survive now that both land in the same terminal state.
    let events = repositories.recorded_events().expect("readable");
    assert!(
        events.iter().any(|event| event.event_type == "run.failed"
            && event
                .payload_json
                .as_deref()
                .is_some_and(|payload| payload.contains("resumable"))),
        "{events:?}",
    );
}

#[tokio::test]
async fn a_terminal_run_is_left_exactly_as_it_was() {
    // "Terminal runs remain terminal" is the contract's first rule here. Recovering one
    // would rewrite a finished run's outcome.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(300));
    run_in(&repositories, run_id, RunState::Responding).await;
    // Finish it properly, so it is terminal for a real reason.
    let stored = repositories.load(workspace(), run_id).await.expect("loads");
    let completed = jarvis_domain::run::lifecycle::RunTransition::new(
        RunState::Responding,
        RunState::Completed,
        stored.version,
        TransitionActor::Controller,
        TransitionReason::new("answer_delivered").expect("valid"),
        now(),
    );
    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &completed,
                NewActivityEvent {
                    run_id,
                    sequence: 7,
                    event_type: "run.completed".to_owned(),
                    payload_json: None,
                    visibility: crate::repository::run::EventVisibility::Public,
                    occurred_at: now(),
                },
            ),
        )
        .await
        .expect("applies");

    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());
    let report = reconcile(&runs, now()).await.expect("runs");
    assert!(report.summary.is_empty(), "{report:?}");
    assert_eq!(report.examined(), 0);

    let after = repositories.load(workspace(), run_id).await.expect("loads");
    assert_eq!(after.state, RunState::Completed);
    assert_eq!(
        after.version.get(),
        6,
        "a completed run must not have been touched",
    );
}

#[tokio::test]
async fn a_page_that_settles_nothing_stops_the_pass_and_reports_the_store_as_incomplete() {
    // The consumer half of `the_paging_decision_...`, and the half that would **hang** if it were
    // wrong: a failed write leaves its run non-terminal, which is exactly what the read looks for,
    // so a pass that continued on a stalled page would read the same page for ever — on the startup
    // path, where nothing else can proceed. The pure test pins the decision; only this one pins
    // that the loop acts on it.
    //
    // `tokio::time::timeout` is what turns "hangs" into a failure. Without it the mutation would
    // present as a suite that never finishes, which a developer reads as a broken test runner
    // rather than as the defect — the same lesson as `BRN-008`'s deadline tests.
    let repositories = StallingWrites::new().await;
    let overflowing = MAX_INCOMPLETE_RUNS as usize + 1;
    for index in 0..overflowing {
        repositories
            .seed_interrupted(RunId::from_uuid(id(
                300_000 + u128::try_from(index).expect("small")
            )))
            .await;
    }
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = tokio::time::timeout(std::time::Duration::from_secs(10), reconcile(&runs, now()))
        .await
        .expect("a pass that settles nothing must return, not spin")
        .expect("the read succeeds");

    assert!(
        !report.is_complete(),
        "a pass that settled nothing must not report success: {report:?}",
    );
    assert!(
        report.incomplete_store,
        "the store still holds the runs it could not settle: {report:?}",
    );
    assert_eq!(
        report.failures.len(),
        MAX_INCOMPLETE_RUNS as usize,
        "one page was attempted, not more — the pass stopped when the page stopped changing: {report:?}",
    );
    // The fixture really did refuse: the runs are still interrupted, or this would be asserting a
    // stall that never happened.
    let remaining = repositories
        .inner()
        .incomplete_runs()
        .await
        .expect("readable");
    assert!(
        !remaining.runs.is_empty(),
        "the runs must still be interrupted for this to be a stall",
    );
}

/// A store whose reads succeed and whose every recovery write is refused.
///
/// The refusal is what makes a page stall: the run stays non-terminal, so the next read offers it
/// again. Reads delegate to the inner double, so the offered set is one the real path produces
/// rather than one forged here.
#[derive(Debug, Clone)]
struct StallingWrites {
    inner: InMemoryRepositories,
}

impl StallingWrites {
    async fn new() -> Self {
        let inner = InMemoryRepositories::new();
        // The conversation is seeded first: the double enforces the FK the migration enforces, so a
        // run in an unknown conversation is refused with the same `Conflict { what: "run" }` a
        // duplicate id gives — which is what made the first version of this fixture fail for a
        // reason that had nothing to do with stalling.
        inner
            .create_conversation(
                NewConversation::new(
                    conversation(),
                    workspace(),
                    PrincipalId::from_uuid(id(3)),
                    None,
                    "cli".to_owned(),
                    now(),
                )
                .expect("valid"),
            )
            .await
            .expect("the conversation is created");
        Self { inner }
    }

    fn inner(&self) -> &InMemoryRepositories {
        &self.inner
    }

    /// Creates a run through the inner double's real path, so its stored shape is one the
    /// production path produces.
    async fn seed_interrupted(&self, run: RunId) {
        self.inner
            .create(
                NewRun::new(
                    run,
                    workspace(),
                    conversation(),
                    PrincipalId::from_uuid(id(3)),
                    None,
                    now(),
                )
                .expect("valid"),
                run_received_event(run, now()),
            )
            .await
            .expect("the run is created");
    }
}

impl RunRepository for StallingWrites {
    fn create(
        &self,
        run: NewRun,
        opening_event: NewActivityEvent,
    ) -> crate::repository::RepositoryFuture<'_, ()> {
        self.inner.create(run, opening_event)
    }

    fn load(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::StoredRun> {
        self.inner.load(workspace, run)
    }

    fn transition(
        &self,
        _workspace: WorkspaceId,
        _write: RunWrite<'_>,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::StoredRun> {
        // A conflict rather than a fault, which is what a competing writer produces. The recovery
        // write can never land, so the run stays in exactly the state the next read looks for.
        //
        // The yield is load-bearing. Without it the returned future resolves on first poll, so a
        // pass that wrongly continued on a stalled page would never yield — and
        // `tokio::time::timeout` **cannot preempt a future that does not await**, on the
        // current-thread runtime a `#[tokio::test]` uses. A mutation of the stall branch therefore
        // presented as a test binary that ran for ever rather than as a failing assertion, which is
        // the worst way to learn about a wrong loop condition. Yielding once per refused write gives
        // the timer a chance to fire, so the same mutation fails as a timeout with a message.
        Box::pin(async {
            tokio::task::yield_now().await;
            Err(crate::repository::RepositoryError::Conflict {
                what: "recovery_stalls",
            })
        })
    }

    fn next_event_sequence(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> crate::repository::RepositoryFuture<'_, u64> {
        self.inner.next_event_sequence(workspace, run)
    }

    fn append_event(
        &self,
        workspace: WorkspaceId,
        event: NewActivityEvent,
    ) -> crate::repository::RepositoryFuture<'_, u64> {
        self.inner.append_event(workspace, event)
    }

    fn load_events(
        &self,
        workspace: WorkspaceId,
        run: RunId,
        from_sequence: u64,
        limit: u32,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RunEventPage> {
        self.inner.load_events(workspace, run, from_sequence, limit)
    }

    fn claim_idempotency(
        &self,
        record: crate::repository::run::NewIdempotencyRecord,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::IdempotencyClaim> {
        self.inner.claim_idempotency(record)
    }

    fn lookup_idempotency(
        &self,
        workspace: WorkspaceId,
        operation: &str,
        key: &str,
    ) -> crate::repository::RepositoryFuture<'_, Option<(String, RunId)>> {
        self.inner.lookup_idempotency(workspace, operation, key)
    }

    fn create_run_idempotent(
        &self,
        run: NewRun,
        opening_event: NewActivityEvent,
        record: crate::repository::run::NewIdempotencyRecord,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::IdempotencyClaim> {
        self.inner.create_run_idempotent(run, opening_event, record)
    }

    fn incomplete_runs(
        &self,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RecoveryPage> {
        self.inner.incomplete_runs()
    }

    fn load_for_resume(
        &self,
        workspace: WorkspaceId,
        run: RunId,
    ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RunResumeState> {
        self.inner.load_for_resume(workspace, run)
    }
}

#[tokio::test]
async fn recovery_publishes_exactly_one_event_and_closes_the_run() {
    // A recovered run must be followable: a client that reconnects sees the terminal
    // event, which is what tells it to stop waiting.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(400));
    run_in(&repositories, run_id, RunState::Planning).await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    reconcile(&runs, now()).await.expect("runs");

    let page = repositories
        .load_events(workspace(), run_id, 1, 100)
        .await
        .expect("readable");
    assert_eq!(
        page.terminal_state,
        Some(RunState::Failed),
        "a recovered run must read as terminal to a stream",
    );
    let terminals = page
        .events
        .iter()
        .filter(|event| event.event_type == "run.failed")
        .count();
    assert_eq!(terminals, 1, "{:?}", page.events);
    // The sequence continues from the run's own events rather than restarting: the
    // planning path published 1 and 2, this path published 3, and the recovery is 4.
    let last = page.events.last().expect("at least the recovery event");
    assert_eq!(last.sequence, 4, "{:?}", page.events);
}

#[tokio::test]
async fn a_recovery_pass_is_idempotent() {
    // Running twice must change nothing the second time, because the first pass left
    // every run terminal and a second pass has nothing to find.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    run_in(
        &repositories,
        RunId::from_uuid(id(500)),
        RunState::AwaitingModel,
    )
    .await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let first = reconcile(&runs, now()).await.expect("runs");
    assert_eq!(first.summary.total(), 1);
    let second = reconcile(&runs, now()).await.expect("runs");
    assert!(
        second.summary.is_empty(),
        "a second pass must find nothing: {second:?}",
    );
    assert_eq!(second.examined(), 0);
}

#[test]
fn the_paging_decision_stops_on_a_stalled_page_and_continues_on_a_changing_one() {
    // The decision inline in the pass, because both of its dangerous answers are hard to reach:
    //
    //   - continuing on a stalled page is an **infinite loop on the startup path**. A page whose
    //     writes all failed leaves its runs non-terminal, which is what the read looks for, so
    //     "read again" would read the same page for ever.
    //   - stopping on a changing page **strands runs**: interrupted runs come oldest-first, so the
    //     ones a stopped pass leaves behind are the newest, and they stay non-terminal across every
    //     restart because each pass recovers the same oldest page.
    //
    // A pure function is the only way to assert the first without hanging the suite; the second is
    // asserted end to end by `recovery_drains_a_store_holding_more_than_one_page`.
    assert_eq!(super::page_outcome(false, 0), super::PageOutcome::Drained);
    assert_eq!(super::page_outcome(false, 7), super::PageOutcome::Drained);
    assert_eq!(super::page_outcome(true, 0), super::PageOutcome::Stalled);
    assert_eq!(super::page_outcome(true, 1), super::PageOutcome::More);
    // And the property that makes the two safe together: a stalled page is never `Drained`, so a
    // caller cannot mistake "nothing changed" for "nothing left".
    assert_ne!(super::page_outcome(true, 0), super::PageOutcome::Drained);
}

#[tokio::test]
async fn recovery_drains_a_store_holding_more_than_one_page() {
    // **The defect this test exists for.** The store reads interrupted runs one bounded page at a
    // time, oldest-first, and the pass used to read **one** page and stop. A profile holding more
    // than `MAX_INCOMPLETE_RUNS` interrupted runs would therefore have its *newest* ones left
    // non-terminal — on this restart and every later one, because each pass recovered the same
    // oldest page and reported success. That is precisely the state this pass exists to prevent.
    //
    // The fixture must exceed the bound, which makes this the one slow test in the module. It is
    // worth the cost: the property is **invisible** to a page-sized fixture, which the old
    // single-page read passed with.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let overflowing = MAX_INCOMPLETE_RUNS as usize + 3;
    for index in 0..overflowing {
        run_in(
            &repositories,
            RunId::from_uuid(id(100_000 + u128::try_from(index).expect("small"))),
            // `AwaitingModel` rather than a waiting state, so every run is *abandoned* and one
            // number covers them all.
            RunState::AwaitingModel,
        )
        .await;
    }
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = reconcile(&runs, now()).await.expect("runs");
    assert_eq!(
        report.summary.abandoned,
        u64::try_from(overflowing).expect("small"),
        "every interrupted run must be recovered, not one page: {report:?}",
    );
    assert!(report.is_complete(), "{report:?}");
    assert!(!report.incomplete_store, "{report:?}");

    // The assertion that makes the count mean something: the **newest** run must have moved. A
    // total alone would be satisfied by an implementation that recovered one page and then
    // recounted, and the newest are exactly the runs an oldest-first single page hides.
    let remaining = repositories.incomplete_runs().await.expect("readable");
    assert!(remaining.runs.is_empty(), "{} left", remaining.runs.len());
    let newest = repositories
        .load(
            workspace(),
            RunId::from_uuid(id(100_000 + u128::try_from(overflowing - 1).expect("small"))),
        )
        .await
        .expect("loads");
    assert!(
        newest.is_terminal(),
        "the newest interrupted run must not be the one left behind: {newest:?}",
    );
}

#[tokio::test]
async fn a_run_recovered_in_one_workspace_is_found_even_when_others_exist() {
    // The read is deliberately unscoped: a recovery pass must find every interrupted run
    // in the profile, and scoping it to one workspace would leave the others
    // non-terminal forever.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    // A second workspace with its own conversation and run.
    let other_workspace = WorkspaceId::from_uuid(id(600));
    let other_conversation = ConversationId::from_uuid(id(601));
    repositories
        .create_conversation(
            NewConversation::new(
                other_conversation,
                other_workspace,
                PrincipalId::from_uuid(id(602)),
                None,
                "cli".to_owned(),
                now(),
            )
            .expect("valid"),
        )
        .await
        .expect("created");
    let other_run = RunId::from_uuid(id(603));
    repositories
        .create(
            NewRun::new(
                other_run,
                other_workspace,
                other_conversation,
                PrincipalId::from_uuid(id(602)),
                None,
                now(),
            )
            .expect("valid"),
            run_received_event(other_run, now()),
        )
        .await
        .expect("created");
    run_in(&repositories, RunId::from_uuid(id(604)), RunState::Planning).await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    let report = reconcile(&runs, now()).await.expect("runs");
    assert_eq!(
        report.summary.total(),
        2,
        "both workspaces' runs must be recovered: {report:?}",
    );
    // Each run is read back in its **own** workspace, which is also the scope the pass
    // had to name when it wrote the recovery.
    let recovered = repositories
        .load(other_workspace, other_run)
        .await
        .expect("the other workspace's run loads");
    assert_eq!(recovered.state, RunState::Failed);
}

#[tokio::test]
async fn the_recovery_event_payload_names_the_classification_and_the_state() {
    // The payload is what an operator reads to tell "was parked" from "lost work", so it
    // must carry both facts and must not carry any caller text.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(700));
    run_in(&repositories, run_id, RunState::ExecutingTool).await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());

    reconcile(&runs, now()).await.expect("runs");

    let events = repositories.recorded_events().expect("readable");
    let recovery = events
        .iter()
        .find(|event| event.event_type == "run.failed")
        .expect("a recovery event exists");
    let payload = recovery.payload_json.as_deref().expect("a payload");
    assert!(
        payload.contains(r#""classification":"abandoned""#),
        "{payload}"
    );
    assert!(
        payload.contains(r#""was_in":"executing_tool""#),
        "{payload}"
    );
    assert!(
        payload.contains(r#""code":"run.interrupted_by_restart""#),
        "{payload}",
    );
    // A fixed shape: four keys, so nothing else can appear in a public payload.
    let parsed: serde_json::Value = serde_json::from_str(payload).expect("valid JSON");
    assert_eq!(parsed.as_object().expect("object").len(), 4, "{payload}");
    assert_eq!(parsed["parked_in"], serde_json::Value::Null, "{payload}");
}

#[tokio::test]
async fn a_read_failure_is_reported_rather_than_looking_like_a_clean_pass() {
    // A pass that could not read must not report "nothing to do", because a caller would
    // conclude the profile was clean while interrupted runs sat unrecovered.
    struct Unreadable;

    impl RunRepository for Unreadable {
        fn create(
            &self,
            _run: NewRun,
            _opening_event: NewActivityEvent,
        ) -> crate::repository::RepositoryFuture<'_, ()> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn load(
            &self,
            _workspace: WorkspaceId,
            _run: RunId,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::StoredRun> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn load_for_resume(
            &self,
            _workspace: WorkspaceId,
            _run: RunId,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RunResumeState>
        {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn transition<'a>(
            &'a self,
            _workspace: WorkspaceId,
            _write: RunWrite<'a>,
        ) -> crate::repository::RepositoryFuture<'a, crate::repository::run::StoredRun> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn next_event_sequence(
            &self,
            _workspace: WorkspaceId,
            _run: RunId,
        ) -> crate::repository::RepositoryFuture<'_, u64> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn append_event(
            &self,
            _workspace: WorkspaceId,
            _event: NewActivityEvent,
        ) -> crate::repository::RepositoryFuture<'_, u64> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn load_events(
            &self,
            _workspace: WorkspaceId,
            _run: RunId,
            _from_sequence: u64,
            _limit: u32,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RunEventPage> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn claim_idempotency(
            &self,
            _record: crate::repository::run::NewIdempotencyRecord,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::IdempotencyClaim>
        {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn lookup_idempotency(
            &self,
            _workspace: WorkspaceId,
            _operation: &str,
            _key: &str,
        ) -> crate::repository::RepositoryFuture<'_, Option<(String, RunId)>> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn create_run_idempotent(
            &self,
            _run: NewRun,
            _opening_event: NewActivityEvent,
            _record: crate::repository::run::NewIdempotencyRecord,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::IdempotencyClaim>
        {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
        fn incomplete_runs(
            &self,
        ) -> crate::repository::RepositoryFuture<'_, crate::repository::run::RecoveryPage> {
            Box::pin(async { Err(crate::repository::RepositoryError::Query) })
        }
    }

    let runs: Arc<dyn RunRepository> = Arc::new(Unreadable);
    let error = reconcile(&runs, now())
        .await
        .expect_err("an unreadable store is not a clean pass");
    assert_eq!(
        error,
        RecoveryError::Read(crate::repository::RepositoryError::Query)
    );
    assert_eq!(error.code(), "storage.query_failed");
}

#[tokio::test]
async fn a_run_that_finished_between_the_read_and_the_write_is_not_overwritten() {
    // The guard that makes this safe to run against a live database: the transition
    // carries the version it read, so a run that advanced in between is refused rather
    // than having a real outcome replaced with a failure.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(800));
    run_in(&repositories, run_id, RunState::Responding).await;

    // A stale write, computed from the version the pass would have read, against a run
    // that has since completed.
    let read = repositories.load(workspace(), run_id).await.expect("loads");
    let completed = jarvis_domain::run::lifecycle::RunTransition::new(
        RunState::Responding,
        RunState::Completed,
        read.version,
        TransitionActor::Controller,
        TransitionReason::new("answer_delivered").expect("valid"),
        now(),
    );
    repositories
        .transition(
            workspace(),
            RunWrite::new(
                &completed,
                NewActivityEvent {
                    run_id,
                    sequence: 7,
                    event_type: "run.completed".to_owned(),
                    payload_json: None,
                    visibility: crate::repository::run::EventVisibility::Public,
                    occurred_at: now(),
                },
            ),
        )
        .await
        .expect("applies");

    // The recovery write is now stale, so it is refused.
    let stale = jarvis_domain::run::lifecycle::RunTransition::new(
        RunState::Responding,
        RunState::Failed,
        read.version,
        TransitionActor::Supervisor,
        TransitionReason::new("interrupted_while_working").expect("valid"),
        now(),
    );
    let error = repositories
        .transition(
            workspace(),
            RunWrite::new(
                &stale,
                NewActivityEvent {
                    run_id,
                    sequence: 8,
                    event_type: "run.failed".to_owned(),
                    payload_json: None,
                    visibility: crate::repository::run::EventVisibility::Public,
                    occurred_at: now(),
                },
            )
            // A `Failed` target must carry its outcome, so this stale write declares one even
            // though the write is refused for a different reason. Without it the consistency
            // check fires first and the test would assert that refusal instead — which is a true
            // fact about a *malformed* write rather than the ordering this test is about.
            .failed_with(crate::repository::run::TerminalOutcome::failed(
                "run.interrupted_by_restart",
            )),
        )
        .await
        .expect_err("a stale recovery must be refused");
    // The run is terminal, so the domain's ordered refusals answer "already terminal"
    // rather than "stale version". That is the more specific true fact: the run finished,
    // and reporting a version conflict would suggest retrying with a fresh read could
    // work when the run is in fact done. The premise of this test was originally the
    // other way round, and the ordering is what corrected it.
    assert_eq!(error.code(), "storage.transition_refused");

    let after = repositories.load(workspace(), run_id).await.expect("loads");
    assert_eq!(
        after.state,
        RunState::Completed,
        "a finished run must keep its real outcome",
    );
}

#[tokio::test]
async fn a_recovery_event_identifier_is_available_for_the_stream() {
    // A recovered run's events must be resumable like any other, which requires each to
    // carry a globally unique identifier a client can echo back.
    let repositories = InMemoryRepositories::new();
    seed_conversation(&repositories).await;
    let run_id = RunId::from_uuid(id(900));
    run_in(&repositories, run_id, RunState::Planning).await;
    let runs: Arc<dyn RunRepository> = Arc::new(repositories.clone());
    reconcile(&runs, now()).await.expect("runs");

    let page = repositories
        .load_events(workspace(), run_id, 1, 100)
        .await
        .expect("readable");
    for event in &page.events {
        assert!(
            !event.id.is_nil(),
            "every event needs a usable identifier: {event:?}",
        );
        // The identifier round-trips, which is what a `Last-Event-ID` resume depends on.
        let parsed = RunActivityEventId::parse(&event.id.to_string()).expect("round-trips");
        assert_eq!(parsed, event.id);
    }
}
