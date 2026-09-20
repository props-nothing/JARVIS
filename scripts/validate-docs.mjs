#!/usr/bin/env node

import {
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ALLOWED_STATUSES = new Set([
  "PROPOSED",
  "ACCEPTED",
  "DEPRECATED",
  "SUPERSEDED",
  "UNVERIFIED",
  "BLOCKED",
  "PARTIAL",
  "DONE",
]);
const REQUIRED_FILES = [
  ".github/copilot-instructions.md",
  ".github/pull_request_template.md",
  "AGENTS.md",
  "CLAUDE.md",
  "CONTRIBUTING.md",
  "PRODUCT.md",
  "README.md",
  "ROADMAP.md",
  "SECURITY.md",
  "TODO.md",
  "MASTER_BUILD_PROMPT.md",
  "docs/README.md",
  "docs/adr/0001-rust-control-plane.md",
  "docs/adr/0002-daemon-client-topology.md",
  "docs/adr/0003-dual-storage-profiles.md",
  "docs/adr/0004-canonical-tool-policy-and-mcp.md",
  "docs/adr/0005-isolated-external-runtimes.md",
  "docs/adr/0006-jarvis-owned-memory.md",
  "docs/adr/0007-voice-as-interface.md",
  "docs/adr/0008-native-workflows-before-temporal.md",
  "docs/adr/0009-process-plugins-before-native-abi.md",
  "docs/adr/0010-upstream-evidence-gate.md",
  "docs/adr/README.md",
  "docs/adr/template.md",
  "docs/architecture/README.md",
  "docs/architecture/agent-runtime.md",
  "docs/architecture/api-protocols.md",
  "docs/architecture/connector-platform.md",
  "docs/architecture/domain-boundaries.md",
  "docs/architecture/identity-workspaces.md",
  "docs/architecture/installation-release.md",
  "docs/architecture/memory-context.md",
  "docs/architecture/model-gateway.md",
  "docs/architecture/observability.md",
  "docs/architecture/overview.md",
  "docs/architecture/process-topology.md",
  "docs/architecture/security.md",
  "docs/architecture/storage-data.md",
  "docs/architecture/tool-fabric.md",
  "docs/architecture/voice-telephony.md",
  "docs/architecture/workflows-events.md",
  "docs/contracts/README.md",
  "docs/contracts/approval-contract.md",
  "docs/contracts/common-conventions.md",
  "docs/contracts/connector-manifest.md",
  "docs/contracts/elevenlabs-edge.md",
  "docs/contracts/event-envelope.md",
  "docs/contracts/local-control-api.md",
  "docs/contracts/model-data-policy.md",
  "docs/contracts/model-stream.md",
  "docs/contracts/plugin-manifest.md",
  "docs/contracts/runtime-protocol.md",
  "docs/contracts/tool-contract.md",
  "docs/contracts/voice-call.md",
  "docs/data/README.md",
  "docs/data/migrations.md",
  "docs/data/retention.md",
  "docs/data/schema.md",
  "docs/operations/foundation-handoff.md",
  "docs/operations/README.md",
  "docs/operations/release-readiness.md",
  "docs/planning/README.md",
  "docs/planning/first-vertical-slice.md",
  "docs/planning/risk-register.md",
  "docs/planning/traceability.md",
  "docs/research/README.md",
  "docs/research/dependencies.md",
  "docs/research/integration-evidence-template.md",
  "docs/research/integration-research-policy.md",
  "docs/research/evidence-manifest.json",
  "docs/research/integrations/elevenlabs.md",
  "docs/research/integrations/mcp.md",
  "docs/research/integrations/tauri.md",
  "docs/research/source-registry.md",
  "docs/research/upstream-projects.md",
  "docs/security/README.md",
  "docs/security/threat-model.md",
  "docs/testing/README.md",
  "docs/testing/acceptance.md",
  "docs/testing/strategy.md",
];
const SKIPPED_DIRECTORIES = new Set([".git", "node_modules", "target"]);

function parseArguments() {
  const changedFiles = [];
  let today = new Date().toISOString().slice(0, 10);

  for (let index = 2; index < process.argv.length; index += 1) {
    const argument = process.argv[index];
    if (argument === "--changed-file") {
      index += 1;
      if (index >= process.argv.length) {
        throw new Error("--changed-file requires a repository-relative path");
      }
      changedFiles.push(process.argv[index]);
    } else if (argument === "--today") {
      index += 1;
      if (index >= process.argv.length || !isIsoDate(process.argv[index])) {
        throw new Error("--today requires a valid YYYY-MM-DD date");
      }
      today = process.argv[index];
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }

  return { changedFiles, today };
}

function isIsoDate(value) {
  return (
    typeof value === "string" &&
    /^\d{4}-\d{2}-\d{2}$/.test(value) &&
    !Number.isNaN(Date.parse(`${value}T00:00:00Z`))
  );
}

function read(relativePath) {
  return readFileSync(path.join(ROOT, relativePath), "utf8");
}

function relative(filePath) {
  return path.relative(ROOT, filePath).replaceAll("\\", "/");
}

function walk(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && SKIPPED_DIRECTORIES.has(entry.name)) {
      continue;
    }
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...walk(entryPath));
    } else if (entry.isFile()) {
      files.push(entryPath);
    }
  }
  return files;
}

function metadata(markdown) {
  const result = new Map();
  for (const line of markdown.split(/\r?\n/).slice(0, 20)) {
    const separator = line.indexOf(": ");
    if (separator > 0 && !line.startsWith("#")) {
      result.set(line.slice(0, separator).trim(), line.slice(separator + 2).trim());
    }
  }
  return result;
}

function duplicateValues(values) {
  const counts = new Map();
  for (const value of values) {
    counts.set(value, (counts.get(value) ?? 0) + 1);
  }
  return [...counts.entries()]
    .filter(([, count]) => count > 1)
    .map(([value]) => value)
    .sort();
}

function matches(pattern, text) {
  return [...text.matchAll(pattern)].map((match) => match[1]);
}

export function duplicateProductRequirements(product) {
  return duplicateValues(
    matches(/^- `((?:FR|NFR)-[A-Z]+-\d{3})`:/gm, product),
  );
}

export function globMatches(value, pattern) {
  const normalizedValue = value.toLowerCase();
  const normalizedPattern = pattern.toLowerCase();
  let expression = "^";
  for (let index = 0; index < normalizedPattern.length; index += 1) {
    const character = normalizedPattern[index];
    if (character === "*" && normalizedPattern[index + 1] === "*") {
      expression += ".*";
      index += 1;
    } else if (character === "*") {
      expression += "[^/]*";
    } else if (character === "?") {
      expression += "[^/]";
    } else {
      expression += character.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    }
  }
  return new RegExp(`${expression}$`).test(normalizedValue);
}

export function canonicalChangedFile(value) {
  const slashPath = value.replaceAll("\\", "/");
  if (
    !slashPath ||
    path.isAbsolute(value) ||
    /^[A-Za-z]:\//.test(slashPath) ||
    slashPath.startsWith("//")
  ) {
    return { error: `invalid changed-file path outside repository: ${value}` };
  }

  const resolved = path.resolve(ROOT, slashPath);
  const repositoryPath = path.relative(ROOT, resolved).replaceAll("\\", "/");
  if (
    !repositoryPath ||
    repositoryPath === ".." ||
    repositoryPath.startsWith("../") ||
    path.isAbsolute(repositoryPath)
  ) {
    return { error: `invalid changed-file path outside repository: ${value}` };
  }
  return { path: repositoryPath };
}

export function canonicalEvidencePath(value) {
  if (typeof value !== "string") {
    return { error: "evidence path must be a string" };
  }
  const slashPath = value.replaceAll("\\", "/");
  const result = canonicalChangedFile(slashPath);
  if (
    result.error ||
    result.path !== slashPath ||
    !/^docs\/research\/integrations\/[a-z0-9][a-z0-9-]*\.md$/.test(slashPath)
  ) {
    return { error: `invalid integration evidence path: ${value}` };
  }
  return result;
}

export function sectionForHeading(markdown, heading) {
  const escapedHeading = heading.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const headingMatch = new RegExp(`^## ${escapedHeading}\\r?$`, "m").exec(markdown);
  if (!headingMatch) {
    return null;
  }
  const start = headingMatch.index;
  const marker = headingMatch[0];
  const next = markdown.indexOf("\n## ", start + marker.length);
  return markdown.slice(start, next < 0 ? markdown.length : next);
}

export function parseTodoItems(section) {
  const lines = section.split(/\r?\n/);
  const items = [];
  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(
      /^- \[([ x~])\] `([A-Z]{2,4}-\d{3})`/,
    );
    if (!match) {
      continue;
    }
    let block = lines[index];
    let cursor = index + 1;
    while (cursor < lines.length && !/^- \[[ x~]\] `/.test(lines[cursor])) {
      block += `\n${lines[cursor]}`;
      cursor += 1;
    }
    items.push({ block, id: match[2], state: match[1] });
  }
  return items;
}

export function hasNonemptyEvidence(block) {
  return block.split(/\r?\n/).some((line) => /^\s+Evidence:\s+\S/.test(line));
}

export function traceabilityRowErrors(line, knownTodos, knownAcceptance) {
  const rowErrors = [];
  const cells = line.split("|").slice(1, -1).map((cell) => cell.trim());
  const requirementId = cells[0]?.replaceAll("`", "") ?? "<unknown>";
  if (cells.length !== 6) {
    return [`${requirementId}: traceability row must have exactly 6 columns`];
  }

  const [, ownerCell, contractCell, milestoneCell, todoCell, acceptanceCell] = cells;
  if (!/\[[^\]]+\]\([^)]+\)/.test(ownerCell)) {
    rowErrors.push(`${requirementId}: owner cell must link an owning document`);
  }
  if (
    !/\[[^\]]+\]\([^)]+\)/.test(contractCell) &&
    !contractCell.startsWith("N/A")
  ) {
    rowErrors.push(
      `${requirementId}: contract/schema cell must link a source or state N/A`,
    );
  }
  if (!milestoneCell) {
    rowErrors.push(`${requirementId}: milestone cell is empty`);
  }

  const rowTodos = matches(/`([A-Z]{2,4}-\d{3})`/g, todoCell).filter(
    (value) => !value.startsWith("ACC-"),
  );
  if (rowTodos.length === 0) {
    rowErrors.push(`${requirementId}: row has no implementation TODO`);
  }
  for (const todoId of rowTodos) {
    if (!knownTodos.has(todoId)) {
      rowErrors.push(`${requirementId}: row references unknown TODO ${todoId}`);
    }
  }

  const rowAcceptance = acceptanceCell.match(/ACC-\d{3}/g) ?? [];
  if (rowAcceptance.length === 0) {
    rowErrors.push(`${requirementId}: row has no stable acceptance scenario`);
  }
  for (const acceptanceId of rowAcceptance) {
    if (!knownAcceptance.has(acceptanceId)) {
      rowErrors.push(
        `${requirementId}: row references unknown acceptance ${acceptanceId}`,
      );
    }
  }
  return rowErrors;
}

export function lacksIntegrationSpecificEvidence(
  changedPath,
  matchedIntegrationIds,
  externalPatterns,
) {
  return (
    externalPatterns.some((pattern) => globMatches(changedPath, pattern)) &&
    !matchedIntegrationIds.some(
      (integrationId) => integrationId !== "rust-foundation",
    )
  );
}

export function dependencyEvidenceErrors(changedPaths, integrations) {
  const dependencyErrors = [];
  const manifestChanged = changedPaths.includes(
    "docs/research/evidence-manifest.json",
  );
  const routineLedgerChanged = changedPaths.includes(
    "docs/research/dependencies.md",
  );
  const changedReadyEvidence = integrations.filter(
    (entry) =>
      entry.implementation_ready === true &&
      typeof entry.evidence === "string" &&
      changedPaths.includes(entry.evidence) &&
      manifestChanged,
  );
  const changedReadyFoundation = changedReadyEvidence.some(
    (entry) => entry.id === "rust-foundation",
  );
  const packageManifestPattern =
    /^(?:.*\/)?(?:Cargo\.toml|Cargo\.lock|package\.json|package-lock\.json|pnpm-lock\.yaml|yarn\.lock|pyproject\.toml|poetry\.lock|requirements(?:-[^/]+)?\.txt)$/;

  for (const changedPath of changedPaths) {
    if (changedPath === "rust-toolchain.toml") {
      if (!changedReadyFoundation) {
        dependencyErrors.push(
          `${changedPath}: toolchain change requires changed implementation-ready rust-foundation evidence and manifest`,
        );
      }
    } else if (packageManifestPattern.test(changedPath)) {
      if (!routineLedgerChanged && changedReadyEvidence.length === 0) {
        dependencyErrors.push(
          `${changedPath}: dependency change requires a changed routine dependency ledger or changed implementation-ready evidence note and manifest`,
        );
      }
    }
  }
  return dependencyErrors;
}

function validateRequiredFiles(errors) {
  for (const relativePath of REQUIRED_FILES) {
    if (!existsSync(path.join(ROOT, relativePath))) {
      errors.push(`missing required file: ${relativePath}`);
    }
  }
}

function validateMarkdown(errors) {
  const markdownFiles = walk(ROOT)
    .filter((filePath) => filePath.endsWith(".md"))
    .sort();
  const statusPattern = /^Status:\s*(.+?)\s*$/gm;
  const linkPattern = /\[[^\]]+\]\(([^)]+)\)/g;

  for (const filePath of markdownFiles) {
    const text = readFileSync(filePath, "utf8");
    const relativePath = relative(filePath);

    for (const status of matches(statusPattern, text)) {
      if (!ALLOWED_STATUSES.has(status)) {
        errors.push(`${relativePath}: unsupported Status value ${JSON.stringify(status)}`);
      }
    }

    const lines = text.split(/\r?\n/);
    for (let index = 0; index < lines.length; index += 1) {
      for (const linkMatch of lines[index].matchAll(linkPattern)) {
        const target = linkMatch[1].trim();
        if (/^(https?:\/\/|mailto:|#|<)/.test(target)) {
          continue;
        }
        const encodedPath = target.split("#", 1)[0];
        if (!encodedPath) {
          continue;
        }
        let targetPath;
        try {
          targetPath = decodeURIComponent(encodedPath);
        } catch {
          errors.push(`${relativePath}:${index + 1}: invalid encoded link ${target}`);
          continue;
        }
        const resolved = path.resolve(path.dirname(filePath), targetPath);
        if (!existsSync(resolved)) {
          errors.push(`${relativePath}:${index + 1}: broken local link ${target}`);
        }
      }
    }
  }

  return markdownFiles.length;
}

function validateIdsAndTraceability(errors) {
  const product = read("PRODUCT.md");
  const traceability = read("docs/planning/traceability.md");
  const todo = read("TODO.md");
  const acceptance = read("docs/testing/acceptance.md");

  const requirementDefinitions = matches(
    /^- `((?:FR|NFR)-[A-Z]+-\d{3})`:/gm,
    product,
  );
  for (const duplicate of duplicateProductRequirements(product)) {
    errors.push(`duplicate product requirement: ${duplicate}`);
  }
  const requirements = [...new Set(requirementDefinitions)].sort();
  const rows = matches(
    /^\| `((?:FR|NFR)-[A-Z]+-\d{3})` \|/gm,
    traceability,
  );
  const todoDefinitions = matches(
    /^- \[[ x~]\] `([A-Z]{2,4}-\d{3})`/gm,
    todo,
  );
  const acceptanceHeadings = matches(/^### `(ACC-\d{3})`:/gm, acceptance);

  for (const [label, values] of [
    ["traceability requirement row", rows],
    ["TODO definition", todoDefinitions],
    ["acceptance heading", acceptanceHeadings],
  ]) {
    for (const duplicate of duplicateValues(values)) {
      errors.push(`duplicate ${label}: ${duplicate}`);
    }
  }

  const rowSet = new Set(rows);
  const requirementSet = new Set(requirements);
  const missingRows = requirements.filter((value) => !rowSet.has(value));
  const extraRows = rows.filter((value) => !requirementSet.has(value));
  if (missingRows.length > 0) {
    errors.push(`requirements missing traceability rows: ${missingRows.join(", ")}`);
  }
  if (extraRows.length > 0) {
    errors.push(`unknown traceability requirement rows: ${extraRows.join(", ")}`);
  }

  const knownTodos = new Set(todoDefinitions);
  const traceTodos = new Set(
    matches(/`([A-Z]{2,4}-\d{3})`/g, traceability).filter(
      (value) => !value.startsWith("ACC-"),
    ),
  );
  const unknownTodos = [...traceTodos].filter((value) => !knownTodos.has(value));
  if (unknownTodos.length > 0) {
    errors.push(`traceability references unknown TODO IDs: ${unknownTodos.sort().join(", ")}`);
  }

  const knownAcceptance = new Set(acceptanceHeadings);
  const traceAcceptance = new Set(traceability.match(/ACC-\d{3}/g) ?? []);
  const unknownAcceptance = [...traceAcceptance].filter(
    (value) => !knownAcceptance.has(value),
  );
  if (unknownAcceptance.length > 0) {
    errors.push(
      `traceability references unknown acceptance IDs: ${unknownAcceptance.sort().join(", ")}`,
    );
  }

  for (const line of traceability.split(/\r?\n/)) {
    if (!/^\| `(?:FR|NFR)-[A-Z]+-\d{3}` \|/.test(line)) {
      continue;
    }
    errors.push(...traceabilityRowErrors(line, knownTodos, knownAcceptance));
  }

  const milestoneZero = sectionForHeading(todo, "Milestone 0: Specification");
  if (!milestoneZero) {
    errors.push("TODO is missing the Milestone 0 section");
    return {
      acceptanceCount: acceptanceHeadings.length,
      requirementCount: requirements.length,
      todoCount: todoDefinitions.length,
    };
  }
  const milestoneZeroItems = parseTodoItems(milestoneZero);
  const todoStates = new Map(milestoneZeroItems.map((item) => [item.id, item.state]));
  const expectedDocumentationIds = new Set(
    Array.from({ length: 15 }, (_, index) =>
      `DOC-${String(index + 1).padStart(3, "0")}`,
    ),
  );
  for (const item of milestoneZeroItems) {
    if (!expectedDocumentationIds.has(item.id)) {
      errors.push(`Milestone 0 contains unexpected TODO ${item.id}`);
    }
    if (item.state === "x" && !hasNonemptyEvidence(item.block)) {
      errors.push(`${item.id}: completed TODO has no nonempty Evidence entry`);
    }
  }
  for (let number = 1; number <= 15; number += 1) {
    const todoId = `DOC-${String(number).padStart(3, "0")}`;
    if (todoStates.get(todoId) !== "x") {
      errors.push(`${todoId}: Milestone 0 documentation task is not complete`);
    }
  }

  for (let milestone = 1; milestone <= 10; milestone += 1) {
    const section = todo.match(
      new RegExp(
        `^## Milestone ${milestone}:.*?(?=^## Milestone |^## Explicitly Deferred|$(?![\\s\\S]))`,
        "ms",
      ),
    );
    if (!section) {
      errors.push(`TODO is missing Milestone ${milestone}`);
    } else if (!/^Dependencies:/m.test(section[0])) {
      errors.push(`Milestone ${milestone} has no dependency declaration`);
    }
  }
  const milestoneTwo = sectionForHeading(todo, "Milestone 2: Brain");
  if (!milestoneTwo || !/^Dependencies: Milestone 1 exit gate\./m.test(milestoneTwo)) {
    errors.push("Milestone 2 must depend on the complete Milestone 1 exit gate");
  }

  return {
    acceptanceCount: acceptanceHeadings.length,
    requirementCount: requirements.length,
    todoCount: todoDefinitions.length,
  };
}

function validateEvidence(errors, changedFiles, today) {
  const manifestPath = path.join(ROOT, "docs/research/evidence-manifest.json");
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (error) {
    errors.push(`cannot read evidence manifest: ${error.message}`);
    return 0;
  }

  if (manifest.schema_version !== 1) {
    errors.push("evidence manifest schema_version must be 1");
  }
  if (manifest.policy !== "docs/research/integration-research-policy.md") {
    errors.push("evidence manifest policy path is invalid");
  }

  const expectedStates = new Set([
    "REQUIRED",
    "ARCHITECTURE_ONLY",
    "IMPLEMENTATION_READY",
    "STALE",
    "BLOCKED",
  ]);
  const allowedStates = new Set(manifest.allowed_states ?? []);
  if (
    expectedStates.size !== allowedStates.size ||
    [...expectedStates].some((value) => !allowedStates.has(value))
  ) {
    errors.push("evidence manifest allowed_states does not match policy");
  }

  const integrations = manifest.integrations ?? [];
  for (const duplicate of duplicateValues(integrations.map((entry) => entry.id))) {
    errors.push(`duplicate evidence manifest integration: ${duplicate}`);
  }

  const knownTodos = new Set(
    matches(/^- \[[ x~]\] `([A-Z]{2,4}-\d{3})`/gm, read("TODO.md")),
  );
  const listedEvidence = new Set();
  const normalizedChanges = [];
  for (const value of changedFiles) {
    const result = canonicalChangedFile(value);
    if (result.error) {
      errors.push(result.error);
    } else {
      normalizedChanges.push(result.path);
    }
  }
  const matchedIntegrations = new Map(
    normalizedChanges.map((changedPath) => [changedPath, new Set()]),
  );

  for (const entry of integrations) {
    const integrationId = entry.id ?? "<missing-id>";
    const state = entry.state;
    const ready = entry.implementation_ready;
    const evidencePath = entry.evidence;

    if (!allowedStates.has(state)) {
      errors.push(`${integrationId}: unknown evidence state ${JSON.stringify(state)}`);
    }
    if (typeof ready !== "boolean") {
      errors.push(`${integrationId}: implementation_ready must be boolean`);
    } else if (ready !== (state === "IMPLEMENTATION_READY")) {
      errors.push(`${integrationId}: state and implementation_ready disagree`);
    }

    for (const todoId of entry.todos ?? []) {
      if (!knownTodos.has(todoId)) {
        errors.push(`${integrationId}: unknown TODO ID ${todoId}`);
      }
    }

    const patterns = entry.implementation_paths ?? [];
    if (patterns.length === 0) {
      errors.push(`${integrationId}: no implementation_paths declared`);
    }
    for (const pattern of patterns) {
      if (
        !pattern ||
        path.isAbsolute(pattern) ||
        pattern.replaceAll("\\", "/").split("/").includes("..")
      ) {
        errors.push(`${integrationId}: invalid implementation path pattern ${pattern}`);
      }
    }

    if (evidencePath === null || evidencePath === undefined) {
      if (!new Set(["REQUIRED", "BLOCKED"]).has(state)) {
        errors.push(`${integrationId}: ${state} requires an evidence document`);
      }
    } else {
      const canonicalEvidence = canonicalEvidencePath(evidencePath);
      if (canonicalEvidence.error) {
        errors.push(`${integrationId}: ${canonicalEvidence.error}`);
        continue;
      }
      listedEvidence.add(canonicalEvidence.path);
      const fullPath = path.join(ROOT, canonicalEvidence.path);
      if (!existsSync(fullPath) || !statSync(fullPath).isFile()) {
        errors.push(`${integrationId}: missing evidence file ${evidencePath}`);
      } else {
        const note = readFileSync(fullPath, "utf8");
        const fields = metadata(note);
        if (fields.get("Status") !== "ACCEPTED") {
          errors.push(`${integrationId}: evidence Status must be ACCEPTED`);
        }
        if (!note.includes("llms.txt")) {
          errors.push(`${integrationId}: evidence does not record llms.txt discovery`);
        }
        if (fields.get("Last verified") !== entry.last_verified) {
          errors.push(`${integrationId}: Last verified metadata mismatch`);
        }
        if (fields.get("Revalidate by") !== entry.revalidate_by) {
          errors.push(`${integrationId}: Revalidate by metadata mismatch`);
        }
        if (!isIsoDate(entry.last_verified) || !isIsoDate(entry.revalidate_by)) {
          errors.push(`${integrationId}: invalid evidence dates`);
        } else {
          if (entry.last_verified > entry.revalidate_by) {
            errors.push(`${integrationId}: Last verified is after Revalidate by`);
          }
          if (entry.revalidate_by < today && state !== "STALE") {
            errors.push(`${integrationId}: evidence expired on ${entry.revalidate_by}`);
          }
        }

        const gate = fields.get("Implementation gate");
        if (ready) {
          if (gate !== "PASSED") {
            errors.push(`${integrationId}: ready evidence gate is not PASSED`);
          }
          if ((fields.get("Review scope") ?? "").toLowerCase().includes("architecture only")) {
            errors.push(`${integrationId}: architecture-only note cannot be ready`);
          }
        } else if (gate === "PASSED") {
          errors.push(`${integrationId}: note passes gate but manifest is not ready`);
        }
      }
    }

    for (const changedPath of normalizedChanges) {
      if (patterns.some((pattern) => globMatches(changedPath, pattern))) {
        matchedIntegrations.get(changedPath).add(integrationId);
        if (!ready) {
          errors.push(`${changedPath}: blocked by ${integrationId} evidence state ${state}`);
        }
      }
    }
  }

  const externalPatterns = manifest.external_integration_paths ?? [];
  if (externalPatterns.length === 0) {
    errors.push("evidence manifest has no external_integration_paths");
  }
  for (const changedPath of normalizedChanges) {
    if (lacksIntegrationSpecificEvidence(
      changedPath,
      [...(matchedIntegrations.get(changedPath) ?? [])],
      externalPatterns,
    )) {
      errors.push(
        `${changedPath}: external integration path has no integration-specific evidence entry`,
      );
    }
  }
  errors.push(...dependencyEvidenceErrors(normalizedChanges, integrations));

  const evidenceDirectory = path.join(ROOT, "docs/research/integrations");
  const actualEvidence = new Set(
    readdirSync(evidenceDirectory)
      .filter((name) => name.endsWith(".md"))
      .map((name) => `docs/research/integrations/${name}`),
  );
  const unlisted = [...actualEvidence].filter((value) => !listedEvidence.has(value));
  if (unlisted.length > 0) {
    errors.push(`evidence notes missing from manifest: ${unlisted.sort().join(", ")}`);
  }

  return integrations.length;
}

function main() {
  let argumentsValue;
  try {
    argumentsValue = parseArguments();
  } catch (error) {
    console.error(`ERROR: ${error.message}`);
    return 2;
  }

  const errors = [];
  validateRequiredFiles(errors);
  const markdownCount = validateMarkdown(errors);
  const { acceptanceCount, requirementCount, todoCount } =
    validateIdsAndTraceability(errors);
  const integrationCount = validateEvidence(
    errors,
    argumentsValue.changedFiles,
    argumentsValue.today,
  );

  if (errors.length > 0) {
    for (const error of errors) {
      console.error(`ERROR: ${error}`);
    }
    console.error(`Documentation validation failed with ${errors.length} error(s).`);
    return 1;
  }

  console.log(
    `Validated ${markdownCount} Markdown files, ${requirementCount} requirements, ` +
      `${todoCount} TODO IDs, ${acceptanceCount} acceptance scenarios, and ` +
      `${integrationCount} integration evidence entries.`,
  );
  return 0;
}

const invokedPath = process.argv[1] ? path.resolve(process.argv[1]) : null;
if (invokedPath === fileURLToPath(import.meta.url)) {
  process.exitCode = main();
}