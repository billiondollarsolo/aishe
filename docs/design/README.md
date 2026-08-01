> **Lifecycle: Active.** Baseline: AIShe v0.6.5. This index is the authoritative
> lifecycle catalog for planning and design records. Update it whenever a design
> document is added or changes lifecycle.

# Design document lifecycle index

AIShe keeps old specifications and qualification reports because they explain
decisions and preserve evidence. They are not all current requirements. Use the
lifecycle label at the top of a document before treating any unchecked item or
future-tense statement as work to perform.

For behavior that exists now, user documentation and source code are
authoritative. For post-0.6.5 product work, use the
[next product, UX, routing, compatibility, and reliability plan](NEXT_PRODUCT_UX_RELIABILITY_PLAN.md).
For security boundaries, use [SECURITY.md](../../SECURITY.md).

## Lifecycle definitions

Each document has exactly one lifecycle:

- **Active** — an authoritative source for work in progress or repository
  governance. Implementation changes that alter its decisions must update it in
  the same change.
- **Implemented** — the described milestone shipped. The document is retained
  as design rationale or delivery evidence, not as an open backlog.
- **Superseded** — a named successor replaced its product or architecture
  decisions. Read it only for historical context.
- **Historical** — a point-in-time review, backlog, or plan that no longer
  governs current work. Unchecked boxes are not automatically requirements.
- **Validation evidence** — results for an identified candidate, commit, and
  environment. The record proves only that candidate and must not be generalized
  to the current checkout without fresh validation.

The baseline identifies the release, commit, or date the document describes.
The current authority/successor column says where to look next.

## `docs/design` inventory

| Document | Lifecycle | Baseline | Current authority or successor |
| --- | --- | --- | --- |
| [README.md](README.md) | Active | v0.6.5 | This lifecycle catalog |
| [NEXT_PRODUCT_UX_RELIABILITY_PLAN.md](NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) | Active | v0.6.5 (`4a2c7e4`) | Implemented and clean deterministic candidate-qualified at `35297d0`; ONB-001 research and external release disposition remain open |
| [NEXT_PRODUCT_UX_RELIABILITY_IMPLEMENTATION_REPORT.md](NEXT_PRODUCT_UX_RELIABILITY_IMPLEMENTATION_REPORT.md) | Validation evidence | v0.6.5 functional candidate (`35297d0`) | Story-by-story and Definition-of-Done implementation/evidence audit; candidate-specific and not a release decision |
| [FISH_INTEGRATION_DECISION.md](FISH_INTEGRATION_DECISION.md) | Active | v0.6.5 (`4a2c7e4`) | No native Fish hook in the current milestone; Fish completions remain distinct |
| [WSL_COMPATIBILITY_DECISION.md](WSL_COMPATIBILITY_DECISION.md) | Active | v0.6.5 (`4a2c7e4`) | No WSL-specific build or support claim until genuine WSL2 qualification |
| [OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md](OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md) | Implemented | v0.5.0 (`b388ee3`) | Backend design record; current operations are in [Managed agent backend](../managed-agent-backend.md) |
| [OPENCODE_BACKEND_VALIDATION.md](OPENCODE_BACKEND_VALIDATION.md) | Validation evidence | v0.5.0 candidate (`b388ee3`) | Candidate-specific qualification; current work requires fresh release gates |
| [AGENT_OUTPUT_DISCLOSURE.md](AGENT_OUTPUT_DISCLOSURE.md) | Implemented | v0.5.2 | Delivered output-density contract; future UX is in the active post-0.6.5 plan |
| [CREDENTIALS_PLAN.md](CREDENTIALS_PLAN.md) | Implemented | v0.4.0 | Current behavior is in [Commands](../commands.md), [Configuration](../configuration.md), and [Providers](../providers.md) |
| [NAMED_CONNECTIONS_PRD.md](NAMED_CONNECTIONS_PRD.md) | Superseded | Pre-v0.6.0 proposal | [Commands — connection vs model](../commands.md#connection-vs-model) |
| [P1_P2_BRAND_SWEEP_PLAN.md](P1_P2_BRAND_SWEEP_PLAN.md) | Implemented | v0.6.3; delivered in v0.6.4 | Future product work is in the active post-0.6.5 plan |
| [UX_MILESTONE_PLAN.md](UX_MILESTONE_PLAN.md) | Implemented | v0.2.30; delivered in v0.3.0 | Future UX work is in the active post-0.6.5 plan |
| [PLAN.md](PLAN.md) | Superseded | June 2026, pre-managed backend | OpenCode design for backend history; active post-0.6.5 plan for remaining work |
| [PRD.md](PRD.md) | Implemented | v0.1 | [Architecture](../architecture.md) for the current system; active post-0.6.5 plan for future work |

## Other planning records

| Document | Lifecycle | Baseline | Current authority or successor |
| --- | --- | --- | --- |
| [AUDIT_FIXES_PLAN.md](../../AUDIT_FIXES_PLAN.md) | Historical | v0.2.24 (`a80c49f`) | [SECURITY.md](../../SECURITY.md) and the active post-0.6.5 plan |
| [READINESS_REVIEW.md](../../READINESS_REVIEW.md) | Historical | v0.2.23 (`62654ef`) | Active post-0.6.5 plan and current CI/release gates |
| [ROADMAP.md](../ROADMAP.md) | Historical | June 2026 | Active post-0.6.5 plan |
| [proposals.md](../proposals.md) | Implemented | v0.2.x feature series | Active post-0.6.5 plan for new requirements |

## Authoring and transition rules

1. Put a visible banner matching `> **Lifecycle: …**` before the title.
2. Include a concrete baseline and a link to the current authority or successor.
3. Add the document to the inventory in the same change.
4. Do not silently turn historical unchecked boxes into a backlog. Copy a
   still-valid requirement into an active plan with fresh reasoning and tests.
5. When work ships, change **Active** to **Implemented** and name the release or
   commit. When decisions are replaced, use **Superseded** and link the successor.
6. Keep validation evidence candidate-specific. Record new evidence in a new
   report instead of rewriting what an older candidate proved.

CI checks every Markdown file directly under `docs/design/` for one of the
recognized lifecycle markers. It intentionally does not infer lifecycle from a
filename or from unchecked task boxes.
