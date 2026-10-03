// The MCP tool-offer journey: a real `jarvisd` composes a real MCP server and the model is **told about its tool**.
//
// Not gated on a model or a network. A local fake OpenAI-compatible server records the request body the daemon
// sends, so the assertion is on the wire. What it proves, and why a unit test cannot:
//
//   - the daemon composed the declared MCP server (the repository's own fixture, launched as a child process);
//   - that server's tool is in the **catalog** the pipeline resolves against, and therefore **offered to the
//     model** with its real description and argument schema. Before this was wired, the daemon composed MCP
//     servers and could route to them, but built the catalog from native tools alone, so the model was never
//     told an MCP tool existed;
//   - the offer is the tool's own schema (the fixture's tool requires `path`), not a bare object, which is what
//     a model needs to supply valid arguments — a live run showed a model guessing wrong without it.
//
//   cargo build -p jarvisd && cargo build -p jarvis-infrastructure --example mcp_fixture_server
//   node tests/e2e/mcp-tool-offer.mjs target/debug

import http from "node:http";
import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile, mkdir, readFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node tests/e2e/mcp-tool-offer.mjs <bin-dir>");
  process.exit(2);
}
const exe = process.platform === "win32" ? ".exe" : "";
const daemonBin = join(binDir, `jarvisd${exe}`);
const fixtureBin = resolve(join(binDir, "examples", `mcp_fixture_server${exe}`));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let failures = 0;
const pass = (message) => console.log(`ok    ${message}`);
function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail !== undefined) console.error(`      ${detail}`);
}

if (!existsSync(fixtureBin)) {
  console.log(`skip  the MCP fixture is not built (${fixtureBin})`);
  console.log("\nskipped: nothing was proved about the tool offer");
  process.exit(0);
}

let captured = null;
const fake = http
  .createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      captured ??= body;
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write('data: {"id":"1","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}\n\n');
      res.write('data: {"id":"1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n');
      res.end("data: [DONE]\n\n");
    });
  })
  .listen(0, "127.0.0.1");
await new Promise((r) => fake.once("listening", r));
const port = fake.address().port;

const profile = await mkdtemp(join(tmpdir(), "jarvis-mcp-offer-"));
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
      'api_key_ref = "env:JARVIS_OFFER_KEY"',
      "",
      "[model.provider]",
      'id = "local.offer"',
      'host = "127.0.0.1"',
      `port = ${port}`,
      'base_path = "/v1"',
      'models = ["model-one"]',
      "",
      "[[mcp.servers]]",
      'name = "fixture-files"',
      `program = '${fixtureBin}'`,
      'env = { JARVIS_MCP_FIXTURE = "env:JARVIS_OFFER_SELECT" }',
      "",
    ].join("\n"),
    "utf8",
  );
  daemon = spawn(daemonBin, ["--profile", profile], {
    env: { ...process.env, JARVIS_OFFER_KEY: "placeholder", JARVIS_OFFER_SELECT: "standard" },
    shell: false,
  });
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
    await sleep(100);
  }
  if (!discovery || !credential) {
    fail("the daemon never became reachable", "its startup errors are in the profile's log directory");
  } else {
    const response = await fetch(`${discovery.base_url}/api/v1/runs`, {
      method: "POST",
      headers: {
        authorization: `Bearer ${credential}`,
        "jarvis-api-version": "1",
        "content-type": "application/json",
        "idempotency-key": `offer-${Date.now()}`,
      },
      body: JSON.stringify({ conversation_id: null, input: { type: "text", text: "hello" }, runtime: "jarvis-native" }),
    });
    if (response.status !== 202) {
      fail(`the run was refused with ${response.status}`, (await response.text()).slice(0, 300));
    } else {
      for (let i = 0; i < 100 && captured === null; i += 1) await sleep(100);
      const tools = captured ? JSON.parse(captured).tools : undefined;
      const mcp = tools?.find((tool) => tool.function.name === "mcp.read_file@1");
      if (!mcp) {
        fail("the model was not offered the MCP server's tool", JSON.stringify(tools?.map((t) => t.function.name)));
      } else {
        pass("the model is offered the MCP server's tool");
        if (typeof mcp.function.description === "string" && mcp.function.description.length > 0) {
          pass("with a description");
        } else {
          fail("the offered tool has no description", JSON.stringify(mcp.function));
        }
        const required = mcp.function.parameters?.required ?? [];
        if (required.includes("path") && mcp.function.parameters?.properties?.path?.type === "string") {
          pass("with the tool's own argument schema, not a bare object");
        } else {
          fail("the offered tool's parameters are not the tool's schema", JSON.stringify(mcp.function.parameters));
        }
      }
      if (!tools?.some((tool) => tool.function.name === "clock.now@1")) {
        fail("the native tool disappeared from the offer", JSON.stringify(tools?.map((t) => t.function.name)));
      } else {
        pass("the native tool is still offered beside it");
      }
    }
  }
} catch (error) {
  fail("the harness could not complete", error?.stack ?? String(error));
} finally {
  daemon?.kill();
  fake.close();
  await sleep(300);
  await rm(profile, { recursive: true, force: true }).catch(() => {});
}

if (failures > 0) {
  console.error(`\n${failures} MCP tool-offer check(s) failed`);
  process.exit(1);
}
console.log("\nMCP tool-offer checks passed");
