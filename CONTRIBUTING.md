# Contributing to JARVIS

JARVIS is currently in specification bootstrap. Contributions should reduce
ambiguity, prove a vertical slice, or improve safety and operability. Large
feature dumps without contract and test evidence are not ready for review.

## Required Reading

1. [AGENTS.md](AGENTS.md)
2. [PRODUCT.md](PRODUCT.md)
3. [ROADMAP.md](ROADMAP.md)
4. [TODO.md](TODO.md)
5. [docs/README.md](docs/README.md)

Run the documentation and evidence gate before selecting work:

```bash
node scripts/validate-docs.mjs
```

## Choose Work

- Select a stable TODO ID whose dependencies are complete.
- Keep one pull request focused on one behavior or tightly coupled vertical slice.
- Link requirement IDs, contract sections, ADRs, and acceptance scenarios.
- For external systems, create or refresh the integration evidence note first.
- Confirm the integration is `IMPLEMENTATION_READY` in the evidence manifest and
  pass every changed implementation/package/evidence path together using repeated
  `node scripts/validate-docs.mjs --changed-file <path>` arguments.
- Record every dependency version/license in the full evidence note or the
  routine dependency ledger before changing a package manifest.
- Open an ADR before changing a non-negotiable boundary.

## Change Workflow

1. Inspect the current implementation and nearby tests.
2. State a falsifiable hypothesis and the cheapest validating check.
3. Add or update the contract before incompatible implementation changes.
4. Make the smallest coherent edit.
5. Run the narrow test immediately.
6. Add failure, cancellation, restart, auth, and redaction tests as appropriate.
7. Run all checks required by the touched subsystem.
8. Update TODO status and documentation with exact evidence.

## Commit and Pull Request Shape

- Keep generated output separate or clearly marked.
- Do not mix unrelated formatting or dependency churn.
- Never commit secrets, real customer payloads, recordings, access tokens, or
  unredacted diagnostics.
- Do not claim live integration coverage when only mocks ran.
- Describe skipped tests and the credentials/environment they require.
- Include migration and rollback notes for persisted or public contract changes.

## Planned Validation Baseline

Once the workspace exists:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps --all-features
cargo test --workspace
```

The `cargo doc` step exists because the library crates deny `rustdoc::broken_intra_doc_links`,
which only a rustdoc invocation evaluates. See `AGENTS.md`'s Rust quality bar.

Additional checks will cover:

- schema/code generation drift;
- SQLite and PostgreSQL migrations;
- MCP conformance and Inspector scenarios;
- installer and upgrade paths on native operating systems;
- frontend lint/type/test/build and Playwright flows;
- dependency audit, license policy, SBOM, and provenance;
- opt-in provider contract and live tests.

Documentation/evidence changes always run:

```bash
node --test scripts/validate-docs.test.mjs
node scripts/validate-docs.mjs
```

## Documentation Changes

- Use ISO dates.
- Keep local links valid.
- Use Mermaid for maintainable diagrams.
- Do not edit accepted ADRs except typo/link fixes; supersede them.
- Label future design as `PROPOSED` and observed behavior with evidence labels.
- Do not duplicate volatile upstream API details across multiple local files.

## License Notice

The project license is not yet selected. Until it is, do not import or adapt
third-party source code. Architectural study and links are acceptable; copied
implementation is not.