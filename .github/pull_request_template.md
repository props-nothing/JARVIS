## Scope

- TODO ID(s):
- Requirement ID(s):
- Contract/ADR sections:
- Falsifiable hypothesis:
- Cheapest discriminating check:

## Evidence

- [ ] `node scripts/validate-docs.mjs` passes.
- [ ] No external integration or dependency behavior changed; or the exact
      official `llms.txt`/docs/spec/SDK sources were refreshed in an evidence
      note and the manifest is `IMPLEMENTATION_READY`.
- [ ] Every changed integration path passes
      `node scripts/validate-docs.mjs --changed-file <path>`.
- [ ] Focused tests pass before broader checks.
- [ ] Failure, cancellation, timeout, restart, migration, auth, approval,
      idempotency, and redaction cases are covered as applicable.
- [ ] Live tests used only dedicated accounts and are reported accurately; any
      skipped live test names the missing gate.
- [ ] No secret, production payload, recording, private key, or unrelated file
      is included.
- [ ] Contracts, generated artifacts, evidence, operator docs, traceability,
      roadmap, and TODO state are synchronized.

## Validation

List exact commands, target operating systems, results, and skipped lanes.

## Risk and Rollback

State unsupported behavior, residual risk, migration impact, disable/rollback
path, and any owner-controlled release blocker.