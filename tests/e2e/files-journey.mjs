// The file-tools journey: JARVIS reading and writing the operator's files on a real `jarvisd`.
//
// The native `files.list@1`, `files.read@1` and `files.write@1` tools act only inside directories the
// operator declares as `[[tools.files.roots]]`; the model names a root and a relative path and nothing else.
// A fake OpenAI-compatible server stands in for the model and proposes scripted calls, so there is no model,
// no network, and no grant is ever written.
//
//   balanced (the default)
//     - a read runs with no prompt and its text reaches the model;
//     - a write raises a prompt whose preview shows the path and the start of the content, and approving it
//       with `--remember` creates the file; the next write then runs without asking;
//     - a write into a read-only root and a path that escapes the root are refused, and nothing outside the
//       root is ever returned to the model.
//   autonomous
//     - a write runs with no prompt.
//
//   cargo build -p jarvisd -p jarvis-cli
//   node tests/e2e/files-journey.mjs target/debug

import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const SECRET = "TOP-SECRET-SENTINEL-OUTSIDE-THE-ROOT";
const HELLO = "hello from the notes root";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/files-journey.mjs <bin-dir>");
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
  console.log("\nskipped: nothing was proved about the file tools");
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

// The fake model: each run takes the next scripted call; a request that already carries a tool result answers.
let requests = [];
let script = [];
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      const parsed = JSON.parse(body);
      requests.push(parsed);
      const answered = parsed.messages.some((message) => message.role === "tool");
      res.writeHead(200, { "content-type": "text/event-stream" });
      const next = answered ? undefined : script.shift();
      if (next) {
        const call = { index: 0, id: `call-${requests.length}`, type: "function", function: { name: next.name, arguments: JSON.stringify(next.args) } };
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: { tool_calls: [call] }, finish_reason: null }] })}\n\n`);
        res.write(`data: ${JSON.stringify({ id: "1", choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
      } else {
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{"content":"done"},"finish_reason":null}]}\n\n');
        res.write('data: {"id":"2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
      }
      res.end("data: [DONE]\n\n");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;

const toolText = (request) => request.messages.filter((m) => m.role === "tool").map((m) => JSON.stringify(m)).join("\n");
const everything = () => requests.map((r) => JSON.stringify(r)).join("\n");
const propose = (name, args) => script.push({ name, args });

/** Fresh directories for a scenario: a writable root, a read-only root, and a secret beside them. */
async function layout() {
  const base = await mkdtemp(join(tmpdir(), "jarvis-files-"));
  await mkdir(join(base, "notes"));
  await mkdir(join(base, "ro"));
  await writeFile(join(base, "notes", "hello.txt"), HELLO, "utf8");
  await writeFile(join(base, "secret.txt"), SECRET, "utf8");
  return base;
}
const rootsToml = (base, extra = "") =>
  [
    "[tools]",
    extra,
    "[[tools.files.roots]]",
    'name = "notes"',
    `path = '${join(base, "notes")}'`,
    'mode = "read_write"',
    "[[tools.files.roots]]",
    'name = "ro"',
    `path = '${join(base, "ro")}'`,
  ].join("\n");

/** Runs `scenario` against a fresh daemon whose profile carries `toolsToml`. */
async function withDaemon(label, toolsToml, scenario) {
  const profile = await mkdtemp(join(tmpdir(), `jarvis-files-${label}-`));
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
        'api_key_ref = "env:JARVIS_FILES_KEY"',
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
        "",
      ].join("\n"),
      "utf8",
    );
    const environment = { ...process.env, JARVIS_FILES_KEY: "placeholder" };
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
          ...(body ? { "content-type": "application/json", "idempotency-key": `files-${Date.now()}-${Math.random()}` } : {}),
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
    await scenario({ api, profile, waitFor, startRun, completed, pending, cli, stateOf });
  } finally {
    daemon?.kill();
    await sleep(300);
    await rm(profile, { recursive: true, force: true }).catch(() => {});
  }
}

try {
  // ---- balanced (the default) ----------------------------------------------------------------------------------
  const base = await layout();
  await withDaemon("balanced", rootsToml(base), async ({ api, waitFor, startRun, completed, pending, cli }) => {
    requests = [];
    propose("files_read_1", { root: "notes", path: "hello.txt" });
    const read = await startRun("read hello");
    check(await waitFor("the read to complete with no prompt", completed(read)), "a read ran with no prompt and no grant");
    check(toolText(requests.at(-1)).includes(HELLO), "the file's text reached the model");

    propose("files_write_1", { root: "notes", path: "out.txt", content: "written by jarvis" });
    const write = await startRun("write out");
    const approval = await waitFor("a prompt for the write", pending);
    const shown = JSON.stringify(approval);
    check(shown.includes("out.txt") && shown.includes("written by jarvis"), "the prompt shows the path and the start of the content", shown.slice(0, 400));
    check(!existsSync(join(base, "notes", "out.txt")), "nothing was written while the prompt was open");
    const decided = await cli("approvals", "approve", approval.approval_id, "--fingerprint", approval.action_fingerprint, "--version", String(approval.version), "--remember");
    check(decided.code === 0, "approved with --remember", decided.err.slice(0, 300));
    check(await waitFor("the write to complete", completed(write)), "the approved write completed");
    check(existsSync(join(base, "notes", "out.txt")) && (await readFile(join(base, "notes", "out.txt"), "utf8")) === "written by jarvis", "the file exists with the content");

    propose("files_write_1", { root: "notes", path: "second.txt", content: "second" });
    const again = await startRun("write second");
    check(await waitFor("the second write", completed(again)), "the next write ran without asking");
    check(existsSync(join(base, "notes", "second.txt")), "the second file exists");

    requests = [];
    propose("files_write_1", { root: "ro", path: "nope.txt", content: "x" });
    const blocked = await startRun("write into the read-only root");
    check(await waitFor("the refused write", completed(blocked)), "a write into a read-only root ended the run cleanly");
    check(!existsSync(join(base, "ro", "nope.txt")), "the read-only root was not written");
    check(toolText(requests.at(-1)).includes("tool.permission_denied"), "the model was told it is not permitted", toolText(requests.at(-1)).slice(0, 300));

    requests = [];
    propose("files_read_1", { root: "notes", path: "../secret.txt" });
    const escape = await startRun("read outside");
    check(await waitFor("the refused read", completed(escape)), "a path escaping the root ended the run cleanly");
    check(!everything().includes(SECRET), "nothing outside the root reached the model");

    propose("files_list_1", { root: "notes" });
    const listed = await startRun("list notes");
    check(await waitFor("the listing", completed(listed)), "a listing ran with no prompt");
    check(toolText(requests.at(-1)).includes("hello.txt"), "the listing names the files");
  });
  await rm(base, { recursive: true, force: true }).catch(() => {});

  // ---- autonomous ---------------------------------------------------------------------------------------------
  const base2 = await layout();
  await withDaemon("autonomous", rootsToml(base2, 'autonomy = "autonomous"'), async ({ waitFor, startRun, completed, pending }) => {
    requests = [];
    propose("files_write_1", { root: "notes", path: "auto.txt", content: "autonomous" });
    const run = await startRun("write auto");
    check(await waitFor("the write to complete", completed(run)), "autonomous wrote with no prompt");
    check(!(await pending()), "nothing was raised for a person to decide");
    check(existsSync(join(base2, "notes", "auto.txt")), "the file exists");
  });
  await rm(base2, { recursive: true, force: true }).catch(() => {});

  // ---- no roots: the tools are not offered ---------------------------------------------------------------------
  await withDaemon("none", "", async ({ waitFor, startRun, completed }) => {
    requests = [];
    const run = await startRun("hello");
    check(await waitFor("the run", completed(run)), "a profile with no roots runs");
    check(!everything().includes("files_read_1") && !everything().includes("files_write_1"), "no file tool was offered to the model");
  });
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  fake.close();
}

if (failures > 0) {
  console.error(`\n${failures} file-tool check(s) failed`);
  process.exit(1);
}
console.log("\nfile-tool checks passed");
