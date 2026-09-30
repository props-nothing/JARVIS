import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  canonicalChangedFile,
  canonicalEvidencePath,
  dependencyEvidenceErrors,
  duplicateProductRequirements,
  encodingErrors,
  globMatches,
  hasNonemptyEvidence,
  lacksIntegrationSpecificEvidence,
  parseTodoItems,
  sectionForHeading,
  traceabilityRowErrors,
  unownedAcceptanceScenarios,
} from "./validate-docs.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const VALIDATOR = path.join(ROOT, "scripts", "validate-docs.mjs");

function runValidator(changedFile) {
  return spawnSync(
    process.execPath,
    [VALIDATOR, "--changed-file", changedFile],
    { cwd: ROOT, encoding: "utf8" },
  );
}

/**
 * Reads the validated-Markdown count out of a validator run.
 *
 * The exclusion test asserts the count is unchanged, not merely that the run
 * passed, because a scratch file that was *counted* while still passing would
 * mean the exclusion only suppressed errors rather than removing the file from
 * the gate's world.
 */
function validatedFileCount(result) {
  const match = /Validated (\d+) Markdown files/.exec(
    `${result.stdout}${result.stderr}`,
  );
  assert.ok(match, `no validated file count in output: ${result.stdout}${result.stderr}`);
  return Number(match[1]);
}

test("canonicalChangedFile collapses repeated dot segments", () => {
  assert.deepEqual(canonicalChangedFile("././adapters/example/client.rs"), {
    path: "adapters/example/client.rs",
  });
});

test("canonicalChangedFile rejects traversal and absolute Windows paths", () => {
  assert.match(canonicalChangedFile("../Cargo.toml").error, /outside repository/);
  assert.match(canonicalChangedFile("C:/outside/Cargo.toml").error, /outside repository/);
});

test("globMatches is fail-closed for case variants", () => {
  assert.equal(
    globMatches(
      "CrAtEs/core/src/Providers/new/client.rs",
      "crates/**/src/providers/**",
    ),
    true,
  );
});

test("Milestone 0 parser requires nonempty evidence", () => {
  const markdown = [
    "## Milestone 0: Specification",
    "",
    "- [x] `DOC-001` Complete.",
    "  Evidence:",
    "- [x] `DOC-002` Complete.",
    "  Evidence: verified artifact.",
    "",
    "## Milestone 1: Foundation",
  ].join("\n");
  const section = sectionForHeading(markdown, "Milestone 0: Specification");
  const items = parseTodoItems(section);
  assert.equal(items.length, 2);
  assert.equal(hasNonemptyEvidence(items[0].block), false);
  assert.equal(hasNonemptyEvidence(items[1].block), true);
  assert.equal(
    sectionForHeading(
      markdown.replace(
        "## Milestone 0: Specification",
        "## Milestone 0: Specification Extra",
      ),
      "Milestone 0: Specification",
    ),
    null,
  );
});

test("duplicate requirement definitions fail closed", () => {
  const product = [
    "- `FR-ID-001`: First definition.",
    "- `FR-ID-001`: Duplicate definition.",
  ].join("\n");
  assert.deepEqual(duplicateProductRequirements(product), ["FR-ID-001"]);
});

test("an acceptance scenario named by no requirement is reported", () => {
  // `ACC-007` was documented in `docs/testing/acceptance.md` and named by **no** traceability row, so
  // nothing owned it. The validator checked only the other direction — that a row names an ACC which
  // exists — so an unreferenced heading was invisible, and a scenario nothing schedules reads as
  // covered because it is written down.
  const headings = ["ACC-004", "ACC-007", "ACC-074"];
  const table =
    "| `NFR-COMP-001` | [Owner](o.md) | [Contract](c.md) | All | `FND-004` | `ACC-004`, `ACC-074` |";
  assert.deepEqual(unownedAcceptanceScenarios(table, headings), ["ACC-007"]);

  // Every heading owned: nothing reported, so the check cannot be satisfied by always reporting.
  const complete =
    "| `NFR-COMP-001` | [Owner](o.md) | [Contract](c.md) | All | `FND-004` | `ACC-004`, `ACC-007`, `ACC-074` |";
  assert.deepEqual(unownedAcceptanceScenarios(complete, headings), []);
});

test("a scenario covered through a range is not reported as unowned", () => {
  // **This is the false positive that made the check unusable until it was fixed.** `NFR-SEC-001`
  // covers `ACC-070` through `ACC-073`, naming only the endpoints literally — so a bare `ACC-\d{3}`
  // scan reported `ACC-071` and `ACC-072` as unowned while they are covered, and a reader following
  // the message would have "fixed" a row that was already correct.
  const headings = ["ACC-070", "ACC-071", "ACC-072", "ACC-073", "ACC-074"];
  const ranged =
    "| `NFR-SEC-001` | [Owner](o.md) | [Contract](c.md) | All | `FND-007` | `ACC-070` through `ACC-073` |";
  assert.deepEqual(unownedAcceptanceScenarios(ranged, headings), ["ACC-074"]);

  // The `to` spelling and a second range form both expand, because the table uses whichever reads
  // better in a cell and a check that understood only one would silently miss the other.
  const toForm =
    "| `NFR-X-001` | [Owner](o.md) | [Contract](c.md) | All | `FND-007` | `ACC-070` to `ACC-074` |";
  assert.deepEqual(unownedAcceptanceScenarios(toForm, headings), []);
});

test("traceability rows reject unknown and missing IDs", () => {
  const knownTodos = new Set(["BRN-001"]);
  const knownAcceptance = new Set(["ACC-010"]);
  const unknown =
    "| `FR-RUN-001` | [Owner](owner.md) | [Contract](contract.md) | 2 | `BRN-999` | `ACC-999` |";
  assert.deepEqual(traceabilityRowErrors(unknown, knownTodos, knownAcceptance), [
    "FR-RUN-001: row references unknown TODO BRN-999",
    "FR-RUN-001: row references unknown acceptance ACC-999",
  ]);

  const missing =
    "| `FR-RUN-001` | [Owner](owner.md) | [Contract](contract.md) | 2 | none | none |";
  assert.deepEqual(traceabilityRowErrors(missing, knownTodos, knownAcceptance), [
    "FR-RUN-001: row has no implementation TODO",
    "FR-RUN-001: row has no stable acceptance scenario",
  ]);
});

test("repeated dot segments cannot bypass unmapped integration gate", () => {
  const result = runValidator("././integrations/unregistered/client.rs");
  assert.equal(result.status, 1);
  assert.match(
    `${result.stdout}\n${result.stderr}`,
    /external integration path has no integration-specific evidence entry/,
  );
});

test("case variants inside crate provider namespaces need specific evidence", () => {
  const result = runValidator("CrAtEs/core/src/Providers/unregistered/client.rs");
  assert.equal(result.status, 1);
  assert.match(
    `${result.stdout}\n${result.stderr}`,
    /external integration path has no integration-specific evidence entry/,
  );
});

test("repository traversal is rejected before evidence matching", () => {
  const result = runValidator("../Cargo.toml");
  assert.equal(result.status, 1);
  assert.match(
    `${result.stdout}\n${result.stderr}`,
    /invalid changed-file path outside repository/,
  );
});

test("Foundation readiness cannot authorize an unknown external runtime", () => {
  assert.equal(
    lacksIntegrationSpecificEvidence(
      "crates/runtime/src/external_runtimes/unknown/client.rs",
      ["rust-foundation"],
      ["crates/**/src/external_runtimes/**"],
    ),
    true,
  );
  assert.equal(
    lacksIntegrationSpecificEvidence(
      "crates/runtime/src/external_runtimes/openclaw/client.rs",
      ["rust-foundation", "openclaw-runtime"],
      ["crates/**/src/external_runtimes/**"],
    ),
    false,
  );
});

test("underscore external runtime namespaces need specific evidence", () => {
  const result = runValidator(
    "crates/jarvis-runtime/src/external_runtimes/unregistered/client.rs",
  );
  assert.equal(result.status, 1);
  assert.match(
    `${result.stdout}\n${result.stderr}`,
    /external integration path has no integration-specific evidence entry/,
  );
});

test("integration evidence paths cannot escape or use another directory", () => {
  assert.deepEqual(canonicalEvidencePath("docs/research/integrations/mcp.md"), {
    path: "docs/research/integrations/mcp.md",
  });
  assert.match(
    canonicalEvidencePath("docs/research/integrations/../../AGENTS.md").error,
    /invalid integration evidence path/,
  );
  assert.match(
    canonicalEvidencePath("docs/research/dependencies.md").error,
    /invalid integration evidence path/,
  );
});

test("Foundation readiness is not blanket dependency approval", () => {
  const foundation = {
    id: "rust-foundation",
    implementation_ready: true,
    evidence: "docs/research/integrations/rust-foundation.md",
  };
  assert.equal(
    dependencyEvidenceErrors(["Cargo.toml"], [foundation]).length,
    1,
  );
  assert.deepEqual(
    dependencyEvidenceErrors(
      ["Cargo.toml", "docs/research/dependencies.md"],
      [foundation],
    ),
    [],
  );
  assert.equal(
    dependencyEvidenceErrors(
      ["rust-toolchain.toml", "docs/research/dependencies.md"],
      [foundation],
    ).length,
    1,
  );
  assert.deepEqual(
    dependencyEvidenceErrors(
      [
        "rust-toolchain.toml",
        "docs/research/integrations/rust-foundation.md",
        "docs/research/evidence-manifest.json",
      ],
      [foundation],
    ),
    [],
  );
});

test("an unlinked document is reported by the index check", () => {
  // The top-level index drifted twice in this repository: a document was added,
  // linked from its own section index, and never linked from `docs/README.md`.
  // A document nothing links to is effectively unreviewed, so this must fail
  // closed. The fixture is created and removed around the run so the repository
  // is never left dirty by a failing test.
  const fixture = path.join(ROOT, "docs", "testing", "temp-index-probe.md");
  const contents = "# Temporary index probe\n\nStatus: PROPOSED\n";
  writeFileSync(fixture, contents, "utf8");
  try {
    const result = runValidator("docs/testing/strategy.md");
    assert.equal(result.status, 1, "an unlinked document must fail the gate");
    assert.match(
      `${result.stdout}${result.stderr}`,
      /temp-index-probe\.md exists but the index does not link it/,
    );
  } finally {
    rmSync(fixture, { force: true });
  }
});

test("the index check ignores section indexes and templates", () => {
  // `README.md` and `template.md` are index and scaffolding files, not documents
  // a reader is expected to reach from the top level. Excluding them is what
  // keeps the check from failing on the repository's own structure.
  const result = runValidator("docs/testing/strategy.md");
  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`);
});

test("the excluded scratch area is invisible to the validator", () => {
  // `docs/research-telephony/` is excluded from the repository, so its Markdown
  // must not participate in any check: a scratch file with an unsupported
  // `Status:` value or a broken link must not fail the gate, and must not change
  // the validated file count either. If the exclusion entry in
  // `EXCLUDED_DIRECTORIES` is ever removed, this test starts failing.
  //
  // The excluded directory is **created by this test when absent** rather than
  // required to exist. It is an ignored scratch area that may legitimately be
  // deleted once its runnable harness is no longer needed, and a test that
  // depended on it being present turned deleting that folder into a red build.
  // The test owns exactly what it creates and removes it again.
  const excluded = path.join(ROOT, "docs", "research-telephony");
  const probe = path.join(excluded, "temp-validator-probe.md");
  const contents = [
    "# Scratch probe - not part of the repository",
    "",
    "Status: NOT_A_VALID_STATUS",
    "",
    "[a link into nothing at all](does/not/exist.md)",
    "",
  ].join("\n");

  const createdDirectory = !existsSync(excluded);
  if (createdDirectory) {
    mkdirSync(excluded, { recursive: true });
  }
  const countBefore = validatedFileCount(runValidator("docs/testing/strategy.md"));

  writeFileSync(probe, contents, "utf8");
  try {
    const result = runValidator("docs/testing/strategy.md");
    assert.equal(
      result.status,
      0,
      `an excluded scratch file must not affect the gate: ${result.stdout}${result.stderr}`,
    );
    assert.doesNotMatch(
      `${result.stdout}${result.stderr}`,
      /temp-validator-probe/,
      "the excluded file must not be mentioned at all",
    );
    assert.equal(
      validatedFileCount(result),
      countBefore,
      "an excluded file must not change the validated file count",
    );
  } finally {
    rmSync(probe, { force: true });
    // Only the directory this test created is removed, and only once it is empty
    // again, so a real scratch area is never destroyed by a test run.
    if (createdDirectory && readdirSync(excluded).length === 0) {
      rmSync(excluded, { force: true, recursive: true });
    }
  }
});

test("code-page round-trip damage in a document fails closed", () => {
  // A document whose em dash was read with a legacy code page and written back as UTF-8 still
  // compiles, still links, and still parses — which is why the same damage lived in a checked-in
  // `.rs` file for several rounds while every gate reported success. Only the bytes reveal it, so
  // the check has to look at the content rather than the file encoding: the damaged text IS
  // valid UTF-8.
  assert.deepEqual(encodingErrors("docs/x.md", "a plain ascii document\n"), []);
  assert.deepEqual(
    encodingErrors("docs/x.md", "an em dash \u2014 reads fine\n"),
    [],
  );
  // The dash the repository actually uses in prose is not damage.
  assert.equal(encodingErrors("docs/x.md", "caf\u00e9 \u2014 \u65e5\u672c\u8a9e\n").length, 0);

  const damaged = "a client shares \u00e2\u20ac\u201d a type\n";
  const errors = encodingErrors("docs/x.md", damaged);
  assert.equal(errors.length, 1, errors.join("\n"));
  assert.match(errors[0], /docs\/x\.md:1: text contains a code-page round-trip artefact/);

  // The line number is the artefact's own line, not the file's first line.
  const laterLine = "first\nsecond\nthird \u00e2\u20ac\u201d here\n";
  assert.match(encodingErrors("docs/x.md", laterLine)[0], /docs\/x\.md:3:/);

  // A byte-order mark is the same class: a signature that reached the content.
  assert.match(
    encodingErrors("docs/x.md", "\ufeff# Title\n")[0],
    /starts with a UTF-8 byte-order mark/,
  );

  // And the validator itself refuses a real file with the damage, so the check is wired into
  // the gate rather than only being callable.
  const fixture = path.join(ROOT, "docs", "testing", "temp-mojibake-probe.md");
  writeFileSync(fixture, "# Probe \u00e2\u20ac\u201d damaged\n\nStatus: PROPOSED\n", "utf8");
  try {
    const result = runValidator("docs/testing/strategy.md");
    assert.equal(result.status, 1, "a damaged document must fail the gate");
    assert.match(
      `${result.stdout}${result.stderr}`,
      /temp-mojibake-probe\.md:1: text contains a code-page round-trip artefact/,
    );
  } finally {
    rmSync(fixture, { force: true });
  }

  // The instance this check exists for was in a **Rust source file**, not a document, and it
  // survived because damaged source still compiles. A source fixture therefore has to be
  // rejected too, or the guard would miss the case that motivated it.
  const sourceFixture = path.join(ROOT, "crates", "jarvis-protocol", "src", "temp-mojibake-probe.rs");
  writeFileSync(sourceFixture, "//! a comment \u00e2\u20ac\u201d damaged\n", "utf8");
  try {
    const result = runValidator("docs/testing/strategy.md");
    assert.equal(result.status, 1, "a damaged source file must fail the gate");
    assert.match(
      `${result.stdout}${result.stderr}`,
      /temp-mojibake-probe\.rs:1: text contains a code-page round-trip artefact/,
    );
  } finally {
    rmSync(sourceFixture, { force: true });
  }
});

test("a damaged file in the scratch area cannot fail the gate or change the count", () => {
  // `.scratch/` is gitignored (`.gitignore` line 13), so its contents are not part of the
  // repository and must be invisible here exactly as `target/` and `node_modules/` are. This
  // matters most for the encoding check, whose extension list already reaches `.rs` and `.mjs`:
  // a developer debugging that check leaves a scratch copy of the corrupted file behind, and that
  // copy must not turn the debugging into a red build.
  //
  // The count assertion is the stronger half. Before `.scratch` was added to the skipped set, a
  // stale scratch document was being **counted** as a validated Markdown file — so the gate was
  // reading a file that is not in the repository at all, and its count did not describe the repo.
  const scratch = path.join(ROOT, ".scratch");
  const probe = path.join(scratch, "temp-validator-probe.rs");
  const createdDirectory = !existsSync(scratch);
  if (createdDirectory) {
    mkdirSync(scratch, { recursive: true });
  }
  const countBefore = validatedFileCount(runValidator("docs/testing/strategy.md"));

  writeFileSync(probe, "//! damaged \u00e2\u20ac\u201d probe\n", "utf8");
  try {
    const result = runValidator("docs/testing/strategy.md");
    assert.equal(
      result.status,
      0,
      `a scratch file must not affect the gate: ${result.stdout}${result.stderr}`,
    );
    assert.doesNotMatch(
      `${result.stdout}${result.stderr}`,
      /temp-validator-probe/,
      "the scratch file must not be mentioned at all",
    );
    assert.equal(
      validatedFileCount(result),
      countBefore,
      "a scratch file must not change the validated file count",
    );
  } finally {
    rmSync(probe, { force: true });
    if (createdDirectory && readdirSync(scratch).length === 0) {
      rmSync(scratch, { force: true, recursive: true });
    }
  }
});