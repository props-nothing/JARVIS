//! The daemon's supervision of runs parked on an approval.
//!
//! A decision continues its run directly, from the request that took it. This loop is the backstop for
//! everything that path cannot cover: a decision whose continuation was lost because the daemon stopped
//! in between, a run still waiting when the daemon restarted, and an approval that lapsed with nobody
//! there to record it. Each sweep asks [`RunService::resume_parked`], which is idempotent, so running it
//! on a timer — and once immediately at start, which is what makes a restart pick up where it left off —
//! cannot move a run twice.
//!
//! It owns no policy: which runs are waiting, which approvals are decided or lapsed, and what a
//! continuation does are the application layer's. This module is the timer and the log line.

use std::sync::Arc;
use std::time::Duration;

use jarvis_application::approval_service::ApprovalService;
use jarvis_application::run_service::{RunService, TokioSpawner};

/// How often parked runs are looked at.
///
/// A decision is continued immediately by its own request, so this only bounds how long a *lost*
/// continuation or an unrecorded lapse can go unnoticed. Ten seconds is long enough that an idle
/// daemon is not polling its database for nothing and short enough that a lapsed prompt does not
/// leave a run waiting visibly past its deadline.
pub const PARKED_RUN_SWEEP_INTERVAL: Duration = Duration::from_secs(10);

/// Sweeps until `stop` is notified. The first sweep runs immediately.
pub async fn supervise_parked_runs(
    runs: Arc<RunService>,
    approvals: Arc<ApprovalService>,
    interval: Duration,
    stop: Arc<tokio::sync::Notify>,
) {
    let spawner = TokioSpawner;
    loop {
        match crate::time::SystemClock::new().now() {
            Ok(at) => match runs.resume_parked(&approvals, &spawner, at).await {
                Ok(sweep) if sweep.resumed > 0 || sweep.failed > 0 || sweep.discarded > 0 => {
                    log::info!(
                        "parked runs: examined={} resumed={} waiting={} discarded={} failed={}",
                        sweep.examined,
                        sweep.resumed,
                        sweep.waiting,
                        sweep.discarded,
                        sweep.failed,
                    );
                }
                Ok(_) => {}
                Err(error) => log::warn!("parked-run sweep could not read: code={}", error.code()),
            },
            Err(_) => log::warn!("parked-run sweep skipped: the clock is unavailable"),
        }
        tokio::select! {
            () = stop.notified() => return,
            () = tokio::time::sleep(interval) => {}
        }
    }
}
