// The autonomy journey: how often JARVIS interrupts, as the operator configures it, on a real `jarvisd`.
//
// A tool call used to need a hand-written operator grant AND a prompt for every call, which made an agent
// that wants to use its tools unusable. This proves the levels and the "always allow" path end to end, with a
// fake OpenAI-compatible server standing in for the model (every request is recorded) and the repository's
// own MCP fixture server as the tool. No model, no network, and **no grant is ever written**.
//
//   balanced (the default)
//     - the first call to an unannotated server tool reaches a prompt rather than a refusal;
//     - approving it with `--remember` runs it, and the same tool then runs on later calls with other
//       arguments **without asking**;
//     - `approvals standing` lists the permission and `approvals cancel` revokes it, after which it asks again.
//   autonomous
//     - the same tool (a reversible write of moderate risk) runs with no prompt at all.
//   ask
//     - the original behaviour: with no grant the model is told the call is not permitted.
//
//   cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/autonomy-journey.mjs target/debug

import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const SENTENCE = "fixture result for read_file";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/autonomy-journey.mjs <bin-dir>");
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
  console.log("\nskipped: nothing was proved about autonomy");
  process.exit(0);
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

// The fake model: a request with no tool result proposes the call; one with a result answers.
let requests = [];
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
          function: { name: "mcp_read_file_1", arguments: JSON.stringify({ path: `notes-${requests.length}.txt` }) },
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

const toolMessages = (request) => request.messages.filter((message) => message.role === "tool");
const sawSentence = (request) => toolMessages(request).some((m) => JSON.stringify(m).includes(SENTENCE));

/** Runs `scenario` against a fresh daemon whose profile carries `toolsToml`. */
async function withDaemon(label, toolsToml, scenario) {
  const profile = await mkdtemp(join(tmpdir(), `jarvis-autonomy-${label}-`));
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
        'api_key_ref = "env:JARVIS_AUTONOMY_KEY"',
        "",
        "[model.provider]",
        'id = "local.autonomy"',
        'host = "127.0.0.1"',
        `port = ${port}`,
        'base_path = "/v1"',
        'models = ["model-one"]',
        "",
        toolsToml,
        "",
        "[[mcp.servers]]",
        'name = "fixture-files"',
        `program = '${fixtureBin}'`,
        'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_AUTONOMY_SELECT" }',
        "",
      ].join("\n"),
      "utf8",
    );
    const environment = { ...process.env, JARVIS_AUTONOMY_KEY: "placeholder", JARVIS_AUTONOMY_SELECT: "standard" };
    daemon = spawn(daemonBin, ["--profile", profile], { env: environment, shell: false });
    const discoveryPath = join(profile, "run", "discovery.json");
    const credentialPath = join(profile, "config", "client-credential");
    let discovery;
    let credential = "";
    for (let i = 0; i < 150 && !(discovery && credential); i += 1) {
      try {
        discovery ??= existsSync(discoveryPath) ? JSON.parse(await readFile(discoveryPath, "utf8")) : undefined;
        credential = existsSync(credentialPath) ? (await readFile(credentialPath, "utf8")).trim() : "";
      } catch {
        // A partially written file is expected; try again.
      }
      if (!discovery || !credential) await sleep(100);
    }
    if (!discovery || !credential) throw new Error("the daemon never became reachable (see the profile's log directory)");

    const api = async (method, path, body) => {
      const response = await fetch(`${discovery.base_url}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${credential}`,
          "jarvis-api-version": "1",
          ...(body ? { "content-type": "application/json", "idempotency-key": `auto-${Date.now()}-${Math.random()}` } : {}),
        },
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
    const startRun = async (text) => {
      const created = await api("POST", "/api/v1/runs", { conversation_id: null, input: { type: "text", text }, runtime: "jarvis-native" });
      if (created.status !== 202) throw new Error(`the run was refused with ${created.status}: ${created.text.slice(0, 300)}`);
      return created.json.run_id;
    };
    const completed = (runId) => async () => {
      const state = await stateOf(runId);
      if (state === "failed" || state === "cancelled") throw new Error(`the run ended ${state}`);
      return state === "completed";
    };
    const pending = async () => (await api("GET", "/api/v1/approvals")).json?.approvals?.find((a) => a.state === "pending");
    const cli = (...args) => run(cliBin, [...args, "--profile", profile], { env: environment });
    await scenario({ api, waitFor, startRun, completed, pending, cli, stateOf });
  } finally {
    daemon?.kill();
    await sleep(300);
    await rm(profile, { recursive: true, force: true }).catch(() => {});
  }
}

try {
  // ---- balanced (the default): no grant written anywhere ---------------------------------------------------
  await withDaemon("balanced", "", async ({ api, waitFor, startRun, completed, pending, cli, stateOf }) => {
    requests = [];
    const first = await startRun("read notes");
    const approval = await waitFor("a prompt (not a refusal) for the first call", pending);
    check(Boolean(approval), "the first call to a server tool reaches a prompt with no grant written");
    check(approval.scope === "one_shot", "the prompt starts as a one-time approval");

    const decided = await cli(
      "approvals", "approve", approval.approval_id,
      "--fingerprint", approval.action_fingerprint, "--version", String(approval.version), "--remember",
    );
    check(decided.code === 0, "approved with --remember", `${decided.err.slice(0, 300)}`);
    check(await waitFor("the first run", completed(first)), "the approved run completed");
    check(requests.some(sawSentence), "the tool ran and its output reached the model");

    const standing = await cli("approvals", "standing");
    check(standing.code === 0 && standing.out.includes(approval.approval_id), "`approvals standing` lists the permission", `${standing.err} ${standing.out}`.slice(0, 300));

    const before = requests.length;
    const second = await startRun("read other notes");
    check(await waitFor("the second run to complete without a prompt", completed(second)), "the next call, with other arguments, ran without asking");
    check(!(await pending()), "no prompt was raised for it");
    check(requests.slice(before).some(sawSentence), "its tool output reached the model");

    const view = (await api("GET", `/api/v1/approvals/${approval.approval_id}`)).json;
    check(view?.scope === "standing" && view?.state === "approved", "the permission is standing and still approved after two uses", JSON.stringify(view)?.slice(0, 200));

    const revoked = await cli("approvals", "cancel", approval.approval_id, "--version", String(view.version), "--reason", "changed my mind");
    check(revoked.code === 0, "the permission was revoked with `approvals cancel`", revoked.err.slice(0, 300));
    const none = await cli("approvals", "standing");
    check(none.code === 0 && !none.out.includes(approval.approval_id), "it no longer appears in the standing list");

    const third = await startRun("read notes yet again");
    const asked = await waitFor("a prompt after revocation", pending).catch(async (e) => {
      const ev = await api("GET", `/api/v1/runs/${third}/events`).catch(() => ({ text: "" }));
      const last = requests.at(-1);
      throw new Error(`${e.message}; run=${(await api("GET", `/api/v1/runs/${third}`)).text.slice(0, 500)}; state=${await stateOf(third)} approvals=${(await api("GET", "/api/v1/approvals")).text.slice(0, 300)} last=${JSON.stringify(last?.messages?.slice(-2)).slice(0, 400)}`);
    });
    check(Boolean(asked), "after revocation the tool asks again");
    await api("POST", `/api/v1/runs/${third}/cancel`, { reason: "done" });
  });

  // ---- an action that must always ask cannot be remembered ------------------------------------------------
  // (covered by unit tests against the service; the journey keeps to what a real daemon adds.)

  // ---- autonomous: a reversible write of moderate risk needs no prompt -------------------------------------
  await withDaemon("autonomous", '[tools]\nautonomy = "autonomous"', async ({ waitFor, startRun, completed, pending }) => {
    requests = [];
    const runId = await startRun("read notes");
    check(await waitFor("the run to complete with no prompt", completed(runId)), "autonomous ran the tool with no prompt and no grant");
    check(!(await pending()), "nothing was raised for a person to decide");
    check(requests.some(sawSentence), "the tool's output reached the model");
  });

  // ---- ask: the original behaviour --------------------------------------------------------------------------
  await withDaemon("ask", '[tools]\nautonomy = "ask"', async ({ waitFor, startRun, completed }) => {
    requests = [];
    const runId = await startRun("read notes");
    check(await waitFor("the run to complete", completed(runId)), "the run completes");
    check(!requests.some(sawSentence), "the tool did not run: with no grant, ask refuses");
    const told = requests.some((r) => toolMessages(r).some((m) => JSON.stringify(m).includes("tool.permission_denied")));
    check(told, "the model was told the call is not permitted");
  });
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  fake.close();
}

if (failures > 0) {
  console.error(`\n${failures} autonomy check(s) failed`);
  process.exit(1);
}
console.log("\nautonomy checks passed");

