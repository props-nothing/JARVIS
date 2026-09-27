// The gated real-provider smoke test: Milestone 2's exit gate.
//
// "A gated real-provider smoke test streams a response." Every other test in this repository runs
// against the scripted provider or a fake server, so none of them proves that JARVIS can reach a
// **real** model, translate its stream, and record the answer. This harness does, and it is the only
// one that can.
//
// ## Why it is gated, and how the gate works
//
// It needs a real OpenAI-compatible endpoint. Rather than requiring a paid cloud account and a
// credential in an environment variable, it targets **Ollama on loopback**, which is the endpoint
// this project can reach without a TLS implementation — the adapter refuses a non-loopback host for
// exactly that reason, so a cloud endpoint is not testable here at all. Ollama also needs no API key,
// so the gate is "is a local server listening" rather than "is a secret present".
//
// **The gate is explicit and reported.** When no server answers, the harness prints a `skip` line
// naming what was missing and exits 0. Exiting non-zero would make a developer without Ollama unable
// to run the suite, and exiting 0 *silently* would let a CI run look like it had proved the real
// provider path when it had proved nothing — so the skip is announced.
//
// ## What it proves
//
// A daemon is configured for the endpoint (host, port, base path, and a model-name mapping), started
// for real, and driven through the **real control API and the real CLI**. The assertions are:
//
//   - the run reaches `completed` rather than `failed`, so the adapter's transport, framing,
//     translation, and the daemon's composition all worked;
//   - the run's durable events carry the model's own text, which is what makes this a proof about
//     the *provider* rather than about JARVIS's ability to answer from a script;
//   - `jarvis ask` prints that answer and exits 0, which exercises the client's SSE reader against a
//     real chunked stream — the path where a framing defect shows up as a truncated answer.
//
// ## Configuration
//
// Environment variables, all optional:
//
//   JARVIS_SMOKE_HOST    default 127.0.0.1
//   JARVIS_SMOKE_PORT    default 11434
//   JARVIS_SMOKE_PATH    default /v1          (Ollama mounts the route under /v1)
//   JARVIS_SMOKE_MODEL   default the first model the server reports
//   JARVIS_SMOKE_WIRE    default the model name exactly as the server reports it
//
// `JARVIS_SMOKE_WIRE` exists because the two namespaces differ: Ollama reports
// `glm-5.3-flash:cloud`, and a JARVIS model id is a lowercase dotted slug that cannot contain `:`.
// So the configured id is the server's name with the illegal characters replaced by `-`, and the wire
// name is the server's own spelling. The default derives both from `/api/tags`, so a plain run needs
// no variables at all.

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

/** How long to wait for the daemon to publish its discovery file. */
const START_TIMEOUT_MS = 15000;
/** How long to wait for a real model to answer. Generous: a cloud-proxied model is not local. */
const RUN_TIMEOUT_MS = 120000;

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/provider-smoke.mjs <bin-dir>");
  process.exit(2);
}
const daemonBin = join(binDir, process.platform === "win32" ? "jarvisd.exe" : "jarvisd");
const cliBin = join(binDir, process.platform === "win32" ? "jarvis.exe" : "jarvis");

const host = process.env.JARVIS_SMOKE_HOST ?? "127.0.0.1";
const port = process.env.JARVIS_SMOKE_PORT ?? "11434";
const basePath = process.env.JARVIS_SMOKE_PATH ?? "/v1";

let failures = 0;
function pass(message) {
  console.log(`ok    ${message}`);
}
function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail !== undefined) console.error(`      ${detail}`);
}
function skip(message) {
  console.log(`skip  ${message}`);
}

/**
 * Turns a provider's model name into the closest JARVIS-legal model id.
 *
 * A JARVIS model id is a lowercase dotted slug, so the provider's own name is not always expressible
 * as one — `glm-5.3-flash:cloud` contains a colon, and a provider may use `/` or `@`. This maps the
 * characters the grammar excludes onto `-` and lowercases, which is deterministic, so a re-run
 * configures the same id. The daemon is then told, through `model_names`, which provider name to send
 * for it — which is the whole point of that mapping existing.
 */
function canonicalModelId(name) {
  const mapped = name
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  // The grammar requires every dot-separated segment to begin and end alphanumerically, so a name
  // that became all separators has no legal id at all and the caller must be told rather than sent a
  // value the daemon will refuse.
  return mapped.split(".").filter((segment) => /^[a-z0-9](.*[a-z0-9])?$/.test(segment)).join(".");
}

/** Asks the server what it serves, and returns `null` when nothing is listening. */
async function probeServer() {
  try {
    const response = await fetch(`http://${host}:${port}/api/tags`, {
      signal: AbortSignal.timeout(3000),
    });
    if (!response.ok) return null;
    const body = await response.json();
    return Array.isArray(body?.models) ? body.models : null;
  } catch {
    return null;
  }
}

/** Waits for the daemon's discovery file, returning its parsed contents. */
async function awaitDiscovery(profile) {
  const path = join(profile, "run", "discovery.json");
  const deadline = Date.now() + START_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      try {
        return JSON.parse(await readFile(path, "utf8"));
      } catch {
        // A partially written file is expected; the publisher writes atomically but this may observe
        // the directory entry before the contents are visible.
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("the daemon never published a discovery file");
}

/** Reads the enrollment credential the daemon writes for its owner client. */
async function readCredential(profile) {
  const path = join(profile, "config", "client-credential");
  const deadline = Date.now() + START_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (existsSync(path)) {
      const text = (await readFile(path, "utf8")).trim();
      if (text !== "") return text;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("the daemon never issued a client credential");
}

/** Runs a command to completion, returning its exit code and combined output. */
function run(command, args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { ...options, shell: false });
    let output = "";
    child.stdout?.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.stderr?.on("data", (chunk) => {
      output += chunk.toString();
    });
    child.on("error", (error) => resolve({ code: -1, output: `${output}${error.message}` }));
    child.on("close", (code) => resolve({ code, output }));
  });
}

const profile = await mkdtemp(join(tmpdir(), "jarvis-provider-smoke-"));
let daemon;
let exitCode = 0;
/** Whether the gated half actually ran, so the summary does not claim a proof it did not make. */
let exercised = false;

try {
  // **The gate.** A missing server is a reported skip, never a silent pass: a CI log that said nothing
  // would look identical to one where the real provider path had been proved.
  const models = await probeServer();
  if (models === null) {
    skip(`no OpenAI-compatible server on ${host}:${port} — set JARVIS_SMOKE_PORT to run this`);
    exitCode = 0;
  } else {
    exercised = true;
    const wire = process.env.JARVIS_SMOKE_WIRE ?? models[0]?.name;
    const model = process.env.JARVIS_SMOKE_MODEL ?? canonicalModelId(wire ?? "");
    if (!wire || !model) {
      fail("the server reported no usable model", JSON.stringify(models));
    } else {
      console.log(`using ${host}:${port}${basePath} model=${model} wire=${wire}`);

      // A real configuration document, written where the daemon reads it. The credential is a
      // **placeholder in the environment**, never in the file: Ollama takes no key, and the file's
      // `api_key_ref` is a reference the daemon resolves at startup.
      await mkdir(join(profile, "config"), { recursive: true });
      const config = [
        "schema_version = 2",
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
      ].join("\n");
      await writeFile(join(profile, "config", "config.toml"), config, "utf8");

      const environment = { ...process.env, JARVIS_SMOKE_KEY: "smoke-placeholder" };
      daemon = spawn(daemonBin, ["--profile", profile], { env: environment, shell: false });
      const daemonLog = [];
      daemon.stdout?.on("data", (chunk) => daemonLog.push(chunk.toString()));
      daemon.stderr?.on("data", (chunk) => daemonLog.push(chunk.toString()));

      const discovery = await awaitDiscovery(profile);
      const credential = await readCredential(profile);

      // The daemon must be wired to the **configured** provider, not the scripted fallback. This is
      // the composition check, made over the API an operator would use.
      const status = await fetch(`${discovery.base_url}/api/v1/system/status`, {
        headers: { authorization: `Bearer ${credential}`, "jarvis-api-version": "1" },
        signal: AbortSignal.timeout(5000),
      });
      const statusBody = await status.json();
      const served = JSON.stringify(statusBody);
      if (served.includes("scripted.local")) {
        fail("the daemon composed the scripted provider, not the configured one", served.slice(0, 400));
      } else {
        pass("the daemon is running with the configured provider");
      }

      // A real run, created through the real API.
      const created = await fetch(`${discovery.base_url}/api/v1/runs`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${credential}`,
          "jarvis-api-version": "1",
          "content-type": "application/json",
          "idempotency-key": `smoke-${Date.now()}`,
        },
        body: JSON.stringify({
          conversation_id: null,
          input: { type: "text", text: "Reply with exactly: JARVIS PROVIDER SMOKE OK" },
          runtime: "jarvis-native",
        }),
        signal: AbortSignal.timeout(10000),
      });
      if (created.status !== 202) {
        fail(`the create was refused with ${created.status}`, (await created.text()).slice(0, 400));
      } else {
        const { run_id: runId } = await created.json();
        pass("a run was accepted against the configured provider");

        // Poll the run to a terminal state, bounded. Polling rather than streaming so a failure of the
        // *stream* cannot be confused with a failure of the *run*.
        const deadline = Date.now() + RUN_TIMEOUT_MS;
        let state = "received";
        while (Date.now() < deadline) {
          const read = await fetch(`${discovery.base_url}/api/v1/runs/${runId}`, {
            headers: { authorization: `Bearer ${credential}`, "jarvis-api-version": "1" },
            signal: AbortSignal.timeout(10000),
          });
          const body = await read.json();
          state = body.state;
          if (["completed", "failed", "cancelled"].includes(state)) break;
          await new Promise((resolve) => setTimeout(resolve, 250));
        }
        if (state !== "completed") {
          fail(`the run reached ${state} rather than completed`, daemonLog.join("").slice(-1500));
        } else {
          pass("the real provider run reached completed");
        }

        // The model's own text, read back from the durable events. This is the assertion that makes
        // the harness a proof about the provider: the scripted provider's fixed acknowledgement cannot
        // satisfy it, because the text is the model's answer to this prompt.
        const events = await fetch(`${discovery.base_url}/api/v1/runs/${runId}/events`, {
          headers: {
            authorization: `Bearer ${credential}`,
            "jarvis-api-version": "1",
            accept: "text/event-stream",
          },
          signal: AbortSignal.timeout(RUN_TIMEOUT_MS),
        });
        const streamed = await events.text();
        if (!streamed.includes("event: run.output_text.delta")) {
          fail("the run published no output delta", streamed.slice(0, 600));
        } else if (/scripted provider/i.test(streamed)) {
          fail("the answer came from the scripted provider", streamed.slice(0, 600));
        } else {
          pass("the model's own output is durable in the run's events");
        }
        if (!streamed.includes("event: run.completed")) {
          fail("the stream carried no completion event", streamed.slice(0, 600));
        }

        // The CLI's own reader, over the real chunked stream. This is the half a daemon-side assertion
        // cannot cover: `jarvis ask` decodes the response itself, so a framing defect on either side
        // shows up as a truncated answer or a non-zero exit.
        const asked = await run(cliBin, ["ask", "Reply with exactly: JARVIS CLI SMOKE OK", "--profile", profile], {
          env: environment,
        });
        if (asked.code !== 0) {
          fail(`jarvis ask exited ${asked.code}`, asked.output.slice(0, 600));
        } else if (!/JARVIS CLI SMOKE OK/.test(asked.output)) {
          fail("jarvis ask did not print the model's answer", asked.output.slice(0, 600));
        } else {
          pass("jarvis ask printed the model's answer and exited 0");
        }
      }
    }
  }
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  daemon?.kill();
  // A short grace so the daemon can release its lock and remove discovery before the profile goes.
  await new Promise((resolve) => setTimeout(resolve, 300));
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}

if (failures > 0) {
  console.error(`\n${failures} provider smoke check(s) failed`);
  exitCode = 1;
} else if (exercised) {
  console.log("\nreal-provider smoke checks passed");
} else {
  // **A skip is not a pass, and the summary says so.** Printing "checks passed" after running none
  // would make a CI log from a machine without a provider look identical to one that had proved the
  // real path — which is the single thing a gated test must not do.
  console.log("\nskipped: no provider endpoint, so nothing was proved about a real model");
}

process.exit(exitCode);
