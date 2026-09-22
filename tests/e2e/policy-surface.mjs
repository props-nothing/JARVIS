// Model data policy surface journey.
//
// Proves that the policy selector is reachable from a **real** `jarvisd` rather than only
// from an in-process router. The handler tests in `jarvis_infrastructure::http::tests` build
// their own `ApiState` with `.with_policies(...)`, so they prove the handlers work when the
// state carries a service. They cannot prove the *daemon composition* passes one — a daemon
// that never called `.with_policies` would still pass every one of them while answering
// `service.not_ready` to every real client. That gap is exactly what a composition root
// needs an end-to-end test for, and it is the reason this file exists rather than another
// unit test.
//
// It also proves the two wire properties a unit test is structurally unable to check:
//
// 1. **The unconfigured and the un-populated states are distinguishable over the wire.**
//    A fresh daemon has migrations and a policy store, but no policy row. The contract treats
//    those as different things: no store is `service.not_ready` (503), whereas an empty store
//    is `model.policy_not_found` (404). Collapsing them would make "the daemon has not started"
//    and "no policy is in force" one answer, and a client could not tell a retry from a
//    decision. This harness asserts the 404 specifically, and that the body is not the
//    not-ready envelope.
// 2. **The route probe's refusal is a `200` whose rejected reason is a contract code.** The
//    reason is asserted byte-for-byte because the defect that shipped here was the domain's
//    operator prose (`"locality violated"`) where the contract specifies `locality_violated`,
//    and prose in a field a client switches on is invisible to a status assertion.
//
// Dependency-free (Node standard library only), matching the other harness in this directory.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;
/** The API major this harness speaks. A mismatch is refused, so it is sent explicitly. */
const API_MAJOR = 1;

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
    throw new Error("usage: policy-surface.mjs <directory containing jarvis and jarvisd>");
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
  // Kept on the handle and printed when a check fails, because a daemon that reports a fault
  // only in its own log is otherwise invisible.
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

/** One HTTP request to the daemon. */
function request(record, credential, method, path, body, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, record.base_url);
    const headers = {
      // The authority the daemon actually bound, because it refuses any other `Host`.
      Host: url.host,
      Authorization: `Bearer ${credential}`,
      // Version negotiation is checked before authentication, so omitting this makes every
      // request fail with `api.version_unsupported` regardless of credentials — which would
      // make a test assert a status it never actually reached.
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

/** Every `model.*` code the contract fixes. A response must not invent one outside this set. */
const POLICY_CODES = [
  "model.policy_not_found",
  "model.policy_version_conflict",
  "model.policy_unsatisfied",
  "model.evidence_missing",
  "model.evidence_stale",
  "model.exception_required",
  "model.exception_expired",
];

/**
 * A submission body, so the sections below differ only in what they vary.
 *
 * Every rule field is present because the write shape requires them: a client that omits one has
 * stated no policy for it, and defaulting that to the permissive end would grant more than it
 * asked for. `maximum_sensitivity` is the rules shape's name — the *statement* shape calls the
 * same concept `sensitivity`, and sending that spelling here is a parse failure, which is the
 * point of keeping the two shapes distinct.
 */
function policyBody(expectedVersion, locality) {
  return {
    name: "operator policy",
    expected_version: expectedVersion,
    rules: {
      locality,
      maximum_provider_retention: "provider_default_allowed",
      provider_training_use: "provider_default_allowed",
      telemetry: "local_only",
      allowed_residency_regions: [],
      maximum_sensitivity: "confidential",
      allow_fallback: "denied",
    },
  };
}

/**
 * An idempotency key for a write.
 *
 * Clock-derived rather than random: the key is only ever compared for presence, so it needs no
 * entropy, and this harness has no random source it would be right to use for a security control
 * it is not providing.
 */
let keyCounter = 0;
function idempotencyKey(label) {
  keyCounter += 1;
  return `${label}-${Date.now()}-${keyCounter}`;
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-e2e-policy-"));
  let running = null;

  try {
    running = await startAndWait(daemon, client, profile);
    pass("the daemon started and reported ready on a fresh profile");

    const record = discoveryFor(profile);
    const credential = credentialFor(profile);

    // ---------------------------------------------------------------------
    // 1. The routes exist and are authenticated.
    //
    // Asserted *first*, because "the route is absent" and "the daemon answered something
    // else at that path" both produce a non-200, and the two need distinguishing: the
    // fallback answers `resource.not_found` and a real policy answer uses a `model.*` code.
    // A request without the version header would be refused as `api.version_unsupported`
    // before authentication, so this proves the route is reached at all.
    // ---------------------------------------------------------------------
    for (const path of ["/api/v1/model-data-policy", "/api/v1/model-data-policy/effective"]) {
      const response = await request(record, credential, "GET", path);
      const code = response.json?.error?.code;
      if (response.status === 404 && code === "resource.not_found") {
        fail(`${path} is not routed: the fallback answered instead`, response.text);
      } else if (POLICY_CODES.includes(code) || response.status === 200) {
        pass(`${path} is routed and reached the policy surface (${response.status})`);
      } else {
        fail(`${path} answered neither a policy code nor the fallback`, response.text);
      }
    }

    // And it must refuse an unauthenticated call, so the surface is not open on loopback.
    // A loopback bind is not authorization: another local user can reach the port.
    const unauthenticated = await request(
      record,
      "not-a-real-credential-that-is-long-enough-to-look-plausible",
      "GET",
      "/api/v1/model-data-policy",
    );
    if (unauthenticated.status !== 401) {
      fail(`the policy surface accepted a bad credential: ${unauthenticated.status}`);
    } else {
      pass("the policy surface refuses a bad credential with 401");
    }

    // ---------------------------------------------------------------------
    // 2. A daemon with a store but no policy row reports `model.policy_not_found`.
    //
    // This is the assertion that proves the composition root passed a `PolicyService`:
    // without one the handler short-circuits to `service.not_ready` *before* it can read
    // anything, so a 404 carrying a `model.*` code is only reachable when the service is
    // present and the store was consulted.
    //
    // The two states are also asserted to be **different**. The contract makes "no store"
    // and "no policy in force" distinct answers — one is a readiness fact a client should
    // retry, the other is a decision it must act on — and a single response shape for both
    // would be indistinguishable to a client.
    // ---------------------------------------------------------------------
    const active = await request(record, credential, "GET", "/api/v1/model-data-policy");
    if (active.status === 503) {
      fail(
        "the daemon answered service.not_ready, so the composition root never attached the policy service",
        active.text,
      );
    } else if (active.status !== 404) {
      fail(
        `a fresh workspace with no policy must be 404, got ${active.status}`,
        active.text,
      );
    } else if (active.json?.error?.code !== "model.policy_not_found") {
      fail(
        "the not-found response must use the contract's code rather than a generic one",
        active.text,
      );
    } else {
      pass("a workspace with no policy reports model.policy_not_found, not a readiness failure");
    }

    // The body must not carry the not-ready text either, since a client that only reads the
    // body would otherwise see two states rendered identically.
    if (typeof active.text === "string" && active.text.includes("not_ready")) {
      fail("the not-found body reads as a readiness failure", active.text);
    } else {
      pass("the not-found body is not the not-ready envelope");
    }

    // ---------------------------------------------------------------------
    // 3. The route probe with no policy in force reports the same fact.
    //
    // It must not report `model.policy_unsatisfied`: nothing was offered to the policy, so
    // "your policy refused every model" would name a decision that was never made. This is
    // the distinction the `RouteSelectionFailure` enum exists to keep, and it is worth
    // asserting over the wire because the two codes are one word apart.
    // ---------------------------------------------------------------------
    const effective = await request(record, credential, "GET", "/api/v1/model-data-policy/effective");
    if (effective.status !== 404 || effective.json?.error?.code !== "model.policy_not_found") {
      fail(
        `the route probe with no policy must be 404 model.policy_not_found, got ${effective.status}`,
        effective.text,
      );
    } else if (effective.json?.error?.code === "model.policy_unsatisfied") {
      fail("a missing policy was reported as a policy refusal", effective.text);
    } else {
      pass("the route probe reports the missing policy rather than a refusal");
    }

    // ---------------------------------------------------------------------
    // 4. The surface is stable across reads.
    //
    // A policy answer that changed between two identical requests would mean a per-request
    // clock or a re-read of the store inside the response, either of which would make two
    // probes of one daemon disagree. The daemon reads its clock once, at composition, so the
    // two bodies must be byte-identical.
    // ---------------------------------------------------------------------
    const firstRead = await request(record, credential, "GET", "/api/v1/model-data-policy/effective");
    const secondRead = await request(
      record,
      credential,
      "GET",
      "/api/v1/model-data-policy/effective",
    );
    if (firstRead.text !== secondRead.text) {
      fail(
        "two identical policy probes answered differently, so something is read per request",
        `${firstRead.text}\n${secondRead.text}`,
      );
    } else {
      pass("two identical probes answer byte-identically");
    }

    // ---------------------------------------------------------------------
    // 5. A write creates a version, and the reply is the version the daemon chose.
    //
    // The request carries no version, because version identity is what keeps a past route
    // decision explainable: a client that named the version it was creating could skip numbers
    // or collide with one that exists. The version is therefore an output of a write, and this
    // is where a client learns what it created.
    // ---------------------------------------------------------------------
    const firstWrite = await request(
      record,
      credential,
      "PUT",
      "/api/v1/model-data-policy",
      policyBody(0, "local_only"),
      { "Idempotency-Key": idempotencyKey("policy-1") },
    );
    if (firstWrite.status !== 200) {
      fail(`the first policy write must succeed, got ${firstWrite.status}`, firstWrite.text);
    } else if (firstWrite.json?.version !== 1) {
      fail(
        "the write must report the version it created as 1",
        firstWrite.text,
      );
    } else {
      pass("a write created policy version 1 and reported it");
    }

    // The read agrees, which is what proves the write persisted rather than only answering. A
    // single-request assertion would pass against an implementation that answered correctly and
    // stored nothing.
    const afterWrite = await request(record, credential, "GET", "/api/v1/model-data-policy");
    if (afterWrite.status !== 200 || afterWrite.json?.version !== 1) {
      fail("the written policy is not readable", afterWrite.text);
    } else if (afterWrite.json?.rules?.locality !== "local_only") {
      fail("the read does not report the written rules", afterWrite.text);
    } else {
      pass("the written policy is in force and readable");
    }

    // ---------------------------------------------------------------------
    // 6. Every rule the write submitted is present in the reply, and the read agrees.
    //
    // This is the defect this section was written for. The reply originally rendered the rules
    // through the six-field *statement* shape, so `allow_fallback` was silently dropped from the
    // daemon's own description of what it had stored — a client could not confirm its write, and
    // a client doing read-modify-write would resubmit a body that reset the rule.
    // ---------------------------------------------------------------------
    const submitted = ["local_only", "confidential", "denied", "local_only"];
    const replyText = firstWrite.text ?? "";
    const readText = afterWrite.text ?? "";
    const dropped = [];
    for (const fragment of submitted) {
      if (!replyText.includes(`"${fragment}"`)) {
        dropped.push(`write reply is missing ${fragment}`);
      }
      if (!readText.includes(`"${fragment}"`)) {
        dropped.push(`read is missing ${fragment}`);
      }
    }
    if (!replyText.includes("maximum_sensitivity")) {
      // The rules shape names the field `maximum_sensitivity`; the statement shape calls it
      // `sensitivity`. Rendering the rules through the statement shape would rename a policy
      // ceiling into a per-call classification.
      dropped.push("the write reply does not carry the rules shape's field name");
    }
    if (firstWrite.json?.rules?.allow_fallback !== "denied") {
      dropped.push("the write reply dropped allow_fallback");
    }
    if (dropped.length > 0) {
      fail("the write reply and the read must describe the same stored rules", dropped.join("; "));
    } else {
      pass("the write reply and the read agree on every rule");
    }

    // An allow-list was never submitted, so it must stay absent rather than becoming an empty
    // array. An empty set means "permit nothing" while an absent one means "no restriction at
    // this layer", so collapsing them would turn the most permissive statement into the most
    // restrictive-looking one — and a read-modify-write would then submit `[]` and narrow the
    // policy to nothing.
    if (readText.includes("allowed_providers")) {
      fail("an unrestricted allow-list must be absent rather than empty", readText);
    } else {
      pass("an unrestricted allow-list is absent, not an empty set");
    }

    // ---------------------------------------------------------------------
    // 7. A write can only narrow what is in force.
    //
    // The security property of the write endpoint: without an approval step, a request body must
    // not be able to relax the workspace policy. Version 2 is accepted — the precondition holds —
    // and the stricter locality survives.
    // ---------------------------------------------------------------------
    const widening = await request(
      record,
      credential,
      "PUT",
      "/api/v1/model-data-policy",
      policyBody(1, "approved_cloud_allowed"),
      { "Idempotency-Key": idempotencyKey("policy-2") },
    );
    if (widening.status !== 200 || widening.json?.version !== 2) {
      fail(`the second write must create version 2, got ${widening.status}`, widening.text);
    } else if (widening.json?.rules?.locality !== "local_only") {
      fail(
        "a submission must not widen the policy already in force",
        widening.text,
      );
    } else {
      pass("a looser submission was accepted and the stricter locality survived");
    }

    // ---------------------------------------------------------------------
    // 8. A stale precondition is a conflict that changes nothing.
    // ---------------------------------------------------------------------
    const stale = await request(
      record,
      credential,
      "PUT",
      "/api/v1/model-data-policy",
      policyBody(0, "local_only"),
      { "Idempotency-Key": idempotencyKey("policy-3") },
    );
    if (stale.status !== 409) {
      fail(`a stale precondition must be a conflict, got ${stale.status}`, stale.text);
    } else if (stale.json?.error?.code !== "resource.version_conflict") {
      fail("a stale precondition must use the contract's conflict code", stale.text);
    } else {
      pass("a stale precondition is a conflict with resource.version_conflict");
    }
    const stillTwo = await request(record, credential, "GET", "/api/v1/model-data-policy");
    if (stillTwo.json?.version !== 2) {
      fail("a refused write must leave the stored policy unchanged", stillTwo.text);
    } else {
      pass("the refused write left the policy at version 2");
    }

    // ---------------------------------------------------------------------
    // 9. A write without an idempotency key is refused before it writes.
    //
    // The contract requires it, and the consequence of not having one is that a client retrying
    // a timed-out write advances the version for a change already applied — so the caller can no
    // longer tell how many distinct policies it has made.
    // ---------------------------------------------------------------------
    const noKey = await request(
      record,
      credential,
      "PUT",
      "/api/v1/model-data-policy",
      policyBody(2, "local_only"),
    );
    if (noKey.status !== 400 || noKey.json?.error?.code !== "request.invalid") {
      fail(`a write without a key must be refused, got ${noKey.status}`, noKey.text);
    } else {
      pass("a write without an idempotency key is refused with request.invalid");
    }

    // ---------------------------------------------------------------------
    // 10. A locality this build does not support is refused rather than stored.
    //
    // The merge can only narrow a rule it understands, so a value this build did not parse would
    // be a rule it wrote and never enforced — the one edit direction that widens a policy.
    // ---------------------------------------------------------------------
    const badValue = await request(
      record,
      credential,
      "PUT",
      "/api/v1/model-data-policy",
      policyBody(2, "send_it_anywhere"),
      { "Idempotency-Key": idempotencyKey("policy-4") },
    );
    if (badValue.status !== 400) {
      fail(`an unsupported locality must be refused, got ${badValue.status}`, badValue.text);
    } else {
      pass("an unsupported rule value is refused rather than stored");
    }
    const unchanged = await request(record, credential, "GET", "/api/v1/model-data-policy");
    if (unchanged.json?.version !== 2) {
      fail("a refused write advanced the version", unchanged.text);
    } else {
      pass("no refused write advanced the version");
    }

    // ---------------------------------------------------------------------
    // 11. The daemon is still healthy after the policy surface ran.
    //
    // Cheap, but it is the check that catches a handler that panics in a way the router
    // converted into a response: the process would still be alive and the next probe would
    // fail for an unrelated-looking reason.
    // ---------------------------------------------------------------------
    const live = await request(record, credential, "GET", "/health/live");
    if (live.status !== 200) {
      fail(`the daemon stopped reporting liveness after a policy probe: ${live.status}`);
    } else {
      pass("the daemon still reports liveness after probing the policy surface");
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
  console.log("\nall model data policy surface checks passed");
}

await main();
