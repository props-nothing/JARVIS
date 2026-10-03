// The gated live memory journey: remember through the CLI, ask a real model, restart, forget.
//
// `tests/memory_recall.rs` proves from a fake provider's wire request that a remembered line reaches the
// model. This proves the same thing end to end against a **real model**, through the **real binaries**
// (`jarvisd` and `jarvis`), so a defect in the CLI's request, the daemon's composition, the recall, or the
// adapter against a real stream fails here and nowhere else.
//
// The memory is a deliberately arbitrary fact the model cannot know — a favourite slide-deck colour — so the
// only way the answer can contain it is that JARVIS put it in the prompt. The assertions are about what
// JARVIS owns (the fact reached the model; it stopped reaching it after `forget`), never about a model
// following an instruction in general.
//
// Gated like `provider-smoke.mjs`: with no server on the Ollama port it prints `skip` and exits 0, and says
// so in the summary rather than pretending to have proved anything.
//
//   JARVIS_SMOKE_HOST / _PORT / _PATH / _MODEL / _WIRE  as in provider-smoke.mjs

import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const START_TIMEOUT_MS = 15000;
const RUN_TIMEOUT_MS = 120000;
const FACT = "My favourite colour for slide decks is vermilion.";
const QUESTION = "Which colour should you use for my slide decks? Answer with just the colour.";
const UNRELATED = "What is two plus two? Answer with just the number.";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/memory-live.mjs <bin-dir>");
  process.exit(2);
}
const daemonBin = join(binDir, process.platform === "win32" ? "jarvisd.exe" : "jarvisd");
const cliBin = join(binDir, process.platform === "win32" ? "jarvis.exe" : "jarvis");
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

function canonicalModelId(name) {
  const mapped = name
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return mapped.split(".").filter((segment) => /^[a-z0-9](.*[a-z0-9])?$/.test(segment)).join(".");
}

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
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${path}`);
}

function run(command, args, options = {}) {
  return new Promise((resolve) => {
    const child = spawn(command, args, { ...options, shell: false });
    let out = "";
    let err = "";
    child.stdout?.on("data", (chunk) => (out += chunk.toString()));
    child.stderr?.on("data", (chunk) => (err += chunk.toString()));
    child.on("error", (error) => resolve({ code: -1, out: `${out}${error.message}`, err }));
    child.on("close", (code) => resolve({ code, out, err }));
  });
}

const profile = await mkdtemp(join(tmpdir(), "jarvis-memory-live-"));
const environment = { ...process.env, JARVIS_SMOKE_KEY: "smoke-placeholder" };
let daemon;
let exercised = false;

/** Starts a daemon over the profile and waits until it is reachable. */
async function startDaemon() {
  daemon = spawn(daemonBin, ["--profile", profile], { env: environment, shell: false });
  const log = [];
  daemon.stdout?.on("data", (chunk) => log.push(chunk.toString()));
  daemon.stderr?.on("data", (chunk) => log.push(chunk.toString()));
  await waitFor(join(profile, "run", "discovery.json"), async (path) => JSON.parse(await readFile(path, "utf8")));
  return log;
}

/** Stops the daemon and waits for it to release its lock, so a restart is a real restart. */
async function stopDaemon() {
  const exited = new Promise((resolve) => daemon.once("close", resolve));
  daemon.kill();
  await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 5000))]);
  await new Promise((resolve) => setTimeout(resolve, 500));
  // On Windows `kill()` ends the process abruptly, so its discovery file is left behind. Waiting for the
  // *new* daemon means not reading that stale one, which would be a race in this harness, not a product
  // fault: a client reading a stale file is told the daemon is unreachable.
  await rm(join(profile, "run", "discovery.json"), { force: true });
}

const cli = (args) => run(cliBin, [...args, "--profile", profile], { env: environment });

/** Asks through the real CLI and returns what it printed. */
async function ask(question) {
  const result = await cli(["ask", question]);
  return result;
}

try {
  const models = await probeServer();
  if (models === null) {
    skip(`no OpenAI-compatible server on ${host}:${port} — set JARVIS_SMOKE_PORT to run this`);
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
      ].join("\n"),
      "utf8",
    );

    let log = await startDaemon();

    // Control first: with nothing remembered, the model cannot name the colour.
    const before = await ask(QUESTION);
    if (before.code !== 0) {
      fail(`jarvis ask exited ${before.code} before anything was remembered`, `${before.err.slice(0, 400)}\n${log.join("").slice(-800)}`);
    } else if (/vermilion/i.test(before.out)) {
      fail("the model named the colour before it was remembered, so this journey proves nothing", before.out.slice(0, 200));
    } else {
      pass("with nothing remembered, the model does not know the colour");
    }

    const remembered = await cli(["memory", "remember", FACT]);
    if (remembered.code !== 0 || !remembered.out.includes("vermilion")) {
      fail(`jarvis memory remember exited ${remembered.code}`, `${remembered.out.slice(0, 300)} ${remembered.err.slice(0, 300)}`);
    } else {
      pass("jarvis memory remember stored the fact");
    }

    const found = await cli(["memory", "search", "slide decks colour"]);
    if (found.code !== 0 || !found.out.includes("vermilion")) {
      fail("jarvis memory search did not find the fact", `${found.out.slice(0, 300)} ${found.err.slice(0, 300)}`);
    } else {
      pass("jarvis memory search found it by words");
    }

    const after = await ask(QUESTION);
    if (after.code !== 0) {
      fail(`jarvis ask exited ${after.code} after remembering`, after.err.slice(0, 400));
    } else if (!/vermilion/i.test(after.out)) {
      fail("the real model's answer did not carry the remembered fact", JSON.stringify(after.out.slice(0, 300)));
    } else {
      pass("the real model answered with the remembered fact");
    }

    const unrelated = await ask(UNRELATED);
    if (unrelated.code !== 0 || /vermilion/i.test(unrelated.out)) {
      fail("an unrelated question was affected by the memory", `${unrelated.code} ${unrelated.out.slice(0, 200)}`);
    } else {
      pass("an unrelated question is unaffected");
    }

    // A real restart: the process is gone, the profile is not.
    await stopDaemon();
    log = await startDaemon();
    const restarted = await ask(QUESTION);
    if (restarted.code !== 0 || !/vermilion/i.test(restarted.out)) {
      fail("the memory did not survive a daemon restart", `${restarted.code} ${restarted.out.slice(0, 200)} ${restarted.err.slice(0, 200)}`);
    } else {
      pass("after a daemon restart the model still answers with it");
    }

    // Forget it, and the model stops knowing.
    const listing = await cli(["memory", "list"]);
    let memoryId;
    try {
      memoryId = JSON.parse(listing.out).memories?.[0]?.memory_id;
    } catch {
      memoryId = undefined;
    }
    if (!memoryId) {
      fail("jarvis memory list printed no memory to forget", listing.out.slice(0, 300));
    } else {
      const forgotten = await cli(["memory", "forget", memoryId]);
      if (forgotten.code !== 0) {
        fail(`jarvis memory forget exited ${forgotten.code}`, forgotten.err.slice(0, 300));
      } else {
        const gone = await ask(QUESTION);
        if (gone.code !== 0 || /vermilion/i.test(gone.out)) {
          fail("the model still knew the colour after it was forgotten", `${gone.code} ${gone.out.slice(0, 200)}`);
        } else {
          pass("after forget, the model no longer knows the colour");
        }
      }
    }
    void log;
  }
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  daemon?.kill();
  await new Promise((resolve) => setTimeout(resolve, 300));
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}

if (failures > 0) {
  console.error(`\n${failures} live memory check(s) failed`);
  process.exit(1);
} else if (exercised) {
  console.log("\nlive memory checks passed");
} else {
  console.log("\nskipped: no provider endpoint, so nothing was proved about a real model");
}
