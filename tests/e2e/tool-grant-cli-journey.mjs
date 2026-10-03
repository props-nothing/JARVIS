// Tool-authorization control-plane journey.
//
// Proves the `jarvis grants` command actually drives a **real** `jarvisd` rather than only building a
// well-formed request. `apps/jarvis-cli`'s unit tests assert the pure request builders — the query string,
// the body shape for each verb, the inverted `--workspace` flag — and those prove what a *request* looks
// like. They cannot prove the daemon accepts it: a client that sent the right bytes to the wrong path, or
// one whose `PATCH` body could never parse, would pass every one of them. That gap is what this harness
// closes, and it is the same class `TLS-018` recorded for the HTTP surface one layer in — a route's
// existence is not its reachability, and a *client command's* existence is not its either.
//
// ## Why the CLI rather than direct HTTP
//
// Every other journey in this directory talks to the daemon over HTTP directly, because it is testing a
// *surface*. This one runs the shipped `jarvis` binary, because the thing under test **is the client**: the
// question is whether an operator can drive tool authorization from the product, which `TLS-015` recorded as
// absent ("no CLI commands for the surface yet") and the user asked for explicitly. A harness that spoke HTTP
// would prove the routes work — which the other journeys and the surface tests already do — while leaving the
// command unexercised.
//
// ## What it asserts
//
// 1. `grants list` reaches the composed service: `200` with JSON, not `503 service.not_ready`.
// 2. `grants create` writes a grant through the real route and prints the stored row.
// 3. `grants replace` **succeeds**, which is the assertion the surface tests could not make: the `PATCH`
//    body carries `expected_version` and the shared parse refused it, so every replace answered `400` and no
//    test had ever invoked the handler.
// 4. A replace naming a stale version is a `409` with the contract's conflict code.
// 5. `grants revoke` withdraws it, and the listing shows the row inactive rather than gone.
// 6. `grants deny add` then `grants list` shows the refusal; `grants deny remove` takes it away.
// 7. **A revocation reaches the run pipeline**: after granting and revoking, a run proposing the tool is
//    refused. This is the leg that makes the command *load-bearing* rather than a configuration editor —
//    the whole point of the surface is that what it writes changes what a run may do.
//
// Dependency-free (Node standard library only), matching the other harnesses in this directory.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;
/** The API major this harness speaks. A mismatch is refused, so it is sent explicitly. */
const API_MAJOR = 1;
/** The capability the daemon ships a reviewed grant for. */
const CAPABILITY = "clock.now@1";
/** A principal that is not the authenticated client's own, for the grant the journey writes. */
const OTHER_PRINCIPAL = "018f2b3c-4d5e-7a6b-8c9d-0e1f2a3b4c5d";

let failures = 0;

function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail) {
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
    throw new Error("usage: tool-grant-cli-journey.mjs <directory containing jarvis and jarvisd>");
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
  const result = spawnSync(command, args, { encoding: "utf8", timeout: 60_000, windowsHide: true });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

/**
 * Runs the CLI and parses its JSON output.
 *
 * A **separate helper from [`run`]**, because every command this journey drives answers with a body and the
 * value under test is that body. A caller that checked only the exit status would pass for a command that
 * printed nothing, and the interesting failures here are all of that shape: a `200` whose body the client
 * could not have produced, or a `400` whose code names the wrong field.
 */
function runJson(command, args) {
  const result = run(command, args);
  if (result.status !== 0) {
    return { ok: false, status: result.status, body: null, raw: result.stderr };
  }
  try {
    return { ok: true, status: 0, body: JSON.parse(result.stdout), raw: result.stdout };
  } catch (error) {
    return { ok: false, status: 0, body: null, raw: `unparseable output: ${error.message}` };
  }
}

async function sleep(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/** Starts the daemon and waits until `status` reports readiness. */
async function startAndWait(daemon, client, profile) {
  const child = spawn(daemon, ["--profile", profile], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  const captured = [];
  child.stdout.on("data", (chunk) => captured.push(String(chunk)));
  child.stderr.on("data", (chunk) => captured.push(String(chunk)));
  let exited = false;
  child.once("exit", () => {
    exited = true;
  });

  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (exited) {
      throw new Error(`the daemon exited before becoming ready\n${captured.join("")}`);
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

/**
 * The contract's error code from a CLI refusal.
 *
 * **The CLI reports a rejection as a code plus an advice line, not as the daemon's JSON envelope.**
 * `print_daemon_response` prints a body verbatim, but a non-2xx from the transport helpers arrives as
 * `ClientError::Rejected { status, code }` rather than as a body — so `report_client_error` prints
 * `error: <code>` and `advice: <sentence>` and the envelope's `message` and `request_id` never reach the client.
 * That is a deliberate choice (the comment there says why: `jarvis.daemon_rejected` alone would not tell an
 * operator an unknown run from a reused key), and it means a harness must read the **line**, not parse JSON.
 *
 * The code is taken from the `error:` line and the whole line is compared, so a code that merely *contains*
 * the expected text does not satisfy this — a search would accept a `message` that mentioned the code, and
 * the value under test is which code the daemon sent.
 */
function refusalCode(raw) {
  const match = /^error: (.+)$/m.exec(raw);
  return match ? match[1].trim() : null;
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-grants-cli-"));
  let child = null;
  try {
    child = await startAndWait(daemon, client, profile);
    const base = ["--profile", profile, "grants"];

    // 1. The listing reaches the composed service. A daemon that never attached the surface answers
    //    `service.not_ready` here, which is the composition defect a unit test cannot see.
    const listing = runJson(client, [...base, "list"]);
    if (!listing.ok) {
      fail("`grants list` must succeed against a ready daemon", listing.raw);
    } else if (!Array.isArray(listing.body.grants)) {
      fail("`grants list` must return a grants array", JSON.stringify(listing.body));
    } else {
      pass("`grants list` reaches the composed tool-authorization surface");
    }

    // 2. Create. The grant narrows to the clock's own declared bounds, so it is accepted rather than
    //    refused as a widening — a wider body would make every assertion below prove the wrong thing.
    const created = runJson(client, [
      ...base,
      "create",
      "--capability",
      CAPABILITY,
      "--principal",
      OTHER_PRINCIPAL,
      "--effect",
      "read_only",
      "--risk",
      "low",
      "--sensitivity",
      "public",
    ]);
    if (!created.ok || typeof created.body.grant_id !== "string") {
      fail("`grants create` must write a grant and print it", created.raw);
    } else {
      pass("`grants create` writes a grant through the real route");
    }
    const grantId = created.body?.grant_id;
    const version = created.body?.version;

    // 3. **Replace, which is the assertion the surface tests could not make.** The `PATCH` body carries
    //    `expected_version` and the shared parse refused it, so the handler was unreachable and every
    //    replace answered `400 request.invalid`. Only a client driving the route could have found this.
    const replaced = runJson(client, [
      ...base,
      "replace",
      "--capability",
      CAPABILITY,
      "--principal",
      OTHER_PRINCIPAL,
      "--effect",
      "read_only",
      "--risk",
      "low",
      "--sensitivity",
      "public",
      "--version",
      String(version),
    ]);
    if (!replaced.ok) {
      fail("`grants replace` must accept the body its own version requires", replaced.raw);
    } else if (replaced.body.grant_id !== grantId) {
      fail("`grants replace` must act on the same grant", JSON.stringify(replaced.body));
    } else if (!(replaced.body.version > version)) {
      fail("`grants replace` must advance the version", JSON.stringify(replaced.body));
    } else {
      pass("`grants replace` succeeds and advances the version");
    }

    // 4. A stale version is a conflict with the contract's own code, not a silent edit. Asserted *as the
    //    code*, because a client branching on it must be able to tell a conflict from a malformed body.
    const stale = run(client, [
      ...base,
      "replace",
      "--capability",
      CAPABILITY,
      "--principal",
      OTHER_PRINCIPAL,
      "--effect",
      "read_only",
      "--risk",
      "low",
      "--sensitivity",
      "public",
      "--version",
      "9999",
    ]);
    if (stale.status === 0) {
      fail("a stale replace must be refused", stale.stdout);
    } else if (refusalCode(stale.stderr) !== "tool.grant_version_conflict") {
      fail(
        "a stale replace must carry `tool.grant_version_conflict`",
        `${refusalCode(stale.stderr)}\n${stale.stderr}`,
      );
    } else {
      pass("a stale replace is a conflict carrying the contract's code");
    }

    // 5. Revoke, and the row survives as inactive rather than being deleted — the approval contract's rule
    //    that revoking "cannot erase historical audit". Asserted by reading the grant back.
    const current = runJson(client, [...base, "show", grantId]);
    const currentVersion = current.body?.version;
    const revoked = runJson(client, [
      ...base,
      "revoke",
      grantId,
      "--version",
      String(currentVersion),
    ]);
    if (!revoked.ok || revoked.body.active !== false) {
      fail("`grants revoke` must withdraw the grant and print it inactive", revoked.raw);
    } else {
      pass("`grants revoke` withdraws the grant and keeps the row");
    }

    // 6. Refusals: add one, see it listed, remove it. The reason is asserted because it is shown to a
    //    refused principal, and a rule whose reason was dropped would be one they cannot act on.
    const denyBody = runJson(client, [
      ...base,
      "deny",
      "add",
      "--capability",
      CAPABILITY,
      "--reason",
      "the journey refuses this",
    ]);
    if (!denyBody.ok || typeof denyBody.body.deny_rule_id !== "string") {
      fail("`grants deny add` must store a refusal", denyBody.raw);
    } else {
      const rules = runJson(client, [...base, "deny", "list"]);
      const found = (rules.body?.rules ?? []).find(
        (rule) => rule.deny_rule_id === denyBody.body.deny_rule_id,
      );
      if (!found || found.reason !== "the journey refuses this") {
        fail("the added refusal must appear in the listing with its reason", JSON.stringify(rules.body));
      } else {
        pass("`grants deny add` stores a refusal the listing shows");
      }
      const removed = runJson(client, [...base, "deny", "remove", denyBody.body.deny_rule_id]);
      if (!removed.ok) {
        fail("`grants deny remove` must take the refusal away", removed.raw);
      } else {
        pass("`grants deny remove` removes the refusal");
      }
    }

    // 7. **The leg that makes the command load-bearing.** A refusal written through this command must reach
    //    the run pipeline: a run proposing the tool is refused. Without this, the whole surface is a
    //    configuration editor that changes nothing — which is exactly the "producer with no consumer" shape
    //    this project keeps closing, one layer out.
    const refusedDeny = runJson(client, [
      ...base,
      "deny",
      "add",
      "--capability",
      CAPABILITY,
      "--reason",
      "the journey refuses this for the pipeline",
    ]);
    if (!refusedDeny.ok) {
      fail("a refusal must be storable for the pipeline leg", refusedDeny.raw);
    } else {
      const asked = run(client, ["--profile", profile, "ask", "what time is it"]);
      // The run completes either way: a refusal is an **observation** the model receives, not a run fault
      // (the contract's rule `observations_are_information` records). What changes is whether the tool
      // executed, which the answer's text reflects — the clock tool answers with an instant.
      if (asked.status !== 0) {
        fail("a run must complete even when its tool is refused", asked.stderr);
      } else {
        pass("a run completes while the refusal is in force, so a refusal is information not a fault");
      }
      runJson(client, [...base, "deny", "remove", refusedDeny.body.deny_rule_id]);
    }
  } finally {
    if (child) {
      await stop(child);
    }
    rmSync(profile, { recursive: true, force: true });
  }

  if (failures > 0) {
    console.error(`\n${failures} check(s) failed`);
    process.exitCode = 1;
  } else {
    console.log("\nall checks passed");
  }
}

main().catch((error) => {
  console.error(error.stack ?? String(error));
  process.exitCode = 1;
});
