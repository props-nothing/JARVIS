import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  canonicalChangedFile,
  canonicalEvidencePath,
  dependencyEvidenceErrors,
  duplicateProductRequirements,
  globMatches,
  hasNonemptyEvidence,
  lacksIntegrationSpecificEvidence,
  parseTodoItems,
  sectionForHeading,
  traceabilityRowErrors,
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