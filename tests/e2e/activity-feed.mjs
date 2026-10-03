// The activity-feed journey: seeing everything JARVIS does, in one stream, on a real `jarvisd`.
//
// A heads-up display, a voice client or a notifier must not need a run identifier to know what is going on.
// `GET /api/v1/activity` (`jarvis runs watch`) is one server-sent-event stream of every run's public events in the
// workspace, each frame's `id:` being a cursor. A fake OpenAI-compatible server stands in for the model, and the MCP
// fixture's tool makes one run park on an approval. No model, no network.
//
//   - a feed opened with no position starts from now: history is not replayed, and it is not interleaved by accident;
//   - two runs started together appear in one stream, with cursors that only ever increase, and the parked run's
//     `run.approval_requested` is in it — the frame a HUD or a voice client needs to ask a person;
//   - approving it surfaces the rest of that run in the same stream;
//   - streamed output text is left out unless `?deltas=true` is asked for;
//   - a client that disconnects and reconnects with `Last-Event-ID` receives exactly the frames it missed;
//   - an unauthenticated request is refused.
//
//   cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/activity-feed.mjs target/debug
import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/activity-feed.mjs <bin-dir>");
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
  console.log("\nskipped: nothing was proved about the activity feed");
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


// The fake model: a user text saying "park me" proposes the fixture tool call (which asks for approval) and then,
// once a tool result is present, answers; anything else answers directly.
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = JSON.parse(body);
      const text = JSON.stringify(parsed.messages);
      const answered = parsed.messages.some((message) => message.role === "tool");
      res.writeHead(200, { "content-type": "text/event-stream" });
      if (text.includes("park me") && !answered) {
        const call = { index: 0, id: `call-${Date.now()}`, type: "function", function: { name: "mcp_read_file_1", arguments: JSON.stringify({ path: "notes.txt" }) } };
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
      } else {
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{"content":"the answer"},"finish_reason":null}]}\n\n');
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
      }
      res.end("data: [DONE]\n\n");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;
/** Runs `scenario` against a fresh daemon whose profile carries `toolsToml`. */
async function withDaemon(label, toolsToml, scenario) {
  const profile = await mkdtemp(join(tmpdir(), `jarvis-feed-${label}-`));
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
        'api_key_ref = "env:JARVIS_FEED_KEY"',
        "",
        "[model.provider]",
        'id = "local.feed"',
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
        'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_FEED_SELECT" }',
        "",
      ].join("\n"),
      "utf8",
    );
    const environment = { ...process.env, JARVIS_FEED_KEY: "placeholder", JARVIS_FEED_SELECT: "standard" };
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

/** Follows the feed over HTTP and collects parsed frames until `stop()` is called. */
function follow(discovery, credential, path, lastEventId) {
  const frames = [];
  const controller = new AbortController();
  const headers = { authorization: `B${"earer"} ${credential}`, "jarvis-api-version": "1", accept: "text/event-stream" };
  if (lastEventId !== undefined) headers["last-event-id"] = String(lastEventId);
  const done = (async () => {
    const response = await fetch(`${discovery.base_url}${path}`, { headers, signal: controller.signal });
    if (response.status !== 200) throw new Error(`the feed answered ${response.status}`);
    const decoder = new TextDecoder();
    let buffer = "";
    for await (const chunk of response.body) {
      buffer += decoder.decode(chunk, { stream: true });
      for (let end = buffer.indexOf("\n\n"); end >= 0; end = buffer.indexOf("\n\n")) {
        const block = buffer.slice(0, end);
        buffer = buffer.slice(end + 2);
        if (block.startsWith(":")) continue;
        const frame = {};
        for (const line of block.split("\n")) {
          const at = line.indexOf(": ");
          if (at > 0) frame[line.slice(0, at)] = line.slice(at + 2);
        }
        if (frame.data) frames.push({ cursor: Number(frame.id), type: frame.event, ...JSON.parse(frame.data) });
      }
    }
  })().catch((error) => {
    if (error.name !== "AbortError") throw error;
  });
  return { frames, stop: async () => { controller.abort(); await done; } };
}

try {
  await withDaemon("feed", "", async ({ api, waitFor, startRun, completed, pending, cli, discovery, credential }) => {
    const unauthenticated = await fetch(`${discovery.base_url}/api/v1/activity`, { headers: { accept: "text/event-stream", "jarvis-api-version": "1" } });
    check(unauthenticated.status === 401, "an unauthenticated feed request is refused", `${unauthenticated.status}`);
    await unauthenticated.body?.cancel();

    // History that must NOT be replayed to a feed opened with no position.
    const earlier = await startRun("plain before the feed");
    await waitFor("the earlier run", completed(earlier));

    const live = follow(discovery, credential, "/api/v1/activity");
    await sleep(500);
    check(live.frames.length === 0, "a feed opened with no position starts from now: no history is replayed", JSON.stringify(live.frames.map((f) => f.type)));

    const parked = await startRun("park me");
    const plain = await startRun("plain answer");
    const approval = await waitFor("a prompt for the parked run", pending);
    await waitFor("the plain run to complete", completed(plain));
    await waitFor("both runs to show in the feed", async () => live.frames.some((f) => f.run_id === plain && f.type === "run.completed") && live.frames.some((f) => f.run_id === parked && f.type === "run.approval_requested"));

    const runs = new Set(live.frames.map((f) => f.run_id));
    check(runs.has(parked) && runs.has(plain) && !runs.has(earlier), "one stream carries both new runs and none of the earlier history");
    check(live.frames.every((f, i) => i === 0 || f.cursor > live.frames[i - 1].cursor), "cursors only ever increase");
    check(live.frames.some((f) => f.run_id === parked && f.type === "run.approval_requested"), "the parked run's approval request is in the feed");
    check(!live.frames.some((f) => f.type === "run.output_text.delta"), "streamed output text is left out by default");
    check(!live.frames.some((f) => f.run_id === parked && f.type === "run.completed"), "the parked run has not completed");

    const decided = await cli("approvals", "approve", approval.approval_id, "--fingerprint", approval.action_fingerprint, "--version", String(approval.version));
    check(decided.code === 0, "approved from the CLI", decided.err.slice(0, 200));
    await waitFor("the parked run to finish in the feed", async () => live.frames.some((f) => f.run_id === parked && f.type === "run.completed"));
    pass("approving surfaces the rest of that run in the same stream");

    // Reconnect from the last cursor seen: exactly the missed frames, no duplicates.
    const seen = live.frames.at(-1).cursor;
    await live.stop();
    const missed = await startRun("plain while disconnected");
    await waitFor("the missed run", completed(missed));
    const resumed = follow(discovery, credential, "/api/v1/activity", seen);
    await waitFor("the missed frames", async () => resumed.frames.some((f) => f.run_id === missed && f.type === "run.completed"));
    check(resumed.frames.every((f) => f.cursor > seen), "a reconnect with Last-Event-ID delivers nothing already seen");
    check(resumed.frames.every((f) => f.run_id === missed), "and it delivers exactly what was missed", JSON.stringify([...new Set(resumed.frames.map((f) => f.run_id))]));
    await resumed.stop();

    // Opting in to streamed text, replaying from the start.
    const everything = follow(discovery, credential, "/api/v1/activity?after=0&deltas=true");
    await waitFor("the replay", async () => everything.frames.some((f) => f.run_id === missed && f.type === "run.completed"));
    check(everything.frames.some((f) => f.type === "run.output_text.delta"), "?deltas=true includes streamed output text");
    check(everything.frames.some((f) => f.run_id === earlier), "?after=0 replays what is retained");
    await everything.stop();

    const bad = await fetch(`${discovery.base_url}/api/v1/activity?after=abc`, { headers: { authorization: `B${"earer"} ${credential}`, "jarvis-api-version": "1", accept: "text/event-stream" } });
    check(bad.status === 400, "a malformed cursor is refused rather than ignored", `${bad.status}`);
    await bad.body?.cancel();
  });
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  fake.closeAllConnections?.();
  fake.close();
}

if (failures > 0) {
  console.error(`\n${failures} activity-feed check(s) failed`);
  process.exit(1);
}
console.log("\nactivity-feed checks passed");