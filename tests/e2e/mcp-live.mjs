// The gated live MCP journey: a real model proposes a call to an MCP server's tool, the call waits for a human
// approval, and only after `jarvis approvals approve` does the tool run and the model answer with its result.
//
// This closes the gap the deterministic suites cannot: they prove each layer (the MCP client, the tool
// pipeline, the approval store, the run controller) against doubles, and `mcp_conversation.rs` proves the
// composition against a scripted model. Nothing there shows that a **real model's own tool-call syntax**
// survives the adapter, reaches an **MCP child process** through the governed pipeline, and that the pipeline
// **withholds** the call until a person decides.
//
// The MCP server is the repository's own fixture (`examples/mcp_fixture_server.rs`), launched as a child
// process by the daemon — no third-party package is downloaded or executed. Its one tool, `read_file`, returns
// a fixed sentence the model cannot know, so the answer can only contain it if the call really ran.
//
// What is asserted, and why each is JARVIS's to guarantee rather than the model's:
//   - the daemon composed the server and offered its tool (`jarvis status` lists it);
//   - the model's proposal does **not** execute on its own: an approval exists and the sentence has not
//     reached the run (the pipeline withholds, whatever the model wanted);
//   - after the approval is decided through the CLI, the run completes and its durable events carry the
//     sentence, which only the child process could have produced.
// A model that declines to call the tool is reported as a skip of that leg, not a failure: whether a model
// chooses to use a tool is the model's behaviour, and a gate must not turn on it.
//
// Gated like `provider-smoke.mjs`. Needs the fixture built:
//   cargo build -p jarvisd -p jarvis-cli && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/mcp-live.mjs target/debug

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createHash } from "node:crypto";

/**
 * The principal the daemon derives for the enrolled `owner` client: the first sixteen bytes of the SHA-256 of
 * the client id, as a UUID. A client cannot ask the daemon for its own principal today, so a grant for
 * "myself" has to compute it — which is itself a gap worth recording rather than hiding.
 */
function ownerPrincipal() {
  const hex = createHash("sha256").update("owner").digest("hex").slice(0, 32);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

const START_TIMEOUT_MS = 15000;
const RUN_TIMEOUT_MS = 120000;
const SENTENCE = "fixture result for read_file";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/mcp-live.mjs <bin-dir>");
  process.exit(2);
}
const exe = process.platform === "win32" ? ".exe" : "";
const daemonBin = join(binDir, `jarvisd${exe}`);
const cliBin = join(binDir, `jarvis${exe}`);
const fixtureBin = resolve(join(binDir, "examples", `mcp_fixture_server${exe}`));
const host = process.env.JARVIS_SMOKE_HOST ?? "127.0.0.1";
const port = process.env.JARVIS_SMOKE_PORT ?? "11434";
const basePath = process.env.JARVIS_SMOKE_PATH ?? "/v1";

let failures = 0;
const pass = (message) => console.log(`ok    ${message}`);
const skip = (message) => console.log(`skip  ${message}`);
function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail !== undefined) console.error(`      ${detail}`);
}

const canonicalModelId = (name) =>
  name
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .split(".")
    .filter((segment) => /^[a-z0-9](.*[a-z0-9])?$/.test(segment))
    .join(".");

async function probeServer() {
  try {
    const response = await fetch(`http://${host}:${port}/api/tags`, { signal: AbortSignal.timeout(3000) });
    if (!response.ok) return null;
    const body = await response.json();
    return Array.isArray(body?.models) ? body.models : null;
  } catch {
    return null;
  }
}

async function waitFor(path, read) {
  const deadline = Date.now() + START_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      try {
        const value = await read(path);
        if (value) return value;
      } catch {
        // A partially written file is expected; try again.
      }
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out waiting for ${path}`);
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

/** Reassembles the model's answer from a run's output-text delta frames. */
function answerFrom(body) {
  const lines = body.split("\n");
  let text = "";
  for (let i = 0; i < lines.length; i += 1) {
    if (lines[i].trim() !== "event: run.output_text.delta") continue;
    try {
      const delta = JSON.parse(lines[i + 1].slice("data: ".length))?.payload?.delta;
      if (typeof delta === "string") text += delta;
    } catch {
      // A frame that does not parse contributes nothing.
    }
  }
  return text;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const profile = await mkdtemp(join(tmpdir(), "jarvis-mcp-live-"));
let daemon;
let exercised = false;

try {
  const models = await probeServer();
  if (models === null) {
    skip(`no OpenAI-compatible server on ${host}:${port} — set JARVIS_SMOKE_PORT to run this`);
  } else if (!existsSync(fixtureBin)) {
    skip(`the MCP fixture is not built (${fixtureBin}); run cargo build -p jarvis-infrastructure --example mcp_fixture_server`);
  } else {
    exercised = true;
    const wire = process.env.JARVIS_SMOKE_WIRE ?? models[0]?.name;
    const model = process.env.JARVIS_SMOKE_MODEL ?? canonicalModelId(wire ?? "");
    console.log(`using ${host}:${port}${basePath} model=${model} wire=${wire}`);
    await mkdir(join(profile, "config"), { recursive: true });
    await writeFile(
      join(profile, "config", "config.toml"),
      [
        "schema_version = 4",
        "",
        "[model]",
        'policy_id = "default"',
        'api_key_ref = "env:JARVIS_SMOKE_KEY"',
        "",
        "[model.provider]",
        'id = "local.smoke"',
        `host = "${host}"`,
        `port = ${port}`,
        `base_path = "${basePath}"`,
        `models = ["${model}"]`,
        `model_names = { "${model}" = "${wire}" }`,
        "",
        "[[mcp.servers]]",
        'name = "fixture-files"',
        // A TOML literal string, so a Windows path's backslashes are not escapes.
        `program = '${fixtureBin}'`,
        'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_FIXTURE_SELECT" }',
        "",
      ].join("\n"),
      "utf8",
    );
    const environment = {
      ...process.env,
      JARVIS_SMOKE_KEY: "smoke-placeholder",
      JARVIS_FIXTURE_SELECT: "standard",
    };
    daemon = spawn(daemonBin, ["--profile", profile], { env: environment, shell: false });
    const log = [];
    daemon.stdout?.on("data", (c) => log.push(c.toString()));
    daemon.stderr?.on("data", (c) => log.push(c.toString()));
    const discovery = await waitFor(join(profile, "run", "discovery.json"), async (p) => JSON.parse(await readFile(p, "utf8")));
    const credential = await waitFor(join(profile, "config", "client-credential"), async (p) => (await readFile(p, "utf8")).trim());
    const headers = { authorization: `Bearer ${credential}`, "jarvis-api-version": "1" };
    const api = async (method, path, body) => {
      const response = await fetch(`${discovery.base_url}${path}`, {
        method,
        headers: { ...headers, ...(body ? { "content-type": "application/json", "idempotency-key": `mcp-${Date.now()}-${Math.random()}` } : {}) },
        body: body ? JSON.stringify(body) : undefined,
        signal: AbortSignal.timeout(RUN_TIMEOUT_MS),
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

    const status = await api("GET", "/api/v1/system/status");
    const serverRow = status.json?.mcp?.servers?.find((s) => s.name === "fixture-files");
    if (!serverRow || serverRow.closed || serverRow.tools < 1) {
      fail("the daemon did not compose the fixture MCP server", `${status.text.slice(0, 400)}\n${log.join("").slice(-600)}`);
    } else {
      pass(`the daemon composed the MCP server with ${serverRow.tools} tool(s)`);

      // **Discovery grants nothing.** The tool is in the catalog and offered to the model, but the pipeline
      // refuses it `tool.permission_denied` until an operator grants it. The grant is the deliberate act this
      // journey performs through the real CLI, so the approval that follows is about *this* call.
      const granted = await run(
        cliBin,
        ["grants", "create", "--capability", "mcp.read_file@1", "--principal", ownerPrincipal(),
         // An MCP tool with no annotations is classified conservatively: the absence of a read-only claim is
         // recorded as `write`, at moderate risk. A grant may only narrow, so it names exactly that.
         "--effect", "write", "--risk", "moderate", "--sensitivity", "internal", "--profile", profile],
        { env: environment },
      );
      if (granted.code !== 0) {
        fail(`jarvis grants create exited ${granted.code}`, `${granted.err.slice(0, 500)} ${granted.out.slice(0, 300)}`);
      } else {
        pass("an operator grant was written for the MCP tool through the CLI");
      }

      const created = await api("POST", "/api/v1/runs", {
        conversation_id: null,
        input: {
          type: "text",
          text:
            "Call the read_file tool once with the path notes.txt, then reply with exactly the text the tool returned and nothing else.",
        },
        runtime: "jarvis-native",
      });
      if (created.status !== 202) {
        fail(`the create was refused with ${created.status}`, created.text.slice(0, 400));
      } else {
        const runId = created.json.run_id;
        // Poll until the run needs a person, finishes, or the bound passes.
        const deadline = Date.now() + RUN_TIMEOUT_MS;
        let state = "received";
        let approval;
        while (Date.now() < deadline) {
          const read = await api("GET", `/api/v1/runs/${runId}`);
          state = read.json?.state ?? state;
          const listed = await api("GET", "/api/v1/approvals");
          approval = listed.json?.approvals?.[0];
          if (approval || ["completed", "failed", "cancelled"].includes(state)) break;
          await sleep(500);
        }
        if (!approval) {
          if (state === "completed") {
            const events = await api("GET", `/api/v1/runs/${runId}/events`);
            const types = events.text.split("\n").filter((l) => l.startsWith("event: ")).map((l) => l.slice(7));
            const called = types.some((type) => type.includes("tool"));
            if (called) {
              fail("the model called the tool but no approval was raised", `events [${[...new Set(types)].join(",")}]\nanswer: ${answerFrom(events.text)}`);
            } else {
              skip(`the model answered without calling the tool (events: ${[...new Set(types)].join(",")}), so the governed-call leg was not exercised`);
            }
          } else {
            fail(`the run reached ${state} with no approval`, log.join("").slice(-800));
          }
        } else {
          pass(`the model's tool call was withheld and an approval was raised (run state ${state})`);
          // Not the event stream: it follows a live run and would not end while the run waits. The run is
          // still working (not terminal) and the approval is undecided, which is the withheld state.
          const waiting = await api("GET", `/api/v1/runs/${runId}`);
          if (["completed", "failed", "cancelled"].includes(waiting.json?.state) || approval.state !== "pending") {
            fail("the call was not withheld pending the approval", `run ${waiting.json?.state}, approval ${approval.state}`);
          } else {
            pass("the run is waiting and the approval is undecided: the tool has not run");
          }
          const decided = await run(
            cliBin,
            ["approvals", "approve", approval.approval_id, "--fingerprint", approval.action_fingerprint, "--version", String(approval.version), "--profile", profile],
            { env: environment },
          );
          if (decided.code !== 0) {
            fail(`jarvis approvals approve exited ${decided.code}`, `${decided.err.slice(0, 400)} ${decided.out.slice(0, 300)}`);
          } else {
            pass("the approval was decided through the CLI");
            const recorded = await api("GET", `/api/v1/approvals/${approval.approval_id}`);
            // `consumed` is the state after the resumed run spent the approval, which can already have happened by now.
            if (!["approved", "consumed"].includes(recorded.json?.state)) {
              fail("the decision was not recorded as approved", recorded.text.slice(0, 300));
            } else {
              pass(`the approval is recorded as ${recorded.json?.state}`);
            }
            // The decision continues the **same run**: the parked call is released through the pipeline, the tool
            // runs in the MCP child, and the model answers from its result. The wait is the run's own bound, because
            // a real model's second turn takes as long as a model takes.
            let final = "";
            const resumeDeadline = Date.now() + RUN_TIMEOUT_MS;
            while (Date.now() < resumeDeadline) {
              const read = await api("GET", `/api/v1/runs/${runId}`);
              final = read.json?.state ?? final;
              if (["completed", "failed", "cancelled"].includes(final)) break;
              await sleep(1000);
            }            if (final === "completed") {
              const events = await api("GET", `/api/v1/runs/${runId}/events`);
              // The answer arrives as token-sized deltas, so the sentence is only contiguous once reassembled.
              if (answerFrom(events.text).includes(SENTENCE)) {
                pass("after approval the tool ran in the MCP child and the model answered with its result");
              } else {
                fail("the run completed but its events never carried the tool's sentence", answerFrom(events.text));
              }
            } else {
              fail(`after approval the run did not complete (it is ${final || "unknown"})`, log.join("").slice(-600));
            }
          }
        }
      }
    }
  }
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  daemon?.kill();
  await sleep(300);
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}

if (failures > 0) {
  console.error(`\n${failures} live MCP check(s) failed`);
  process.exit(1);
} else if (exercised) {
  console.log("\nlive MCP checks passed");
} else {
  console.log("\nskipped: nothing was proved about a real model calling an MCP tool");
}
