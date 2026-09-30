// Approval surface journey.
//
// Proves that the approval use cases are reachable from a **real** `jarvisd` rather than only from an
// in-process router. The handler tests in `jarvis_infrastructure::http::approval` build their own
// `ApiState` with `.with_approvals(...)`, so they prove the handlers work when the state carries a
// service. They cannot prove the *daemon composition* attaches one — a daemon that never called
// `.with_approvals` would still pass every one of them while answering `service.not_ready` to every
// real client. That gap is precisely what a composition root needs an end-to-end test for, and it is
// the same defect class `BRN-014` fixed for the policy surface.
//
// ## What it seeds, and why directly into the database
//
// No executor exists, so nothing in the product creates an approval over the API: a request comes from
// a policy `Ask` decision on a tool call, and there is no tool invocation path yet. The harness
// therefore writes one approval row into the profile's own database — the same thing a future `Ask`
// decision would leave behind — and then exercises the real HTTP surface against it. That is honest
// about which half is being tested: the *lifecycle over the wire*, not the request creation, which is
// named as absent in `approval_service`'s own module docs.
//
// ## What it asserts
//
// 1. The listing reaches the composed service: `200` with the seeded approval and its preview, rather
//    than `503 service.not_ready`.
// 2. A decision over the wire applies, records the **server-derived** principal and channel, and writes
//    its audit row.
// 3. A repeated decision answers `applied: false` — the contract's "same-key/same-request retry returns
//    the original decision".
// 4. A decision whose fingerprint does not match the approved one is refused with the contract's own
//    code, and the record is untouched.
// 5. Revocation/decision durability: the decision survives a restart, because terminal decisions are
//    immutable.
//
// Dependency-free (Node standard library only), matching the other harnesses in this directory.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import process from "node:process";

const READY_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const SHUTDOWN_TIMEOUT_MS = 20_000;
/** The API major this harness speaks. A mismatch is refused, so it is sent explicitly. */
const API_MAJOR = 1;

let failures = 0;

function fail(message, detail) {
  failures += 1;
  console.error(`FAIL  ${message}`);
  if (detail) {
    console.error(String(detail).slice(0, 2000));
  }
}

function pass(message) {
  console.log(`ok    ${message}`);
}

/** Paths to the two binaries, resolved from a build directory. */
function resolveBinaries() {
  const directory = process.argv[2];
  if (!directory) {
    throw new Error("usage: approval-journey.mjs <directory containing jarvis and jarvisd>");
  }
  const suffix = process.platform === "win32" ? ".exe" : "";
  const daemon = join(directory, `jarvisd${suffix}`);
  const client = join(directory, `jarvis${suffix}`);
  for (const path of [daemon, client]) {
    if (!existsSync(path)) {
      throw new Error(`missing built binary: ${path}`);
    }
  }
  return { daemon, client };
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8", timeout: 60_000, windowsHide: true });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

async function sleep(milliseconds) {
  await new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/** Starts the daemon and waits until `status` reports readiness. */
async function startAndWait(daemon, client, profile) {
  const child = spawn(daemon, ["--profile", profile], {
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  const captured = [];
  child.stdout.on("data", (chunk) => captured.push(String(chunk)));
  child.stderr.on("data", (chunk) => captured.push(String(chunk)));
  let exited = false;
  child.once("exit", () => {
    exited = true;
  });

  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (exited) {
      throw new Error(`the daemon exited before becoming ready\n${captured.join("")}`);
    }
    const status = run(client, ["--profile", profile, "status"]);
    if (status.status === 0 && /state:\s*ready/.test(status.stdout)) {
      return child;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  throw new Error(`the daemon was not ready within ${READY_TIMEOUT_MS}ms`);
}

/** Stops the daemon and waits for the process to exit. */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  const deadline = Date.now() + SHUTDOWN_TIMEOUT_MS;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      return;
    }
    await sleep(POLL_INTERVAL_MS);
  }
  child.kill("SIGKILL");
  throw new Error(`the daemon did not stop within ${SHUTDOWN_TIMEOUT_MS}ms`);
}

/** Opens the profile's database. `readOnly` is false only for the seeding step. */
async function openDatabase(profile, readOnly) {
  const path = join(profile, "data", "db", "jarvis.sqlite");
  if (!existsSync(path)) {
    throw new Error(`the profile has no database at ${path}`);
  }
  let DatabaseSync;
  try {
    ({ DatabaseSync } = await import("node:sqlite"));
  } catch (error) {
    throw new Error(`this journey needs Node's built-in node:sqlite module: ${error.message}`);
  }
  return new DatabaseSync(path, { readOnly });
}

/** Reads the daemon's discovery record for a profile. */
function discoveryFor(profile) {
  const path = join(profile, "run", "discovery.json");
  if (!existsSync(path)) {
    throw new Error(`the daemon published no discovery at ${path}`);
  }
  const record = JSON.parse(readFileSync(path, "utf8"));
  if (typeof record.base_url !== "string" || !record.base_url.startsWith("http://127.0.0.1:")) {
    throw new Error(`discovery named a non-loopback authority: ${record.base_url}`);
  }
  return record;
}

/** Reads the enrolled client credential for a profile. */
function credentialFor(profile) {
  const path = join(profile, "config", "client-credential");
  if (!existsSync(path)) {
    throw new Error(`the profile has no client credential at ${path}`);
  }
  return readFileSync(path, "utf8").trim();
}

/** One HTTP request to the daemon. */
function request(record, credential, method, path, body, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, record.base_url);
    const headers = {
      // The authority the daemon actually bound, because it refuses any other `Host`.
      Host: url.host,
      Authorization: `Bearer ${credential}`,
      // Version negotiation is checked before authentication, so omitting this makes every request
      // fail with `api.version_unsupported` regardless of credentials.
      "Jarvis-API-Version": String(API_MAJOR),
      ...extraHeaders,
    };
    let payload;
    if (body !== undefined) {
      payload = JSON.stringify(body);
      headers["Content-Type"] = "application/json";
      headers["Content-Length"] = Buffer.byteLength(payload);
    }
    const call = http.request(
      { hostname: url.hostname, port: url.port, method, path: url.pathname + url.search, headers },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => chunks.push(chunk));
        response.on("end", () => {
          const text = Buffer.concat(chunks).toString("utf8");
          let json;
          try {
            json = text.length > 0 ? JSON.parse(text) : undefined;
          } catch {
            json = undefined;
          }
          resolve({ status: response.statusCode, text, json });
        });
      },
    );
    call.setTimeout(20_000, () => call.destroy(new Error("the request timed out")));
    call.on("error", reject);
    if (payload !== undefined) {
      call.write(payload);
    }
    call.end();
  });
}

/** The idempotency key for a write. Clock-derived: it is only compared for presence. */
let keyCounter = 0;
function idempotencyKey(label) {
  keyCounter += 1;
  return `${label}-${Date.now()}-${keyCounter}`;
}

const APPROVAL_ID = "01930000-0000-7000-8000-0000000000f1";
// A **second** approval whose deadline is already past when the daemon reads it. Seeded alongside the
// first so the read-path expiry can be asserted at the composed surface: the contract says expiry is
// evaluated on every read, and the two surfaces that *show* a prompt are the listing and the detail read.
// A distinct id, because the point is that both rows exist and only one survives the read.
const LAPSED_ID = "01930000-0000-7000-8000-0000000000f2";
// A **third**, live approval, for the paging check. `has_more` is only true when more rows remain than
// the page bound, and the bound is one — so a cursor cannot be reached with a single live row. Its
// deadline is later than the first row's, so page one is unambiguously the earliest-lapsing row and the
// cursor's position is testable rather than coincidental.
const PAGING_ID = "01930000-0000-7000-8000-0000000000f3";
// A **critical-risk** approval, so the composed surface can prove the step-up rule the contract states
// ("critical actions default to step-up") is enforced by a real daemon rather than only by a service unit
// test. Seeded rather than driven, because the risk label comes from the tool definition's review and no
// executor sets one — the same reason the other rows are seeded.
const CRITICAL_ID = "01930000-0000-7000-8000-0000000000f4";
// A deadline in the past **relative to the daemon's real clock**, which is what makes this row lapsed
// without any test-only knob. The other fixture uses 2030 for the mirror-image reason.
const LAPSED_DEADLINE = "2020-01-01T00:00:00Z";
const FINGERPRINT = `sha256:${"ab".repeat(32)}`;
// The schema fingerprint the seeded tool identity carries. A **distinct** value from the action
// fingerprint, because the two are different facts: the schema identifies the tool's input contract while
// the action fingerprint binds one exact call. A fixture that reused one value for both would let a
// projection that swapped them pass.
const SCHEMA_FINGERPRINT = `sha256:${"cd".repeat(32)}`;
// The operator's own comment on the decision, asserted on the way back out. A value with a space and mixed
// case rather than a single word, so a round trip that trimmed or lowercased it would fail rather than pass
// by coincidence.
const DECISION_COMMENT = "checked the recipient, asked Bob";

/**
 * A canonical `ToolIdentity` document, spelled as the domain's serializer emits it.
 *
 * `capability` is `namespace.name@major` with **exactly one dot** — the type refuses a second, and a
 * document it refuses is read as corruption rather than as a reservation, so a near-miss shape here
 * would fail the journey for the wrong reason.
 */
function toolIdentityDocument() {
  return JSON.stringify({
    capability: "mail.send@1",
    source: { kind: "connector", owner: "acme.mail", version: "1.0.0" },
    schema_fingerprint: SCHEMA_FINGERPRINT,
  });
}

/**
 * Seeds one pending approval, exactly as a policy `Ask` decision would have left it.
 *
 * The scope columns come from the run the create path wrote, so the workspace and principal are the
 * ones the daemon itself resolves for this client rather than invented values — a row seeded outside
 * the caller's scope would make every read a `not_found` for the wrong reason.
 */
async function seedApproval(profile) {
  const database = await openDatabase(profile, false);
  try {
    const run = database
      .prepare("SELECT id, workspace_id, principal_id FROM agent_runs ORDER BY created_at DESC LIMIT 1")
      .get();
    if (!run) {
      throw new Error("no run exists to attach the approval to");
    }
    const insert = database.prepare(
      "INSERT INTO approvals (id, workspace_id, requesting_principal_id, run_id, tool_call_id, " +
        "tool_identity_json, action_fingerprint, risk, effects_json, summary, preview_json, " +
        "allowed_channels_json, expires_at, scope, state, version, created_at, updated_at) " +
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[\"write\"]', ?, ?, '[\"api\"]', ?, 'one_shot', " +
        "'pending', 1, ?, ?)",
    );
    // The risk is a parameter rather than a literal because one row must be **critical** (see
    // `CRITICAL_ID`): the step-up rule turns on the record's own risk, so a fixture that could only seed
    // `high` rows could not exercise it at all.
    const seed = (id, toolCall, deadline, risk) =>
      insert.run(
        id,
        run.workspace_id,
        // The requester is the same principal the client resolves to, so the cancellation path — where
        // the requester may always withdraw its own request — is genuinely exercised.
        run.principal_id,
        run.id,
        toolCall,
        toolIdentityDocument(),
        FINGERPRINT,
        risk,
        "Send one email to peter@example.com",
        JSON.stringify([{ key: "to", value: "peter@example.com" }]),
        deadline,
        "2026-09-27T12:00:00Z",
        "2026-09-27T12:00:00Z",
      );

    // Far in the future: the handler reads the real clock, so a near deadline would make the decision
    // tests exercise expiry instead of the decision path.
    seed(APPROVAL_ID, "01930000-0000-7000-8000-0000000000a1", "2030-01-01T00:00:00Z", "high");
    // **A third row, live and later-lapsing than the first**, so the listing has more pending rows than
    // the paging check's bound of one — without it `has_more` is false and the cursor is unreachable,
    // which is why the paging assertion below needs a second *live* row rather than the lapsed one. Its
    // deadline is later so the first page is unambiguously the earliest-lapsing row, which is the order
    // the cursor is a position in.
    //
    // It does **not** disturb the "exactly three live" assertion: that one reads the unbounded listing,
    // where the paging row is one of the live rows and the count already accounts for it.
    seed(PAGING_ID, "01930000-0000-7000-8000-0000000000a3", "2030-06-01T00:00:00Z", "high");
    // And its mirror image: already past, so the read-path sweep has something real to find. Seeded
    // rather than driven, because no path in this build sets a deadline — the `Ask` executor that would
    // does not exist, which the module docs name as absent.
    seed(LAPSED_ID, "01930000-0000-7000-8000-0000000000a2", LAPSED_DEADLINE, "high");
    // The critical row, live, so the step-up refusal is reachable without disturbing the count above —
    // it is decided (and refused) in its own section, and the listing count is asserted before it.
    seed(CRITICAL_ID, "01930000-0000-7000-8000-0000000000a4", "2030-07-01T00:00:00Z", "critical");
    return run;
  } finally {
    database.close();
  }
}

/** Reads one approval row back. */
async function readRow(profile, id) {
  const database = await openDatabase(profile, true);
  try {
    return database
      .prepare(
        "SELECT state, version, decided_by, decided_via, decided_assurance FROM approvals WHERE id = ?",
      )
      .get(id);
  } finally {
    database.close();
  }
}

/** Counts the audit rows for an approval. */
async function countTransitions(profile, id) {
  const database = await openDatabase(profile, true);
  try {
    return database
      .prepare("SELECT count(*) AS n FROM approval_transitions WHERE approval_id = ?")
      .get(id).n;
  } finally {
    database.close();
  }
}

async function main() {
  const { daemon, client } = resolveBinaries();
  const profile = mkdtempSync(join(tmpdir(), "jarvis-e2e-approval-"));
  let running = null;

  try {
    // ---------------------------------------------------------------------
    // 1. Seed. The daemon runs once first so the profile has a migrated database, an enrolled client,
    //    and a run for the approval's foreign key.
    // ---------------------------------------------------------------------
    running = await startAndWait(daemon, client, profile);
    pass("the daemon started and reported ready on a fresh profile");

    const seeded = run(client, ["--profile", profile, "ask", "seed a run for the approval journey"]);
    if (seeded.status !== 0) {
      throw new Error(`seeding a run failed: ${seeded.stderr || seeded.stdout}`);
    }
    await stop(running);
    running = null;
    pass("the daemon stopped so the profile holds a clean, migrated database");

    const seededRun = await seedApproval(profile);
    pass("a pending approval is seeded in the caller's own workspace and principal scope");

    // ---------------------------------------------------------------------
    // 2. The composed surface answers. A daemon that never attached the approval service would answer
    //    `503 service.not_ready` here, which is the defect this journey exists to catch.
    // ---------------------------------------------------------------------
    running = await startAndWait(daemon, client, profile);
    const record = discoveryFor(profile);
    const credential = credentialFor(profile);
    pass("the daemon restarted and reported ready");

    const listed = await request(record, credential, "GET", "/api/v1/approvals");
    if (listed.status !== 200) {
      fail(
        `**the approval surface must be reachable from a real daemon**: got ${listed.status}; a ` +
          "`service.not_ready` here means the composition root never attached the service",
        listed.text,
      );
    } else if (!Array.isArray(listed.json?.approvals)) {
      fail("the listing must contain an approvals array", listed.text);
    } else if (listed.json.approvals.length !== 3) {
      // **Three live rows plus one past its deadline, and this count is itself the listing half of the
      // read-path expiry assertion**: a surface that offered the lapsed row would report four here
      // before section 2c could say so precisely. The second live row is what makes the paging check
      // reachable, since a cursor needs more rows than the page bound, and the third is the critical one
      // the step-up check decides — keeping this count exact is what stops any of them from becoming an
      // unnoticed extra live row. The first view is asserted below, and it is the earliest-lapsing row,
      // which is the order the cursor is a position in.
      fail(
        `exactly three approvals are live (the fourth is lapsed), got ${listed.json.approvals.length}`,
        listed.text,
      );
    } else {
      const view = listed.json.approvals[0];
      const problems = [];
      if (view.approval_id !== APPROVAL_ID) {
        problems.push(`approval_id=${view.approval_id}`);
      }
      if (view.state !== "pending") {
        problems.push(`state=${view.state}`);
      }
      if (view.tool_id !== "mail.send@1") {
        problems.push(`tool_id=${view.tool_id}`);
      }
      // The contract's detail requirement names "tool source/**schema identity**", and `ACC-024` is the
      // rule behind it: an approval binds to the implementation rather than to a name that can be
      // re-pointed. Asserted at the composed surface, because a handler test would pass against a
      // projection whose fields never survived the daemon's own serialization.
      if (view.tool_source_kind !== "connector") {
        problems.push(`tool_source_kind=${view.tool_source_kind}`);
      }
      if (view.tool_source_owner !== "acme.mail") {
        problems.push(`tool_source_owner=${view.tool_source_owner}`);
      }
      if (view.tool_source_version !== "1.0.0") {
        problems.push(`tool_source_version=${view.tool_source_version}`);
      }
      if (view.schema_fingerprint !== SCHEMA_FINGERPRINT) {
        problems.push(`schema_fingerprint=${view.schema_fingerprint}`);
      }
      if (!Array.isArray(view.preview) || view.preview[0]?.value !== "peter@example.com") {
        problems.push(`preview=${JSON.stringify(view.preview)}`);
      }
      if (!Array.isArray(view.allowed_channels) || view.allowed_channels[0] !== "api") {
        problems.push(`allowed_channels=${JSON.stringify(view.allowed_channels)}`);
      }
      if (view.lapsed !== false) {
        problems.push(`lapsed=${view.lapsed}`);
      }
      if (problems.length > 0) {
        fail("the listing must render the seeded approval faithfully", problems.join("; "));
      } else {
        pass("the listing carries the approval, its preview, its channels, and its state");
      }
      // **`has_more` is what tells a client to keep reading**, and `next_cursor` is how it can. A
      // listing that reported `has_more` without a cursor would tell an operator that prompts awaited a
      // decision and give them no way to reach one — the worst of both, because they know work remains
      // and cannot do it.
      //
      // Two rows are seeded and the bound is one, so this page is genuinely bounded and must carry a
      // cursor. Asserting the pair together is the point: `has_more: true` with no cursor is the defect,
      // and neither field alone expresses it.
      const boundedPage = await request(
        record,
        credential,
        "GET",
        "/api/v1/approvals?limit=1",
      );
      if (boundedPage.status !== 200) {
        fail(
          `a bounded listing must answer, got ${boundedPage.status}`,
          boundedPage.text,
        );
      } else if (boundedPage.json?.has_more !== true) {
        fail(
          "a bound of one over two pending rows must report more may remain, got " +
            JSON.stringify(boundedPage.json?.has_more),
          boundedPage.text,
        );
      } else if (typeof boundedPage.json?.next_cursor !== "string") {
        fail(
          "**a bounded page must hand back a cursor, or has_more names work nobody can reach**",
          boundedPage.text,
        );
      } else {
        // The cursor is then used the way a client would, and the next page must be **different** — a
        // cursor that restarted from the beginning would loop forever, re-reading one prompt while the
        // other went undecided.
        const firstId = boundedPage.json.approvals?.[0]?.approval_id;
        const cursor = encodeURIComponent(boundedPage.json.next_cursor);
        const nextPage = await request(
          record,
          credential,
          "GET",
          `/api/v1/approvals?limit=1&cursor=${cursor}`,
        );
        const secondId = nextPage.json?.approvals?.[0]?.approval_id;
        if (nextPage.status !== 200) {
          fail(`a resuming page must answer, got ${nextPage.status}`, nextPage.text);
        } else if (typeof secondId !== "string" || secondId === firstId) {
          fail(
            "the cursor must read a page that does not repeat the first, got " +
              JSON.stringify({ firstId, secondId }),
            `${boundedPage.text}\n${nextPage.text}`,
          );
        } else {
          pass(
            `a bounded page hands back a cursor, and it reads the next page (${firstId} then ${secondId})`,
          );
        }

        // And an unusable cursor is refused by name rather than silently treated as no cursor: a silent
        // restart looks like a legitimate page one, so a client would loop forever without learning its
        // cursor was bad.
        const refused = await request(
          record,
          credential,
          "GET",
          "/api/v1/approvals?cursor=v1.not-a-cursor",
        );
        if (refused.status !== 400 || refused.json?.error?.code !== "request.invalid_cursor") {
          fail(
            `an unusable cursor must be 400 request.invalid_cursor, got ${refused.status}`,
            refused.text,
          );
        } else {
          pass("an unusable cursor is refused with request.invalid_cursor rather than restarting");
        }
      }

      const completePage = await request(record, credential, "GET", "/api/v1/approvals");
      if (completePage.json.has_more !== false) {
        fail(
          `three live rows cannot fill a page of the default bound, so has_more must be false, got ${completePage.json.has_more}`,
          completePage.text,
        );
      } else if (completePage.json.next_cursor !== undefined) {
        fail(
          "a complete page must omit the cursor, or a client follows one into an empty page",
          completePage.text,
        );
      } else {
        pass("a complete page reports has_more:false and carries no cursor");
      }
    }

    // ---------------------------------------------------------------------
    // 2c. **The contract's "expiry is evaluated on every read"**, proved at the composed surface. Two
    //     rows are pending in the store and only one is live, so the assertions below cannot pass by
    //     accident: a surface that ignored the deadline would report two.
    //
    //     Both halves matter and they fail differently. A **detail read** that said `pending` would render
    //     a decision the daemon then refuses as expired — the caller would see a prompt, act on it, and be
    //     told no. A **listing** that still offered the row would put a dead prompt in front of an operator
    //     and spend page budget on it, which is the short-page defect a channel filter caused here once,
    //     arriving by a different route.
    //
    //     The row is expired, not merely hidden: filtering the lapsed rows out would return a *short*
    //     page, and on a surface that serves no cursor a short page is how a client concludes there is
    //     nothing left to decide. So the assertion is on the stored state below, not only on the response.
    // ---------------------------------------------------------------------
    const lapsedDetail = await request(record, credential, "GET", `/api/v1/approvals/${LAPSED_ID}`);
    if (lapsedDetail.status !== 200) {
      fail(`a lapsed detail read must still answer 200, got ${lapsedDetail.status}`, lapsedDetail.text);
    } else if (lapsedDetail.json?.state !== "expired") {
      fail(
        "**a read past the deadline must report the recorded lapse, not `pending`** — a client that saw " +
          "`pending` would render a decision the daemon refuses",
        `state=${lapsedDetail.json?.state} of ${lapsedDetail.text}`,
      );
    } else if (lapsedDetail.json?.lapsed !== true) {
      fail(`an expired approval must report itself lapsed, got ${lapsedDetail.json?.lapsed}`, lapsedDetail.text);
    } else {
      pass("a detail read past the deadline reports the lapse rather than a decidable `pending`");
    }

    const lapsedRow = await readRow(profile, LAPSED_ID);
    if (lapsedRow?.state !== "expired") {
      fail(
        "**the lapse must be recorded, not only hidden from the response** — a `pending` row would " +
          "reappear in every later listing",
        JSON.stringify(lapsedRow),
      );
    } else if ((await countTransitions(profile, LAPSED_ID)) !== 1) {
      fail(
        `the lapse must leave exactly one audit row, got ${await countTransitions(profile, LAPSED_ID)}`,
        JSON.stringify(lapsedRow),
      );
    } else {
      pass("the lapse is durable and audited, so the prompt cannot reappear");
    }

    // ---------------------------------------------------------------------
    // 2b. A page of one is spent on the row this channel may decide. The seeded approval permits
    //     `api`, which is this client's channel, so a bound of one must return it rather than an
    //     empty page.
    // ---------------------------------------------------------------------
    const boundedPage = await request(record, credential, "GET", "/api/v1/approvals?limit=1");
    if (boundedPage.status !== 200) {
      fail(`a bounded page must be accepted, got ${boundedPage.status}`, boundedPage.text);
    } else if (!Array.isArray(boundedPage.json?.approvals) || boundedPage.json.approvals.length !== 1) {
      fail(
        "**a bound of one must be spent on a row this client may decide**, not on an excluded one",
        boundedPage.text,
      );
    } else if (boundedPage.json.approvals[0].approval_id !== APPROVAL_ID) {
      // Ordering is by `expires_at ASC`, so the lapsed row comes *first* in the store's page. Getting the
      // live approval here therefore proves the sweep **re-read** rather than filtering: a filtered page
      // would have come back short and this row would be missing entirely.
      fail(
        "the bounded page must contain the live approval — order is `expires_at ASC`, so the lapsed row " +
          "sorted first and a sweep that filtered rather than re-read would have returned a short page",
        boundedPage.text,
      );
    } else {
      pass("a bounded page is spent on a row the caller may decide, after the lapsed row is swept out");
    }

    // An unknown filter is refused rather than ignored: ignoring one would return a superset of what
    // the caller asked for, and on this listing that means prompts the client believed it excluded.
    const unknownFilter = await request(record, credential, "GET", "/api/v1/approvals?state=pending");
    if (unknownFilter.status !== 400) {
      fail(
        `an unsupported filter must be refused rather than ignored, got ${unknownFilter.status}`,
        unknownFilter.text,
      );
    } else if (!unknownFilter.text.includes("request.invalid")) {
      fail("the refusal must carry the shared code", unknownFilter.text);
    } else {
      pass("an unsupported list filter is refused rather than silently ignored");
    }

    // ---------------------------------------------------------------------
    // 3. A decision applies and records the server-derived actor.
    // ---------------------------------------------------------------------
    const decidePath = `/api/v1/approvals/${APPROVAL_ID}/decide`;
    const readPath = `/api/v1/approvals/${APPROVAL_ID}`;
    const decisionBody = {
      decision: "approve",
      expected_version: 1,
      action_fingerprint: FINGERPRINT,
      // The contract's decision body carries a `comment`, and this is the only path that proves it is
      // **stored and readable** rather than accepted and dropped: the note lives on the decision's
      // transition, so reading it back means the daemon's detail route read the trail.
      comment: DECISION_COMMENT,
    };
    const decided = await request(record, credential, "POST", decidePath, decisionBody, {
      "Idempotency-Key": idempotencyKey("approve"),
    });
    if (decided.status !== 200) {
      fail(`a decision must be accepted, got ${decided.status}`, decided.text);
    } else if (decided.json?.applied !== true) {
      fail("the first decision must report that it was applied", decided.text);
    } else if (decided.json?.approval?.state !== "approved") {
      fail("the approval must be approved", decided.text);
    } else if (decided.json.approval.decided_via !== "api") {
      // The channel is **server-derived**, so this also asserts the request arrived on the API channel
      // rather than on a channel the caller named.
      fail(`the decision must record the server-derived channel, got ${decided.json.approval.decided_via}`, decided.text);
    } else if (
      typeof decided.json.approval.decided_by !== "string" ||
      decided.json.approval.decided_by !== seededRun.principal_id
    ) {
      fail(
        "**the recorded decider must be the principal the server resolves**, not a caller-supplied value",
        `${decided.json.approval.decided_by} vs ${seededRun.principal_id}`,
      );
    } else if (decided.json.approval.decided_assurance !== "standard") {
      // The contract's audit section requires the **assurance** recorded, and like the channel it is the
      // value the server resolved rather than anything the request stated — a caller-supplied assurance
      // would let whoever filled in the body choose how strong the decision looked. Asserted at the
      // composed surface because a handler test would pass against a projection whose field never reached
      // a real client.
      fail(
        `the decision must record the assurance the caller proved, got ${decided.json.approval.decided_assurance}`,
        decided.text,
      );
    } else {
      pass("a decision applies and records the server-derived principal, channel, and assurance");
    }

    // ---------------------------------------------------------------------
    // 3b. The operator's comment is **stored and readable**, not accepted and dropped.
    // ---------------------------------------------------------------------
    const detail = await request(record, credential, "GET", readPath);
    if (detail.status !== 200) {
      fail(`the detail read must succeed, got ${detail.status}`, detail.text);
    } else if (detail.json?.decided_note !== DECISION_COMMENT) {
      fail(
        "**the comment a caller sent must be readable from detail** — the wire declared it stored and nothing stored it",
        `got ${JSON.stringify(detail.json?.decided_note)} of ${JSON.stringify(detail.json)}`,
      );
    } else {
      pass("a decision's comment is stored on its transition and returned by the detail read");
    }

    // ---------------------------------------------------------------------
    // 4. The repeat is idempotent: the contract's "same-key/same-request retry returns the original
    //    decision".
    // ---------------------------------------------------------------------
    const repeat = await request(record, credential, "POST", decidePath, decisionBody, {
      "Idempotency-Key": idempotencyKey("approve-again"),
    });
    if (repeat.status !== 200) {
      fail(`a repeated decision must be idempotent, got ${repeat.status}`, repeat.text);
    } else if (repeat.json?.applied !== false) {
      fail(
        "**a repeat must not claim to have applied a transition that did not happen**",
        repeat.text,
      );
    } else if (repeat.json?.approval?.state !== "approved") {
      fail("the repeat must report the current state", repeat.text);
    } else {
      pass("a repeated decision answers 200 with applied:false and the current state");
    }

    // ---------------------------------------------------------------------
    // 5. The durable record and its audit trail.
    // ---------------------------------------------------------------------
    const row = await readRow(profile, APPROVAL_ID);
    if (row?.state !== "approved" || row.version !== 2) {
      fail("the decision must be durable with the version advanced", JSON.stringify(row));
    } else if (row.decided_via !== "api") {
      fail("the stored decision must record the channel", JSON.stringify(row));
    } else if (row.decided_assurance !== "standard") {
      // The **column**, not only the wire field: the contract's audit requirement is about what the record
      // holds, and a projection could report a value the row never stored.
      fail(
        "the stored decision must record the assurance in its own column",
        JSON.stringify(row),
      );
    } else {
      const transitions = await countTransitions(profile, APPROVAL_ID);
      if (transitions !== 1) {
        fail(`one decision means one audit row, got ${transitions}`);
      } else {
        pass("the decision is durable and its audit trail has exactly one row");
      }
    }

    // ---------------------------------------------------------------------
    // 6. A reviewed action that is not the approved one is refused, and the record is untouched.
    // ---------------------------------------------------------------------
    const mismatched = await request(
      record,
      credential,
      "POST",
      decidePath,
      { decision: "reject", expected_version: 2, action_fingerprint: `sha256:${"cd".repeat(32)}` },
      { "Idempotency-Key": idempotencyKey("mismatch") },
    );
    if (mismatched.status !== 409) {
      fail(`a fingerprint mismatch must be a 409, got ${mismatched.status}`, mismatched.text);
    } else if (!mismatched.text.includes("approval.fingerprint_mismatch")) {
      fail("the refusal must carry the contract's fingerprint code", mismatched.text);
    } else {
      const after = await readRow(profile, APPROVAL_ID);
      if (after?.state !== "approved" || after.version !== 2) {
        fail("a refused decision must not change the record", JSON.stringify(after));
      } else {
        pass("a fingerprint mismatch is refused by code and changes nothing");
      }
    }

    // ---------------------------------------------------------------------
    // 6b. A **critical** action cannot be decided by this ordinary session. The contract states
    //     "critical actions default to step-up in CLI/desktop/mobile" and lists
    //     `approval.assurance_insufficient` among its stable errors — a code that had **no producer**
    //     at all before the step-up rule was enforced, so a client could key on one it never receives.
    //     Asserted at the composed surface because the rule reads the record's own `risk` and the
    //     daemon is the only place the whole path from a seeded row to a wire refusal exists.
    // ---------------------------------------------------------------------
    const criticalDecide = await request(
      record,
      credential,
      "POST",
      `/api/v1/approvals/${CRITICAL_ID}/decide`,
      { decision: "approve", expected_version: 1, action_fingerprint: FINGERPRINT },
      { "Idempotency-Key": idempotencyKey("critical") },
    );
    if (criticalDecide.status !== 403) {
      fail(
        `**a critical action must be forbidden to an ordinary session**, got ${criticalDecide.status}`,
        criticalDecide.text,
      );
    } else if (!criticalDecide.text.includes("approval.assurance_insufficient")) {
      fail(
        "the refusal must carry the contract's own assurance code",
        criticalDecide.text,
      );
    } else if (!criticalDecide.text.includes('"retryable":false')) {
      fail(
        "a stronger assurance is a different request, so re-sending this one cannot succeed",
        criticalDecide.text,
      );
    } else {
      // **Nothing changed, and the refusal is not a silent decision.** A refusal that had already
      // written the decision would be the worst direction: the operator would see "forbidden" while
      // the record said approved. Read back through the client's own route rather than the database,
      // so the assertion is about what a caller can observe.
      const after = await request(record, credential, "GET", `/api/v1/approvals/${CRITICAL_ID}`);
      if (after.status !== 200) {
        fail(`the critical approval must still be readable, got ${after.status}`, after.text);
      } else if (after.json?.state !== "pending" || after.json?.version !== 1) {
        fail(
          "a refused decision must leave the record where it was",
          JSON.stringify(after.json),
        );
      } else {
        pass(
          "a critical action is refused with approval.assurance_insufficient and nothing is written",
        );
      }
    }

    // ---------------------------------------------------------------------
    // 7. The CLI is a client of the same surface. `TLS-013` names API **and CLI**, and the CLI's
    //    commands are what an operator actually types — so they are exercised against the same daemon
    //    rather than assumed to work because the endpoints do.
    // ---------------------------------------------------------------------
    await stop(running);
    running = null;
    running = await startAndWait(daemon, client, profile);

    const showCli = run(client, ["--profile", profile, "approvals", "show", APPROVAL_ID]);
    if (showCli.status !== 0) {
      fail("`approvals show` must succeed", showCli.stderr || showCli.stdout);
    } else if (!showCli.stdout.includes('"state":"approved"')) {
      fail("`approvals show` must print the daemon's own state", showCli.stdout);
    } else {
      pass("`approvals show` prints the approval the daemon holds");
    }

    const listCli = run(client, ["--profile", profile, "approvals", "list"]);
    if (listCli.status !== 0) {
      fail("`approvals list` must succeed", listCli.stderr || listCli.stdout);
    } else if (!listCli.stdout.includes('"max_page"')) {
      fail("`approvals list` must print the daemon's listing", listCli.stdout);
    } else {
      pass("`approvals list` prints the daemon's listing");
    }

    // A decision against an already-decided approval is a state conflict, and the CLI must report the
    // **daemon's own code** rather than a paraphrase — an operator needs the code.
    const rejectCli = run(client, [
      "--profile",
      profile,
      "approvals",
      "reject",
      APPROVAL_ID,
      "--fingerprint",
      FINGERPRINT,
      "--version",
      "2",
    ]);
    if (rejectCli.status === 0) {
      fail("rejecting an approved request must not succeed");
    } else if (!rejectCli.stderr.includes("approval.state_conflict")) {
      fail(
        "**the CLI must print the daemon's own error code**, not a paraphrase",
        rejectCli.stderr || rejectCli.stdout,
      );
    } else {
      pass("a refused CLI decision prints the daemon's own code");
    }

    // ---------------------------------------------------------------------
    // 8. The decision survives a restart: a terminal decision is immutable.
    // ---------------------------------------------------------------------
    await stop(running);
    running = null;
    running = await startAndWait(daemon, client, profile);
    const reloaded = await readRow(profile, APPROVAL_ID);
    if (reloaded?.state !== "approved" || reloaded.version !== 2) {
      fail("a decision must survive a restart unchanged", JSON.stringify(reloaded));
    } else {
      pass("the decision survives a restart unchanged");
    }
  } finally {
    if (running) {
      await stop(running);
    }
    rmSync(profile, { recursive: true, force: true });
  }

  if (failures > 0) {
    console.error(`\n${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("\nall approval surface journey checks passed");
}

await main();
