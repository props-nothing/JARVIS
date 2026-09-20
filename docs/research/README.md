# Research and External Evidence

Status: ACCEPTED

External behavior is never implemented from model memory, repository examples,
or an unpinned `latest` page. The required workflow is:

1. Find the TODO in [the evidence manifest](evidence-manifest.json).
2. Fetch the provider's current official root and section `llms.txt` indexes.
3. Read the exact official spec/schema, auth, security, limits, errors,
   changelog, migration guidance, SDK source, examples, and relevant tests.
4. Create or refresh a full note from
   [the evidence template](integration-evidence-template.md), or use the
   [dependency ledger](dependencies.md) only when the strict routine-library
   criteria apply.
5. Set an exact supported version, unsupported behavior, kill switch, and a
   falsifiable contract/live test plan.
6. Change the manifest entry to `IMPLEMENTATION_READY` only after the note says
   `Implementation gate: PASSED` and is within its revalidation date.
7. Run one `node scripts/validate-docs.mjs` invocation with repeated
   `--changed-file <path>` arguments for all implementation, package, lock, and
   evidence changes before the first adapter/dependency edit and before finish.

## Documents

- [Integration research policy](integration-research-policy.md)
- [Integration evidence template](integration-evidence-template.md)
- [Evidence readiness manifest](evidence-manifest.json)
- [Routine dependency evidence](dependencies.md)
- [Official source registry](source-registry.md)
- [Upstream project architecture study](upstream-projects.md)
- [MCP evidence](integrations/mcp.md)
- [ElevenLabs evidence](integrations/elevenlabs.md)
- [Tauri evidence](integrations/tauri.md)

## Manifest States

| State | Meaning | May integration code start? |
| --- | --- | --- |
| `REQUIRED` | Planned boundary has no complete evidence note | No |
| `ARCHITECTURE_ONLY` | Sources support design direction, but exact version or contract proof is missing | No |
| `IMPLEMENTATION_READY` | Exact target and falsifiable checks satisfy the implementation gate | Yes, until revalidation |
| `STALE` | Revalidation date passed or upstream behavior contradicted the note | No |
| `BLOCKED` | Official evidence conflicts or a required account/resource is unavailable | No |

The manifest is an index and gate, not evidence itself. An entry cannot become
ready by changing JSON alone.
