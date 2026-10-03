//! Continuing a run that was parked on an approval.
//!
//! The run's tool batch stopped at a call that needed a human; the batch, the observations that had
//! already settled, and the approval are in the resume store. Once the approval is decided this
//! releases the waiting call through the same governed pipeline, dispatches whatever follows it, and
//! hands the observations to the ordinary turn loop — so a run that waited and a run that did not take
//! the same path from the moment its tools have settled.

use super::{
    CancellationScope, CapturedToolCall, ControllerError, DispatchOutcome, DispatchPlan,
    DurableApproval, ParkedBatch, RequestContext, RunController, RunOutcome, RunRef, RunState,
    Step, TurnInputs, TurnStart,
};
use crate::repository::resume::ResumeRecord;
use jarvis_domain::ids::RunId;

impl RunController {
    /// Cancels a run that is parked on an approval.
    ///
    /// A parked run has no task whose scope a cancel could signal, so the transition is made here
    /// directly. The resume record is discarded with the wait.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError`] when the run cannot be read or moved — including when it is no
    /// longer parked, which the caller treats as having lost a race with a decision.
    pub async fn cancel_parked(
        &self,
        workspace: jarvis_domain::ids::WorkspaceId,
        run_id: RunId,
        requester_reason: &str,
    ) -> Result<(), ControllerError> {
        let run = RunRef { workspace, run_id };
        self.finish(
            run,
            Step::cancelled_from(RunState::AwaitingApproval, "cancelled_while_waiting")
                .with_requester_reason(Some(requester_reason.to_owned())),
        )
        .await?;
        if let Some(resumes) = self.resumes.as_ref() {
            let _ = resumes.discard(workspace, run_id).await;
        }
        Ok(())
    }

    /// Continues `run_id` after `approval` was decided, if the run is waiting on exactly it.
    ///
    /// Returns `Ok(None)` when there is nothing to continue — the run is not waiting, was already
    /// continued, is waiting on a different approval, or this controller has no resume store. That
    /// is an answer, not a fault: a decision may be repeated, and only the first one moves the run.
    ///
    /// **The first action is the transition out of `AwaitingApproval`**, which is optimistic on the
    /// run's version, so two decisions arriving together cannot both dispatch the call: one wins the
    /// transition and the other is refused before anything runs.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerError`] for a store or provider fault, or
    /// [`ControllerError::ResumeStateMissing`] when the run was parked with nothing to continue from
    /// (it is failed rather than left waiting).
    pub async fn resume(
        &self,
        context: &RequestContext,
        run_id: RunId,
        approval: &DurableApproval,
        cancel: &CancellationScope,
    ) -> Result<Option<RunOutcome>, ControllerError> {
        let run = RunRef {
            workspace: context.workspace_id,
            run_id,
        };
        let Some(record) = self.parked_record(run, approval).await? else {
            return Ok(None);
        };

        // The context is rebuilt **before** the run leaves the wait, so a failure to rebuild it is a
        // failure from the state the run is actually in.
        let (budget, assembled) = self
            .build_context(
                run,
                record.conversation,
                &record.objective,
                record.objective_message,
                RunState::AwaitingApproval,
            )
            .await?;
        let model = match self.resolve_model(run).await {
            Ok(model) => model,
            Err(error) => {
                self.finish(
                    run,
                    Step::failed(
                        RunState::AwaitingApproval,
                        "run.failed",
                        "no_model_served",
                        error.code(),
                    ),
                )
                .await?;
                return Err(error);
            }
        };

        self.step_unless_cancelled(
            run,
            Step::new(
                RunState::AwaitingApproval,
                RunState::ExecutingTool,
                "run.tool_executing",
                "approval_decided",
            ),
            cancel,
        )
        .await?;

        let inputs = TurnInputs {
            context,
            conversation_id: record.conversation,
            objective: &record.objective,
            objective_message: record.objective_message,
            items: &assembled.items,
            model: &model,
            budget: &budget,
            cancel,
        };
        self.continue_batch(run, &inputs, approval, &record)
            .await
            .map(Some)
    }

    /// Reads the record a run parked with, when `approval` is the decision it is waiting on.
    ///
    /// `None` is every "nothing to do" answer: no store, a run that is not waiting, or a decision
    /// about a superseded approval — the call asked again after a first prompt, and the old prompt's
    /// decision must not release the call the new one is for.
    async fn parked_record(
        &self,
        run: RunRef,
        approval: &DurableApproval,
    ) -> Result<Option<ResumeRecord>, ControllerError> {
        let Some(resumes) = self.resumes.as_ref() else {
            return Ok(None);
        };
        if self.load(run).await?.state != RunState::AwaitingApproval {
            return Ok(None);
        }
        let Some(record) = resumes
            .load(run.workspace, run.run_id)
            .await
            .map_err(ControllerError::Repository)?
        else {
            self.finish(
                run,
                Step::failed(
                    RunState::AwaitingApproval,
                    "run.failed",
                    "resume_state_missing",
                    ControllerError::ResumeStateMissing.code(),
                ),
            )
            .await?;
            return Err(ControllerError::ResumeStateMissing);
        };
        Ok((record.approval == approval.id).then_some(record))
    }

    /// Releases the waiting call, dispatches what follows it, and hands the result to the turn loop.
    ///
    /// The run is in `ExecutingTool` on entry.
    async fn continue_batch(
        &self,
        run: RunRef,
        inputs: &TurnInputs<'_>,
        approval: &DurableApproval,
        record: &ResumeRecord,
    ) -> Result<RunOutcome, ControllerError> {
        let calls: Vec<CapturedToolCall> = record.calls.iter().map(Into::into).collect();
        let first = usize::try_from(record.waiting_index).unwrap_or(usize::MAX);
        if first >= calls.len() {
            // A record whose waiting index is outside its own batch cannot be continued; it is
            // corrupt rather than something a model should be told about.
            self.finish(
                run,
                Step::failed(
                    RunState::ExecutingTool,
                    "run.failed",
                    "resume_state_missing",
                    ControllerError::ResumeStateMissing.code(),
                ),
            )
            .await?;
            return Err(ControllerError::ResumeStateMissing);
        }
        let dispatched = self
            .dispatch_tools(
                run,
                inputs.context,
                inputs.cancel,
                DispatchPlan {
                    calls: &calls,
                    first,
                    decided: Some(approval),
                    settled: record.settled.iter().map(Into::into).collect(),
                },
            )
            .await?;

        match dispatched {
            // The batch needs another decision — a later call asked, or the approval no longer
            // covered the action. The record is replaced with the new position.
            DispatchOutcome::WaitingApproval {
                approval,
                waiting_index,
                settled,
            } => {
                self.park(
                    run,
                    inputs,
                    record.turn_index,
                    ParkedBatch {
                        approval,
                        calls,
                        waiting_index,
                        settled,
                    },
                )
                .await
            }
            DispatchOutcome::Observations(observations) => {
                // Best effort: a leftover record is garbage rather than a hazard, because the run is
                // no longer waiting and the first thing `resume` checks is that it is.
                if let Some(resumes) = self.resumes.as_ref() {
                    let _ = resumes.discard(run.workspace, run.run_id).await;
                }
                self.step(
                    run,
                    Step::new(
                        RunState::ExecutingTool,
                        RunState::Observing,
                        "run.observing",
                        "tool_calls_settled",
                    ),
                )
                .await?;
                self.drive_turns(
                    run,
                    inputs,
                    TurnStart {
                        turn_index: record.turn_index,
                        observations,
                        after_tools: true,
                    },
                )
                .await
            }
        }
    }
}
