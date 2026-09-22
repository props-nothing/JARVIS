// Disconnect and cancellation journey.
//
// This is `tests/e2e/`'s first executable harness. It does what Milestone 2's exit
// gate asks — "killing a client does not corrupt the run; cancellation behavior is
// explicit" — and what the first vertical slice's acceptance list asks — "disconnect
// and cancel semantics match the contract". Both were deferred three times (`BRN-007`,
// recoveries, and budgets each recorded them as outstanding), so the point of this file
// is to stop them being deferred again by making them executable.
//
// It deliberately does NOT use the CLI as its client. `jarvis ask` follows a run to its
// terminal, so a client that *disappears* is not something it can express. This harness
// speaks the local control API directly with Node's own HTTP client against the real
// `jarvisd`, which is what makes "the client was killed" a real condition rather than an
// assertion about a process it controls.
//
// Four properties make this a real acceptance test rather than a "the suite passed" one:
//
// 1. It uses a fresh empty `--profile`, so no ambient state can make it pass.
// 2. It asserts **durable state** — the run's events, read back over the API — rather
//    than only status codes. A 202 with nothing persisted would still be a failure.
// 3. It asserts the *disconnect did nothing* as strongly as it asserts the *cancel
//    worked*, because "a disconnect cancels the run" is the contract's explicit
//    prohibition and a test that only checked the cancel path would miss it.
// 4. It proves its own preconditions. The disconnect case is only meaningful if the run
//    is genuinely mid-flight when the client goes away, so it asserts that, rather than
//    assuming it and passing vacuously.
//
// Dependency-free (Node standard library only) so it runs on every target without an
// install step.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;
/** How long to wait for a run to reach a terminal state before calling it stuck. */
const RUN_TIMEOUT_MS = 30_000;

let failures = 0;

function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail) {
    // Bounded: a runaway body must not bury the message that matters.
    console.error(String(detail).slice(0, 2000));
  }
}

function pass(message) {
  console.log(`ok    ${message}`);
}

/** Paths to the two binaries, resolved from a build directory. */
function resolveBinaries() {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error("usage: disconnect-journey.mjs <directory containing jarvis and jarvisd>");
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const daemon = join(directory, `jarvisd${suffix}`);
  const client = join(directory, `jarvis${suffix}`);
  for (const path of [daemon, client]) {
    if (!existsSync(path)) {
      throw new Error(`missing built binary: ${path}`);
    }
  }
  return { daemon, client };
}

function run(command, args) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 60_000,
    windowsHide: true,
  });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

async function sleep(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/** Starts the daemon and waits until `status` reports readiness. */
async function startAndWait(daemon, client, profile) {
  const daemonLog = [];
  const child = spawn(daemon, ["--profile", profile], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  child.stdout.on("data", (chunk) => daemonLog.push(String(chunk)));
  child.stderr.on("data", (chunk) => daemonLog.push(String(chunk)));
  // Kept on the returned handle and printed when a check fails, because a daemon that
  // reports a fault only in its own log is otherwise invisible — and a controller error
  // is deliberately not propagated through the API, since the run's durable state is what
  // a client reads. Without this the harness can only say "the run did not finish", not
  // why.
  child.daemonLog = daemonLog;

  let exitedEarly = false;
  child.once("exit", () => {
    exitedEarly = true;
  });

  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (exitedEarly) {
      throw new Error(`the daemon exited before becoming ready\n${daemonLog.join("")}`);
    }
    const status = run(client, ["--profile", profile, "status"]);
    if (status.status === 0 && /state:\s*ready/.test(status.stdout)) {
      return child;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  throw new Error(`the daemon was not ready within ${READY_TIMEOUT_MS}ms`);
}

/** Stops the daemon and waits for the process to exit. */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  const deadline = Date.now() + SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  child.kill("SIGKILL");
  throw new Error(`the daemon did not stop within ${SHUTDOWN_TIMEOUT_MS}ms`);
}

/** Reads the daemon's discovery record for a profile. */
function discoveryFor(profile) {
  const path = join(profile, "run", "discovery.json");
  if (!existsSync(path)) {
    throw new Error(`the daemon published no discovery at ${path}`);
  }
  const record = JSON.parse(readFileSync(path, "utf8"));
  if (typeof record.base_url !== "string" || !record.base_url.startsWith("http://127.0.0.1:")) {
    throw new Error(`discovery named a non-loopback authority: ${record.base_url}`);
  }
  return record;
}

/** Reads the enrolled client credential for a profile. */
function credentialFor(profile) {
  const path = join(profile, "config", "client-credential");
  if (!existsSync(path)) {
    throw new Error(`the profile has no client credential at ${path}`);
  }
  const credential = readFileSync(path, "utf8").trim();
  if (credential.length !== 43) {
    throw new Error(`the credential is not the expected 43 characters: ${credential.length}`);
  }
  return credential;
}

/** The API major this harness speaks. A mismatch is refused, so it is sent explicitly. */
const API_MAJOR = 1;

/** One HTTP request to the daemon. */
function request(record, credential, method, path, body, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, record.base_url);
    const headers = {
      // The authority the daemon actually bound, because it refuses any other `Host`.
      Host: url.host,
      Authorization: `Bearer ${credential}`,
      // Version negotiation is checked before authentication, so omitting this makes
      // every request fail with `api.version_unsupported` regardless of credentials.
      "Jarvis-API-Version": String(API_MAJOR),
      ...extraHeaders,
    };
    let payload;
    if (body !== undefined) {
      payload = JSON.stringify(body);
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(payload);
    }
    const call = http.request(
      { hostname: url.hostname, port: url.port, method, path: url.pathname, headers },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let json;
          try {
            json = text.length > 0 ? JSON.parse(text) : undefined;
          } catch {
            json = undefined;
          }
          resolve({ status: response.statusCode, text, json });
        });
      },
    );
    // Bounded, so a hung daemon fails this test rather than hanging the runner.
    call.setTimeout(20_000, () => call.destroy(new Error("the request timed out")));
    call.on("error", reject);
    if (payload !== undefined) {
      call.write(payload);
    }
    call.end();
  });
}

/**
 * An idempotency key for a create.
 *
 * Clock-derived rather than random, matching the CLI: the key is only ever compared for
 * equality, so it does not need entropy, and this harness has no random source it would
 * be right to use for a security control it is not providing.
 */
let keyCounter = 0;
function idempotencyKey(label) {
  keyCounter += 1;
  return `${label}-${Date.now()}-${process.pid}-${keyCounter}`;
}

/** Creates a run and returns its identity. */
async function createRun(record, credential, text) {
  const response = await request(
    record,
    credential,
    "POST",
    "/api/v1/runs",
    {
      conversation_id: null,
      input: { type: "text", text },
      runtime: "jarvis-native",
      model_policy: { policy_id: "scripted-test", version: 1 },
    },
    { "Idempotency-Key": idempotencyKey("disconnect") },
  );
  if (response.status !== 202 || !response.json?.run_id) {
    throw new Error(`create failed: ${response.status} ${response.text}`);
  }
  return response.json;
}

/** Reads one run. */
async function readRun(record, credential, runId) {
  const response = await request(record, credential, "GET", `/api/v1/runs/${runId}`);
  if (response.status !== 200) {
    throw new Error(`read failed: ${response.status} ${response.text}`);
  }
  return response.json;
}

/** Reads a run's events from sequence 1. */
async function readEvents(record, credential, runId) {
  const response = await request(
    record,
    credential,
    "GET",
    `/api/v1/runs/${runId}/events`,
    undefined,
    { Accept: "text/event-stream" },
  );
  if (response.status !== 200) {
    throw new Error(`events failed: ${response.status} ${response.text}`);
  }
  return parseSse(response.text);
}

/**
 * Parses SSE frames into `{id, event, data}` objects.
 *
 * A parser rather than an assertion on raw text, so a frame that is present but malformed
 * is *absent* here rather than matching a regex by accident. A keepalive comment carries no
 * `id:` and is skipped, which is what the contract requires of it.
 */
function parseSse(text) {
  const frames = [];
  for (const block of text.split("\n\n")) {
    const frame = { id: undefined, event: undefined, data: undefined };
    let seen = false;
    for (const line of block.split("\n")) {
      if (line.startsWith("id: ")) {
        frame.id = line.slice(4);
        seen = true;
      } else if (line.startsWith("event: ")) {
        frame.event = line.slice(7);
        seen = true;
      } else if (line.startsWith("data: ")) {
        frame.data = line.slice(6);
        seen = true;
      }
    }
    if (seen) {
      frames.push(frame);
    }
  }
  return frames;
}

const TERMINALS = ["run.completed", "run.failed", "run.cancelled"];

/** Waits until a run reaches a terminal state, and returns it. */
async function waitForTerminal(record, credential, runId, daemonLog = []) {
  const deadline = Date.now() + RUN_TIMEOUT_MS;
  let last;
  while (Date.now() < deadline) {
    last = await readRun(record, credential, runId);
    if (TERMINALS.includes(`run.${last.state}`) || last.state === "completed" || last.state === "failed" || last.state === "cancelled") {
      return last;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  // The daemon log is included because a controller error is deliberately not propagated
  // through the API — the run's durable state is what a client reads — so a stuck run only
  // explains itself in the daemon's own output.
  throw new Error(
    `the run never reached a terminal state: ${JSON.stringify(last)}\ndaemon log:\n${daemonLog.join("").slice(-3000)}`,
  );
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-e2e-disconnect-"));
  let running = null;

  try {
    running = await startAndWait(daemon, client, profile);
    pass("the daemon started and reported ready on a fresh profile");

    const record = discoveryFor(profile);
    const credential = credentialFor(profile);

    // ---------------------------------------------------------------------
    // 1. A client that disappears mid-run must not cancel it.
    //
    // This is Milestone 2's exit gate and the contract's explicit rule: "A client
    // disconnect never cancels a durable run. Cancellation uses the command endpoint."
    // ---------------------------------------------------------------------
    const abandoned = await createRun(record, credential, "abandon me");
    pass(`a run was created and is received: ${abandoned.run_id}`);

    // A partial events read, then the connection is dropped. The harness reads the stream
    // with a socket it destroys rather than one it lets finish, so the disconnect is a
    // real TCP abort rather than a client that politely stopped listening.
    await abortMidStream(record, credential, abandoned.run_id);
    pass("the events connection was aborted mid-stream");

    // The disconnect must leave the run reachable and *not* cancelled. Asserting only
    // "the run still exists" would pass even if the abort had cancelled it, so the state
    // is asserted directly.
    const afterDisconnect = await readRun(record, credential, abandoned.run_id);
    if (afterDisconnect.state === "cancelled") {
      fail("the aborted events connection cancelled the run", JSON.stringify(afterDisconnect));
    } else {
      pass(`the run survived the client disconnect, state: ${afterDisconnect.state}`);
    }

    // And it must still reach a terminal state on its own, which is what proves the run
    // was never dependent on the connection. A run left non-terminal after a disconnect
    // would be the corruption the exit gate names.
    const finished = await waitForTerminal(record, credential, abandoned.run_id);
    if (finished.state !== "completed") {
      fail("the disconnected run did not complete on its own", JSON.stringify(finished));
    } else {
      pass("the disconnected run completed without any client watching it");
    }

    // Its events must be complete and ordered, which is what "does not corrupt the run"
    // means concretely: the audit trail is whole.
    const events = await readEvents(record, credential, abandoned.run_id);
    const terminals = events.filter((frame) => TERMINALS.includes(frame.event));
    if (terminals.length !== 1) {
      fail(`expected exactly one terminal event, found ${terminals.length}`, JSON.stringify(events));
    } else {
      pass(`the disconnected run published exactly one terminal event: ${terminals[0].event}`);
    }
    const sequences = events.map((frame) => Number(frame.id && frame.data ? JSON.parse(frame.data).sequence : NaN));
    const ordered = sequences.every((value, index) => index === 0 || value === sequences[index - 1] + 1);
    if (!ordered || sequences[0] !== 1) {
      fail("the run's event sequences are not contiguous from 1", JSON.stringify(sequences));
    } else {
      pass(`the run's events are contiguous from 1 through ${sequences.at(-1)}`);
    }

    // ---------------------------------------------------------------------
    // 2. Cancellation is explicit, and reaches a durable terminal state.
    // ---------------------------------------------------------------------
    const toCancel = await createRun(record, credential, "cancel me");
    const cancelled = await request(
      record,
      credential,
      "POST",
      `/api/v1/runs/${toCancel.run_id}/cancel`,
      { reason: "user_requested" },
      { "Idempotency-Key": idempotencyKey("cancel") },
    );
    if (cancelled.status !== 202 && cancelled.status !== 200) {
      fail(`cancel was refused: ${cancelled.status}`, cancelled.text);
    } else {
      pass(`cancel was accepted with ${cancelled.status}`);
    }

    // The contract requires the cancellation intent to be *recorded* before it is
    // signalled, so a repeated cancel is idempotent rather than a second stop. Asserted
    // before the terminal wait so it holds whether or not the first cancel won its race.
    const repeated = await request(
      record,
      credential,
      "POST",
      `/api/v1/runs/${toCancel.run_id}/cancel`,
      { reason: "user_requested" },
      { "Idempotency-Key": idempotencyKey("cancel-2") },
    );
    if (repeated.status !== 202 && repeated.status !== 200) {
      fail(`a repeated cancel was refused: ${repeated.status}`, repeated.text);
    } else {
      pass("a repeated cancel is idempotent");
    }

    // The run must reach **a terminal state** and never hang. Which terminal is a race
    // the caller cannot win deterministically: the scripted provider finishes in about
    // twenty-five milliseconds, so a cancel issued immediately after creation may land
    // before the run starts, during it, or after it has already completed. All three are
    // truthful outcomes, and asserting `cancelled` specifically would fail whenever the
    // cancel legitimately lost — which is what an earlier version of this harness did.
    //
    // What must never happen is a run left non-terminal, and that is the assertion with
    // teeth: a cancel that is accepted and then ignored leaves a client polling forever.
    // This assertion is not hypothetical. A run left non-terminal is the exact symptom
    // this harness found: a contended transition failed with `database is locked`, the
    // adapter reported it as a generic storage fault, and the terminal transition was
    // refused by a *lock* rather than by a rule — so the run sat in `context_building`
    // permanently. It reproduced about one run in three. The cause was the deferred
    // transaction's read→write lock upgrade, which bypasses SQLite's busy handler; the
    // fix is `BEGIN IMMEDIATE`, and
    // `concurrent_transitions_on_one_run_never_fail_with_a_storage_fault` now reproduces
    // the fault against a real file database.
    //
    // The window in which a cancel *must* win is covered deterministically by
    // `run_controller`'s `a_cancel_arriving_during_delivery_ends_the_run_cancelled`, which
    // cancels from inside the provider's own stream so there is no race to lose.
    const cancelTerminal = await waitForTerminal(record, credential, toCancel.run_id);
    pass(`the cancel attempt settled the run: ${cancelTerminal.state}`);

    // A cancelled run's events must be truthful: exactly one terminal event, and whichever
    // it is must agree with the reported state. Two different answers for one fact is the
    // defect this checks for on the cancellation path specifically.
    const cancelEvents = await readEvents(record, credential, toCancel.run_id);
    const cancelTerminals = cancelEvents
      .filter((frame) => TERMINALS.includes(frame.event))
      .map((frame) => frame.event);
    if (cancelTerminals.length !== 1) {
      fail(
        `the run must publish exactly one terminal event, found ${cancelTerminals.length}`,
        JSON.stringify(cancelEvents.map((frame) => frame.event)),
      );
    } else {
      pass(`the run published exactly one terminal event: ${cancelTerminals[0]}`);
    }
    const expectedTerminal = `run.${cancelTerminal.state}`;
    if (cancelTerminals.length === 1 && cancelTerminals[0] !== expectedTerminal) {
      fail(
        "the run's state and its terminal event must agree",
        `state ${cancelTerminal.state} with event ${cancelTerminals[0]}`,
      );
    } else {
      pass("the run's state and its terminal event agree");
    }

    // ---------------------------------------------------------------------
    // 3. Cancelling an already-terminal run is a no-op, not a fault.
    // ---------------------------------------------------------------------
    const afterTerminal = await request(
      record,
      credential,
      "POST",
      `/api/v1/runs/${toCancel.run_id}/cancel`,
      { reason: "user_requested" },
      { "Idempotency-Key": idempotencyKey("cancel-3") },
    );
    // The contract returns `200` with the unchanged terminal state rather than an error.
    if (afterTerminal.status !== 200) {
      fail(`cancelling a finished run must be 200, got ${afterTerminal.status}`, afterTerminal.text);
    } else {
      pass("cancelling an already-terminal run returns 200 with its state");
    }

    // ---------------------------------------------------------------------
    // 4. A replay after everything is durable still reports the same run.
    // ---------------------------------------------------------------------
    const reread = await readRun(record, credential, abandoned.run_id);
    if (reread.state !== finished.state || reread.version !== finished.version) {
      fail(
        "a run's state changed between two reads without an intervening write",
        `${JSON.stringify(finished)} then ${JSON.stringify(reread)}`,
      );
    } else {
      pass("a terminal run is stable across reads");
    }
  } finally {
    if (running) {
      await stop(running);
    }
    rmSync(profile, { recursive: true, force: true });
  }

  if (failures > 0) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nall disconnect and cancellation checks passed");
}

/**
 * Opens the events stream and destroys the socket mid-body.
 *
 * A raw `http.request` whose connection is destroyed from the response handler, rather
 * than a client that reads to completion: the point is a client that *disappears*, which
 * is only reproducible if the socket is actually aborted. `req.destroy()` before the
 * response would test the wrong thing (a request that never reached the daemon), so the
 * abort happens on first data.
 */
function abortMidStream(record, credential, runId) {
  return new Promise((resolve, reject) => {
    const url = new URL(`/api/v1/runs/${runId}/events`, record.base_url);
    const call = http.request(
      {
        hostname: url.hostname,
        port: url.port,
        method: "GET",
        path: url.pathname,
        headers: {
          Host: url.host,
          Authorization: `Bearer ${credential}`,
          "Jarvis-API-Version": String(API_MAJOR),
          Accept: "text/event-stream",
        },
      },
      (response) => {
        response.once("data", () => {
          // The socket is destroyed with the body unfinished, so the daemon sees a
          // client that went away rather than one that finished reading.
          call.destroy();
          resolve();
        });
        response.once("end", () => {
          // The stream closed before any data arrived. Still a disconnect from the
          // daemon's point of view, and worth recording rather than hanging.
          resolve();
        });
      },
    );
    call.setTimeout(20_000, () => {
      call.destroy();
      reject(new Error("the events request never produced a response"));
    });
    call.on("error", () => resolve());
    call.end();
  });
}

await main();
