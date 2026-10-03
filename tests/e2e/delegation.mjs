// The delegation journey: one run handing work to another, on a real `jarvisd`.
//
// `agents.delegate@1` lets a run give a self-contained task to a **child run** and use its answer. The child is an
// ordinary run — same principal, same tool pipeline — so delegation adds cost and fan-out, not authority; both
// are bounded. A fake OpenAI-compatible server stands in for the model and plays whichever agent is asking,
// keyed on the objective text. No model, no network.
//
//   disabled (the default)    the model is not offered the tool at all;
//   enabled                   the tool is offered; a "boss" run delegates, the child answers, and the boss uses the
//                             child's answer; the child records its parent, and the boss has none;
//   depth                     a run that keeps delegating is stopped at the limit with a refusal the model can read,
//                             and exactly the allowed number of runs exist;
//   cancellation              cancelling the waiting parent stops its child, and the kill switch stops both.
//
//   cargo build -p jarvisd -p jarvis-cli
//   node tests/e2e/delegation.mjs target/debug
import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/delegation.mjs <bin-dir>");
  process.exit(2);
}
const exe = process.platform === "win32" ? ".exe" : "";
const daemonBin = join(binDir, `jarvisd${exe}`);
const cliBin = join(binDir, `jarvis${exe}`);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let failures = 0;
const pass = (message) => console.log(`ok    ${message}`);
function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail !== undefined) console.error(`      ${detail}`);
}
const check = (condition, message, detail) => (condition ? pass(message) : fail(message, detail));

if (!existsSync(daemonBin) || !existsSync(cliBin)) {
  console.log(`skip  the daemon or the CLI is not built (${daemonBin}, ${cliBin})`);
  console.log("\nskipped: nothing was proved about delegation");
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


// The fake model. Which agent is asking is read from the first user message; whether a tool already answered is
// read from the transcript. Every request is recorded, so assertions are on what the daemon actually sent.
let requests = [];
const held = new Set();
const textOf = (message) => (typeof message.content === "string" ? message.content : JSON.stringify(message.content));
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = JSON.parse(body);
      requests.push(parsed);
      const objective = textOf(parsed.messages.find((m) => m.role === "user") ?? { content: "" });
      const toolResult = parsed.messages.filter((m) => m.role === "tool").map(textOf).join("\n");
      res.writeHead(200, { "content-type": "text/event-stream" });
      const call = (task) => {
        const toolCall = { index: 0, id: `call-${Date.now()}-${Math.random()}`, type: "function", function: { name: "agents_delegate_1", arguments: JSON.stringify({ task }) } };
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: { tool_calls: [toolCall] }, finish_reason: null }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
        res.end("data: [DONE]\n\n");
      };
      const say = (text) => {
        res.write(`data: ${JSON.stringify({ id: "2", choices: [{ index: 0, delta: { content: text }, finish_reason: null }] })}\n\n`);
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
        res.end("data: [DONE]\n\n");
      };
      const hold = () => {
        res.write('data: {"id":"s","choices":[{"index":0,"delta":{"content":"working"},"finish_reason":null}]}\n\n');
        const timer = setTimeout(() => res.end("data: [DONE]\n\n"), 120000);
        held.add(timer);
        res.on("close", () => clearTimeout(timer));
      };
      const level = Number(/^recurse (\d+)/.exec(objective)?.[1] ?? NaN);
      if (objective.startsWith("boss task")) return toolResult ? say(`boss used: ${toolResult.includes("CODEWORD-PINEAPPLE") ? "CODEWORD-PINEAPPLE" : "nothing"}`) : call("child task: report the code word");
      if (objective.startsWith("child task")) return say("CODEWORD-PINEAPPLE");
      if (!Number.isNaN(level)) return toolResult ? say(`level ${level} done`) : call(`recurse ${level + 1}`);
      if (objective.startsWith("slow boss")) return toolResult ? say("slow boss done") : call("slow child");
      if (objective.startsWith("slow child")) return hold();
      return say("plain answer");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;

const offered = (request) => (request.tools ?? []).map((tool) => tool.function.name);
/** Runs `scenario` against a fresh daemon whose profile carries `toolsToml`. */
async function withDaemon(label, toolsToml, scenario) {
  const profile = await mkdtemp(join(tmpdir(), `jarvis-deleg-${label}-`));
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
        'api_key_ref = "env:JARVIS_DELEG_KEY"',
        "",
        "[model.provider]",
        'id = "local.deleg"',
        'host = "127.0.0.1"',
        `port = ${port}`,
        'base_path = "/v1"',
        'models = ["model-one"]',
        "",
        toolsToml,
        "",        "",
      ].join("\n"),
      "utf8",
    );
    const environment = { ...process.env, JARVIS_DELEG_KEY: "placeholder" };
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
          ...(body ? { "content-type": "application/json", "idempotency-key": `feed-${Date.now()}-${Math.random()}` } : {}),
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
    await scenario({ api, waitFor, startRun, completed, pending, cli, stateOf, discovery, credential });
  } finally {
    daemon?.kill();
    await sleep(300);
    await rm(profile, { recursive: true, force: true }).catch(() => {});
  }
}

const get = async (discovery, credential, path) => {
  const response = await fetch(`${discovery.base_url}${path}`, { headers: { authorization: `B${"earer"} ${credential}`, "jarvis-api-version": "1" } });
  return { status: response.status, json: await response.json().catch(() => undefined) };
};

try {
  // ---- disabled (the default) ---------------------------------------------------------------------------------
  await withDaemon("off", "", async ({ waitFor, startRun, completed }) => {
    requests = [];
    const run = await startRun("plain question");
    check(await waitFor("the run", completed(run)), "a default profile runs");
    check(requests.length > 0 && !requests.some((r) => offered(r).includes("agents_delegate_1")), "the model is not offered the delegation tool unless the profile enables it", JSON.stringify(offered(requests[0])));
  });

  // ---- enabled ------------------------------------------------------------------------------------------------
  await withDaemon("on", "[tools.agents]\nenabled = true", async ({ api, waitFor, startRun, completed, discovery, credential, stateOf }) => {
    requests = [];
    const boss = await startRun("boss task: find the code word");
    check(await waitFor("the boss run", completed(boss)), "the delegating run completed");
    check(requests.some((r) => offered(r).includes("agents_delegate_1")), "the model is offered the delegation tool");
    const bossFinal = requests.at(-1);
    const toolText = bossFinal.messages.filter((m) => m.role === "tool").map(textOf).join("\n");
    check(toolText.includes("CODEWORD-PINEAPPLE"), "the child's answer reached the delegating model as the tool result", toolText.slice(0, 300));
    const delegated = JSON.parse(/\{.*\}/s.exec(toolText)?.[0] ?? "{}");
    check(delegated.state === "completed" && typeof delegated.run_id === "string", "the result names the child run and its state", toolText.slice(0, 300));

    const child = await get(discovery, credential, `/api/v1/runs/${delegated.run_id}`);
    const parent = await get(discovery, credential, `/api/v1/runs/${boss}`);
    check(child.json?.parent_run_id === boss, "the child records the delegating run as its parent", JSON.stringify(child.json)?.slice(0, 300));
    check(parent.json?.parent_run_id === undefined, "the delegating run has no parent");
    check(child.json?.conversation_id !== parent.json?.conversation_id, "the child has its own conversation");
    check(child.json?.state === "completed", "the child run is completed");

    // ---- depth: a run that keeps delegating is stopped at the limit --------------------------------------------
    requests = [];
    const root = await startRun("recurse 0");
    check(await waitFor("the chain to finish", completed(root), 60000), "the root of a delegation chain completed");
    const objectiveOf = (request) => textOf(request.messages.find((m) => m.role === "user") ?? { content: "" });
    const finalOf = (level) => requests.find((r) => objectiveOf(r).startsWith(`recurse ${level}`) && r.messages.some((m) => m.role === "tool"));
    const toolText2 = (request) => request?.messages.filter((m) => m.role === "tool").map(textOf).join("\n") ?? "";
    const runIdIn = (request) => /"run_id":"([0-9a-f-]+)"/.exec(toolText2(request).replaceAll('\\"', '"'))?.[1];
    const levelOne = runIdIn(finalOf(0));
    const levelTwo = runIdIn(finalOf(1));
    check(Boolean(levelOne) && Boolean(levelTwo), "the root delegated to a child, which delegated to a grandchild", `${levelOne} ${levelTwo}`);
    const parentOf = async (id) => (await get(discovery, credential, `/api/v1/runs/${id}`)).json?.parent_run_id;
    check((await parentOf(levelOne)) === root && (await parentOf(levelTwo)) === levelOne, "the stored links form the chain root <- child <- grandchild");
    const refused = toolText2(finalOf(2));
    check(finalOf(2) !== undefined && !runIdIn(finalOf(2)), "the grandchild's attempt to delegate again created no run", refused.slice(0, 300));
    check(/limit/i.test(refused), "and it was refused with a reason the model can read", refused.slice(0, 300));

    // ---- cancellation: the waiting parent takes its child with it -----------------------------------------------
    const childOf = async (parentId) => {
      const active = await get(discovery, credential, "/api/v1/runs");
      return active.json?.runs?.find((r) => r.parent_run_id === parentId);
    };
    requests = [];
    const slowBoss = await startRun("slow boss");
    const slowChild = await waitFor("the child to be running", () => childOf(slowBoss));
    check(Boolean(slowChild), "a running delegation shows the child under its parent in `runs list`");
    await api("POST", `/api/v1/runs/${slowBoss}/cancel`, { reason: "changed my mind" });
    check(await waitFor("the parent to be cancelled", async () => (await stateOf(slowBoss)) === "cancelled"), "the parent run ended cancelled");
    check(await waitFor("the child to be cancelled", async () => (await stateOf(slowChild.run_id)) === "cancelled"), "and its child was stopped with it");

    // ---- the kill switch reaches children too -------------------------------------------------------------------
    requests = [];
    const another = await startRun("slow boss again");
    const anotherChild = await waitFor("the second child", () => childOf(another));
    const stopped = await api("POST", "/api/v1/runs/stop-all", {});
    check(stopped.json?.signalled === 2, "the kill switch signalled the parent and its child", stopped.text.slice(0, 200));
    check(await waitFor("both cancelled", async () => (await stateOf(another)) === "cancelled" && (await stateOf(anotherChild.run_id)) === "cancelled"), "both ended cancelled");  });
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  for (const timer of held) clearTimeout(timer);
  fake.closeAllConnections?.();
  fake.close();
}

if (failures > 0) {
  console.error(`\n${failures} delegation check(s) failed`);
  process.exit(1);
}
console.log("\ndelegation checks passed");