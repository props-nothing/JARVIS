// The kill-switch journey: seeing what JARVIS is doing, and stopping all of it, on a real `jarvisd`.
//
// Oversight needs two things an operator can reach for in a hurry: "what is it doing right now?" and "stop
// everything". `GET /api/v1/runs` (`jarvis runs list`) lists runs that have not finished, including one parked on
// an approval; `POST /api/v1/runs/stop-all` (`jarvis runs stop-all`) stops them all. A fake OpenAI-compatible server
// stands in for the model: one run is parked on an approval (the MCP fixture's tool asks at the default level) and
// another is mid-way through a slow model call. No model, no network.
//
//   - both runs are listed, and only the parked one says `awaiting_approval`;
//   - `stop-all` reports one executing run signalled and one parked run cancelled;
//   - both end `cancelled`, the approval the parked run waited on is no longer pending, and nothing is listed;
//   - repeating it stops nothing and does no harm, and a new run is still accepted afterwards (it is not a latch).
//
//   cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/kill-switch.mjs target/debug

import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/kill-switch.mjs <bin-dir>");
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
  console.log("\nskipped: nothing was proved about the kill switch");
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


// The fake model: a request whose user text says "slow" holds the connection open (a model call in progress);
// any other request with no tool result proposes the fixture tool call, which asks for approval.
const held = new Set();
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = JSON.parse(body);
      const text = JSON.stringify(parsed.messages);
      res.writeHead(200, { "content-type": "text/event-stream" });
      if (text.includes("slow work")) {
        res.write('data: {"id":"s","choices":[{"index":0,"delta":{"content":"thinking"},"finish_reason":null}]}\n\n');
        const timer = setTimeout(() => res.end("data: [DONE]\n\n"), 120000);
        held.add(timer);
        res.on("close", () => clearTimeout(timer));
        return;
      }
      const call = {
        index: 0,
        id: `call-${Date.now()}`,
        type: "function",
        function: { name: "mcp_read_file_1", arguments: JSON.stringify({ path: "notes.txt" }) },
      };
      res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] })}\n\n`);
      res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
      res.end("data: [DONE]\n\n");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;
/** Runs `scenario` against a fresh daemon whose profile carries `toolsToml`. */
async function withDaemon(label, toolsToml, scenario) {
  const profile = await mkdtemp(join(tmpdir(), `jarvis-kill-${label}-`));
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
        'api_key_ref = "env:JARVIS_KILL_KEY"',
        "",
        "[model.provider]",
        'id = "local.kill"',
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
        'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_KILL_SELECT" }',
        "",
      ].join("\n"),
      "utf8",
    );
    const environment = { ...process.env, JARVIS_KILL_KEY: "placeholder", JARVIS_KILL_SELECT: "standard" };
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
          ...(body ? { "content-type": "application/json", "idempotency-key": `kill-${Date.now()}-${Math.random()}` } : {}),
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

const runsOf = async (cli) => {
  const out = await cli("runs", "list");
  try {
    return { code: out.code, ...JSON.parse(out.out) };
  } catch {
    return { code: out.code, runs: [], raw: `${out.out} ${out.err}` };
  }
};

try {
  await withDaemon("kill", "", async ({ api, waitFor, startRun, pending, cli, stateOf }) => {
    const parked = await startRun("park me");
    const approval = await waitFor("a prompt for the parked run", pending);
    const slow = await startRun("slow work");
    await waitFor("the slow run to be in flight", async () => (await stateOf(slow)) !== "received");

    const before = await runsOf(cli);
    check(before.code === 0 && before.runs.length === 2, "both unfinished runs are listed", JSON.stringify(before).slice(0, 300));
    const byId = Object.fromEntries(before.runs.map((r) => [r.run_id, r]));
    check(byId[parked]?.awaiting_approval === true, "the parked run says it is awaiting approval");
    check(byId[slow]?.awaiting_approval === false, "the executing run does not");

    const stopped = await cli("runs", "stop-all", "--reason", "leaving the house");
    check(stopped.code === 0, "`runs stop-all` succeeded", `${stopped.err} ${stopped.out}`.slice(0, 300));
    const summary = JSON.parse(stopped.out);
    check(summary.signalled === 1, "one executing run was signalled", stopped.out);
    check(summary.parked_cancelled === 1, "one parked run was cancelled directly", stopped.out);
    check(summary.unsignalled === 0 && summary.bounded === false, "nothing was left unsignalled", stopped.out);

    const ended = async (id) => (await stateOf(id)) === "cancelled";
    check(await waitFor("the parked run to be cancelled", () => ended(parked)), "the parked run ended cancelled");
    check(await waitFor("the executing run to be cancelled", () => ended(slow)), "the executing run ended cancelled");

    const view = (await api("GET", `/api/v1/approvals/${approval.approval_id}`)).json;
    check(view?.state !== "pending" && view?.state !== "approved", "the approval it waited on was withdrawn", JSON.stringify(view)?.slice(0, 200));
    check(!(await pending()), "no approval is left pending");

    const after = await runsOf(cli);
    check(after.runs.length === 0, "nothing is listed as active afterwards");

    const again = JSON.parse((await cli("runs", "stop-all")).out);
    check(again.signalled === 0 && again.parked_cancelled === 0, "repeating it stops nothing and does no harm", JSON.stringify(again));

    const fresh = await api("POST", "/api/v1/runs", { conversation_id: null, input: { type: "text", text: "after the stop" }, runtime: "jarvis-native" });
    check(fresh.status === 202, "a new run is still accepted afterwards: it is a stop, not a latch", `${fresh.status}`);
    await api("POST", `/api/v1/runs/${fresh.json?.run_id}/cancel`, { reason: "done" });
  });
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  for (const timer of held) clearTimeout(timer);
  fake.closeAllConnections?.();
  fake.close();
}

if (failures > 0) {
  console.error(`\n${failures} kill-switch check(s) failed`);
  process.exit(1);
}
console.log("\nkill-switch checks passed");