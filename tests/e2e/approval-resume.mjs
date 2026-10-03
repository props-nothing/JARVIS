// The approval-resume journey: a tool call that needs a human **parks the run**, and the human's decision
// **moves it again** — approved, the tool runs and the model answers from its result; rejected, the model is
// told it was refused and the tool never runs.
//
// Not gated on a model or a network. A local fake OpenAI-compatible server stands in for the model: its first
// request proposes a call to the MCP fixture's `read_file` tool, its second answers, and **both requests are
// recorded**, so the assertion is on what the daemon actually sent the model. What this proves and a unit
// test cannot:
//
//   - a real `jarvisd` parks the run: it stays open on a pending approval and the tool has not run (before this,
//     the park was refused as an illegal transition and the failure was swallowed by the detached task);
//   - the decision, taken through the real CLI, continues the **same run**: it completes with the model's
//     second answer, and the second request carries the **tool's own output** — which only the MCP child
//     process could have produced — so the call really ran through the governed pipeline after approval;
//   - a rejection never runs the tool, and the model still gets a tool result (the refusal) so it can answer;
//   - the approval is spent: the same action asks again rather than reusing the first approval;
//   - cancelling a run that is parked cancels it (it has no task to signal);
//   - a parked run survives a daemon restart: the approval is still pending, recovery does not fail the run, and
//     a decision taken on the restarted daemon continues it.
//
//   cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/approval-resume.mjs target/debug

import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHash } from "node:crypto";

const SENTENCE = "fixture result for read_file";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/approval-resume.mjs <bin-dir>");
  process.exit(2);
}
const exe = process.platform === "win32" ? ".exe" : "";
const daemonBin = join(binDir, `jarvisd${exe}`);
const cliBin = join(binDir, `jarvis${exe}`);
const fixtureBin = resolve(join(binDir, "examples", `mcp_fixture_server${exe}`));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let failures = 0;
const pass = (message) => console.log(`ok    ${message}`);
function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail !== undefined) console.error(`      ${detail}`);
}
const check = (condition, message, detail) => (condition ? pass(message) : fail(message, detail));

if (!existsSync(fixtureBin) || !existsSync(cliBin)) {
  console.log(`skip  the MCP fixture or the CLI is not built (${fixtureBin}, ${cliBin})`);
  console.log("\nskipped: nothing was proved about resuming after an approval");
  process.exit(0);
}

/** The principal the daemon derives for the enrolled `owner` client. */
function ownerPrincipal() {
  const hex = createHash("sha256").update("owner").digest("hex").slice(0, 32);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

function run(command, args, options = {}) {
  return new Promise((done) => {
    const child = spawn(command, args, { ...options, shell: false });
    let out = "";
    let err = "";
    child.stdout?.on("data", (c) => (out += c.toString()));
    child.stderr?.on("data", (c) => (err += c.toString()));
    child.on("error", (e) => done({ code: -1, out: `${out}${e.message}`, err }));
    child.on("close", (code) => done({ code, out, err }));
  });
}

// The fake model. Every request body is recorded. A request that carries **no tool result** proposes the call;
// one that does answers. Deciding by content rather than by count keeps the scenarios independent of order.
const requests = [];
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = JSON.parse(body);
      requests.push(parsed);
      const answered = parsed.messages.some((message) => message.role === "tool");
      res.writeHead(200, { "content-type": "text/event-stream" });
      if (!answered) {
        const call = {
          index: 0,
          id: `call-${requests.length}`,
          type: "function",
          function: { name: "mcp.read_file@1", arguments: '{"path":"notes.txt"}' },
        };
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
      } else {
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{"content":"the tool answered"},"finish_reason":null}]}\n\n');
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
      }
      res.end("data: [DONE]\n\n");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;

const profile = await mkdtemp(join(tmpdir(), "jarvis-approval-resume-"));
let daemon;
try {
  await mkdir(join(profile, "config"), { recursive: true });
  await writeFile(
    join(profile, "config", "config.toml"),
    [
      "schema_version = 4",
      "",
      "[model]",
      'policy_id = "default"',
      'api_key_ref = "env:JARVIS_RESUME_KEY"',
      "",
      "[model.provider]",
      'id = "local.resume"',
      'host = "127.0.0.1"',
      `port = ${port}`,
      'base_path = "/v1"',
      'models = ["model-one"]',
      "",
      "[[mcp.servers]]",
      'name = "fixture-files"',
      `program = '${fixtureBin}'`,
      'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_RESUME_SELECT" }',
      "",
    ].join("\n"),
    "utf8",
  );
  const environment = { ...process.env, JARVIS_RESUME_KEY: "placeholder", JARVIS_RESUME_SELECT: "standard" };
  const discoveryPath = join(profile, "run", "discovery.json");
  const credentialPath = join(profile, "config", "client-credential");
  let discovery;
  let credential = "";
  const startDaemon = async () => {
    // A killed daemon cannot unpublish its discovery file, so a restart must not read the stale one.
    await rm(discoveryPath, { force: true });
    daemon = spawn(daemonBin, ["--profile", profile], { env: environment, shell: false });
    discovery = undefined;
    for (let i = 0; i < 150 && !discovery; i += 1) {
      try {
        discovery = existsSync(discoveryPath) ? JSON.parse(await readFile(discoveryPath, "utf8")) : undefined;
        credential = existsSync(credentialPath) ? (await readFile(credentialPath, "utf8")).trim() : "";
      } catch {
        // A partially written file is expected; try again.
      }
      if (!discovery || !credential) await sleep(100);
    }
    if (!discovery || !credential) throw new Error("the daemon never became reachable (see the profile's log directory)");
  };
  await startDaemon();
  const api = async (method, path, body) => {
    const response = await fetch(`${discovery.base_url}${path}`, {
      method,
      headers: { authorization: `Bearer ${credential}`, "jarvis-api-version": "1", ...(body ? { "content-type": "application/json", "idempotency-key": `resume-${Date.now()}-${Math.random()}` } : {}) },
      body: body ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(30000),
    });
    const text = await response.text();
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      json = undefined;
    }
    return { status: response.status, text, json };
  };
  const waitFor = async (describe, probe, ms = 20000) => {
    const deadline = Date.now() + ms;
    for (;;) {
      const value = await probe();
      if (value) return value;
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${describe}`);
      await sleep(100);
    }
  };
  const stateOf = async (runId) => (await api("GET", `/api/v1/runs/${runId}`)).json?.state;
  const pendingApproval = (excluding = []) => async () => {
    const listed = await api("GET", "/api/v1/approvals");
    return listed.json?.approvals?.find((a) => a.state === "pending" && !excluding.includes(a.approval_id));
  };
  const startRun = async (text) => {
    const created = await api("POST", "/api/v1/runs", { conversation_id: null, input: { type: "text", text }, runtime: "jarvis-native" });
    if (created.status !== 202) throw new Error(`the run was refused with ${created.status}: ${created.text.slice(0, 300)}`);
    return created.json.run_id;
  };
  const decide = (verb, approval) =>
    run(cliBin, ["approvals", verb, approval.approval_id, "--fingerprint", approval.action_fingerprint, "--version", String(approval.version), "--profile", profile], { env: environment });
  const toolMessages = (request) => request.messages.filter((message) => message.role === "tool");

  const granted = await run(
    cliBin,
    ["grants", "create", "--capability", "mcp.read_file@1", "--principal", ownerPrincipal(),
     "--effect", "write", "--risk", "moderate", "--sensitivity", "internal", "--profile", profile],
    { env: environment },
  );
  check(granted.code === 0, "an operator grant was written for the MCP tool", `${granted.err.slice(0, 300)} ${granted.out.slice(0, 200)}`);

  // ---- Scenario 1: approve ---------------------------------------------------------------------------------
  const approved = await startRun("read notes.txt");
  const first = await waitFor("an approval to be raised", pendingApproval());
  // The public state set is deliberately coarse (`agent-runtime.md` forbids domain state names doubling as wire
  // strings), so a parked run reads `model_running`; what distinguishes it is the pending approval and a model
  // that has been asked exactly once.
  check((await stateOf(approved)) === "model_running", "the run is still open while it waits for the decision");
  check(requests.length === 1, "the model has been asked once and the tool has not run", `requests=${requests.length}`);
  const approval = await decide("approve", first);
  check(approval.code === 0, "the approval was decided through the CLI", `${approval.err.slice(0, 300)}`);
  const finished = await waitFor("the run to complete after approval", async () => {
    const state = await stateOf(approved);
    if (state === "failed" || state === "cancelled") throw new Error(`the run ended ${state} after approval`);
    return state === "completed";
  });
  check(finished, "the approved run continued and completed");
  const second = requests[1];
  const carried = second && toolMessages(second).some((message) => JSON.stringify(message).includes(SENTENCE));
  check(carried, "the model's second request carries the tool's own output (it ran in the MCP child after approval)",
    JSON.stringify(second?.messages?.slice(-2)));
  const events = await api("GET", `/api/v1/runs/${approved}/events`);
  const types = events.text.split("\n").filter((l) => l.startsWith("event: ")).map((l) => l.slice(7));
  const order = ["run.approval_requested", "run.tool_executing", "run.observing", "run.completed"];
  const positions = order.map((type) => types.lastIndexOf(type));
  check(positions.every((p, i) => p >= 0 && (i === 0 || p > positions[i - 1])),
    "the event stream shows the park, the resumed execution, and the completion in order", `[${types.join(",")}]`);
  const spent = await api("GET", `/api/v1/approvals/${first.approval_id}`);
  check(spent.json?.state === "consumed", "the one-shot approval was spent by the call it authorized", spent.text.slice(0, 200));

  // ---- Scenario 2: the same action asks again -------------------------------------------------------------
  const again = await startRun("read notes.txt again");
  const asked = await waitFor("a second approval", pendingApproval([first.approval_id]));
  check(asked.approval_id !== first.approval_id, "the same action asks again instead of reusing the spent approval");

  // ---- Scenario 3: reject ----------------------------------------------------------------------------------
  const toolRunsBefore = requests.filter((r) => toolMessages(r).some((m) => JSON.stringify(m).includes(SENTENCE))).length;
  const rejection = await decide("reject", asked);
  check(rejection.code === 0, "the second approval was rejected through the CLI", `${rejection.err.slice(0, 300)}`);
  const refused = await waitFor("the rejected run to finish", async () => {
    const state = await stateOf(again);
    if (state === "failed" || state === "cancelled") throw new Error(`the run ended ${state} after rejection`);
    return state === "completed";
  });
  check(refused, "a rejected run still completes: the refusal is information the model answers from");
  const last = requests.at(-1);
  const sawRefusal = toolMessages(last).some((m) => JSON.stringify(m).includes("tool.approval_rejected"));
  check(sawRefusal, "the model was told the call was refused", JSON.stringify(last?.messages?.slice(-1)));
  const toolRunsAfter = requests.filter((r) => toolMessages(r).some((m) => JSON.stringify(m).includes(SENTENCE))).length;
  check(toolRunsAfter === toolRunsBefore, "a rejection never ran the tool");

  // ---- Scenario 4: cancel a parked run --------------------------------------------------------------------
  const cancelled = await startRun("read notes.txt a third time");
  const third = await waitFor("a third approval", pendingApproval([first.approval_id, asked.approval_id]));
  const cancel = await api("POST", `/api/v1/runs/${cancelled}/cancel`, { reason: "no longer needed" });
  check(cancel.status === 200 || cancel.status === 202, "cancelling a parked run is accepted", `${cancel.status} ${cancel.text.slice(0, 200)}`);
  const ended = await waitFor("the parked run to be cancelled", async () => (await stateOf(cancelled)) === "cancelled");
  check(ended, "a parked run is cancelled rather than left waiting for ever");
  const withdrawn = await api("GET", `/api/v1/approvals/${third.approval_id}`);
  check(withdrawn.json?.state === "cancelled", "its prompt was withdrawn rather than left pending for a run that no longer exists", withdrawn.text.slice(0, 200));
  // ---- Scenario 5: the wait survives a restart ------------------------------------------------------------
  const stale = await api("GET", "/api/v1/approvals");
  check(!stale.json?.approvals?.some((a) => a.state === "pending"), "no prompt is left pending before the restart scenario");
  const seen = [first.approval_id, asked.approval_id, third.approval_id];
  const survivor = await startRun("read notes.txt across a restart");
  const pending = await waitFor("a fourth approval", pendingApproval(seen));
  // The approval is raised before the run is parked, so killing on sight of it can catch the run mid-dispatch,
  // which restart recovery rightly fails. Wait for the park itself, which the event stream publishes.
  await waitFor("the run to publish its park", async () => {
    const response = await fetch(`${discovery.base_url}/api/v1/runs/${survivor}/events`, {
      headers: { authorization: `Bearer ${credential}`, "jarvis-api-version": "1", accept: "text/event-stream" },
      signal: AbortSignal.timeout(1500),
    }).catch(() => null);
    if (!response) return false;
    let text = "";
    try {
      for await (const chunk of response.body) {
        text += Buffer.from(chunk).toString();
        if (text.includes("run.approval_requested")) return true;
      }
    } catch {
      // The stream follows a live run, so the timeout ends it; what was read is what counts.
    }
    return text.includes("run.approval_requested");
  });
  daemon.kill();
  await new Promise((resolve) => daemon.once("exit", resolve));
  await startDaemon();
  const stillPending = await waitFor("the approval to survive the restart", pendingApproval(seen));
  check(stillPending.approval_id === pending.approval_id, "the pending approval survived the restart");
  await sleep(1500); // a recovery pass that failed the run would have done so before the daemon was ready
  check((await stateOf(survivor)) === "model_running", "the parked run was not failed by restart recovery", await stateOf(survivor));
  const requestsBefore = requests.length;
  const resumedDecision = await decide("approve", stillPending);
  check(resumedDecision.code === 0, "the approval was decided on the restarted daemon", resumedDecision.err.slice(0, 300));
  const survived = await waitFor("the run to complete after the restart", async () => {
    const state = await stateOf(survivor);
    if (state === "failed" || state === "cancelled") throw new Error(`the run ended ${state} after the restart`);
    return state === "completed";
  });
  check(survived, "the run parked before the restart continued and completed after it");
  const carriedAfter = requests.slice(requestsBefore).some((r) => toolMessages(r).some((m) => JSON.stringify(m).includes(SENTENCE)));
  check(carriedAfter, "the tool ran after the restart and its output reached the model");
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
  if (process.env.JARVIS_E2E_DEBUG) {
    const { readdir } = await import("node:fs/promises");
    const logDir = join(profile, "log");
    for (const name of existsSync(logDir) ? await readdir(logDir) : []) console.error(`--- ${name}\n${(await readFile(join(logDir, name), "utf8")).slice(-3000)}`);
  }
} finally {
  daemon?.kill();
  fake.close();
  await sleep(300);
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}

if (failures > 0) {
  console.error(`\n${failures} approval-resume check(s) failed`);
  process.exit(1);
}
console.log("\napproval-resume checks passed");
