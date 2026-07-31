# AIShe P1 / P2 / brand-sweep plan

| Field | Value |
|-------|--------|
| **Status** | Ready for implementation |
| **Owner** | AIShe maintainers |
| **Baseline** | `v0.6.3` (`6b1c023`) + uncommitted half-block terminal mark on Hetzner/local |
| **Last updated** | 2026-07-31 |
| **Target release** | Prefer one patch **`0.6.4`** (or fold into next minor if larger work ships first) |
| **Source** | Elite-inconsistencies review (post-0.6.2) + follow-up logo work |

## 1. Purpose

Ship the remaining polish from the elite review **without** reopening architecture that already works (connection vs model split, OpenCode OAuth discovery, task-first `/help`, `/dev/tty` pickers).

Three workstreams:

1. **P1 — product consistency** (user-visible polish and correctness edges)
2. **P2 — engineering hygiene** (shared helpers, CI, help surfaces, assets)
3. **Brand sweep** — lock **AIShe** / **AI Shell** / CLI **`aishe`** everywhere operators and models see product language

Success means: same mental model on every surface, fewer tokens for product Q&A in suggest, safer OAuth connection creation, and no accidental “Aishe / AIshe / AISHE” drift in user-facing copy.

## 2. Non-goals

Do **not** do in this plan:

- Re-merge `/connection` and `/model` into a unified picker
- Invent OAuth plan remaining-quota numbers (status `plan` marker stays honest)
- Redesign setup flow, safety gate, or managed OpenCode pin policy
- Mass-edit **historical** `CHANGELOG` entries for past releases
- Rewrite archived design PRDs as if they are still product truth (only mark superseded)
- Change Homebrew Ruby class name `Aishe` (formula class identifier; desc already uses AIShe)
- Change env vars (`AISHE_*`), crate name, binary name, or package name (`aishe`)

## 3. Brand rules (authoritative)

| Layer | Spelling | Examples |
|-------|----------|----------|
| Product name (UI, docs, splash, skill text) | **AIShe** | “AIShe is ready…”, “Inside AIShe:” |
| Expansion | **AI Shell** | README subtitle, ASCII/Unicode mark footer |
| CLI / crate / package / paths | **`aishe`** (lowercase) | `aishe setup`, `~/.config/aishe`, `cargo install aishe` |
| Env / constants | **`AISHE_*`** | `AISHE_CONFIG_DIR` (unchanged) |
| Avoid as product name | `Aishe`, `AIshe`, `AISHE` (all-caps product) | Keep only as CLI/env/all-caps acronyms if unavoidable |

**Possessive:** `AIShe's` (not `Aishe's`).

**SVG / PNG:** User-facing art spelling is **AIShe**. PNG banner is **canonical** for README; SVG is secondary (titles must match; geometry may be regenerated later under P2).

**Terminal mark:** Unicode half-blocks preferred; pure ASCII fallback only for dumb terminals (optional P2).

## 4. Current state (as of 2026-07-31)

### Already shipped (`0.6.3`)

- [x] `/connection` / `/model` “make default?” uses `confirm(..., false)` → **`[y/N]`**
- [x] Compact `/help` overview; no duplicate long slash index
- [x] README H1 **AIShe — AI Shell**; docs index / getting-started brand lock (partial)
- [x] `product_help` overview + skill body use AIShe in key places
- [x] SVG **title/aria** → AIShe
- [x] Tag/release **v0.6.3**

### Uncommitted / partial

- [ ] Half-block glasses `ASCII_LOGO` in `src/promptui.rs` (local + Hetzner install; **not on `main`**)
- [ ] Brand still **`Aishe`** in many user strings (`src/setup.rs`, clap help in `src/main.rs`, `src/context.rs`, `src/capabilities.rs`, several `docs/*.md`)
- [ ] Suggest mode still injects **full** `product_skill_body()` (yolo already prefers brief in system prompts)

### Known code anchors

| Topic | Location |
|-------|----------|
| Suggest product inject | `src/modes/suggest.rs` ~150–156 and ~383 |
| Product brief / skill | `src/product_help.rs` `product_brief()`, `product_skill_body()` |
| OAuth connection ensure | `src/auth.rs` `ensure_oauth_connection_after_login` |
| Brand labels | `src/config.rs` `oauth_connection_label` |
| Default confirm after picker | `src/main.rs` `model_command` / `connection_pick_command` |
| Terminal mark | `src/promptui.rs` `ASCII_LOGO`; PTY wrapper injects via `src/integration.rs` |
| Design PRDs (stale unified `/model`) | `docs/design/NAMED_CONNECTIONS_PRD.md`, related |

---

## 5. Workstream A — P1 product consistency

### A1. Suggest uses brief; full skill only in yolo

**Problem:** On product how-to questions, suggest appends the entire skill markdown to the user message (two call sites). Token-heavy and redundant with the built-in `aishe-product` skill available in yolo.

**Design:**

| Mode | What to inject |
|------|----------------|
| **suggest** (and suggest stream) | `product_brief()` only + short instruction to answer with exact commands |
| **yolo** | Keep `product_brief()` in system / progressive disclosure; full body via `use_skill name=aishe-product` (already the skill registration path) |
| **auto** | Same as suggest if product-question heuristic fires (if any inject exists) |

**Implementation tasks:**

1. In `src/modes/suggest.rs`, replace both `product_skill_body()` injections with `product_brief()`.
2. Update separator copy: `--- AIShe product reference ---` (not “Aishe”).
3. Optionally add one line: “For deeper recipes, user can switch to yolo and use skill aishe-product” — keep short.
4. Unit/integration: assert suggest path does **not** contain long skill headings like `# AIShe product help` when testing inject helper; assert brief markers remain (`/connection`, `aishe auth login`).
5. Manual: ask “how do I add a Codex OAuth account?” in suggest on a fixture/fake provider and confirm answer quality still acceptable with brief only.

**Acceptance:**

- [ ] No `product_skill_body()` call from suggest path
- [ ] `product_brief()` still injected for `looks_like_product_question`
- [ ] Yolo still registers `aishe-product` skill with full body
- [ ] Existing product_help unit tests still pass; add assert on brief length ≪ skill body length if useful

**Tests:**

```text
cargo test --lib product_
# plus any modes/suggest unit tests if present
# optional: admin_validation product Q if model-gated (document manual if not)
```

**Risk:** Suggest answers slightly less complete for rare recipes. Mitigation: brief already lists exact commands; user can open `/help accounts` or yolo skill.

**Effort:** S · **Risk:** Low

---

### A2. OAuth post-login connection edge cases

**Problem:** `ensure_oauth_connection_after_login` is correct for the happy path but:

1. If **multiple** connections already match provider+profile, only `existing.first()` is mentioned (map iteration order is not user-stable).
2. Label refresh only rewrites if label is exactly `"OpenAI"`, `"xAI"`, or equals the id — not other stale labels.
3. Special-casing may drift from `oauth_connection_label` as the single source of brand labels.

**Design:**

1. **List all matches**, not only first:
   - If exactly one match → offer use (current behavior).
   - If multiple → print all ids/labels; offer picker or “use N” style confirm; **do not** create another connection unless none match.
2. **Label refresh:** if connection auth is OAuth for this provider/profile, set `label = oauth_connection_label(...)` when:
   - label is empty, equals id, or is in a small set of legacy generics (`OpenAI`, `xAI`, `OpenAI · …` without Codex brand), **or**
   - optional flag `--refresh-label` later; for this plan: refresh when label does **not** already equal the canonical oauth label (always sync to canonical on login).  
   **Preferred elite rule:** on successful login for a matching connection, **always** set label to `oauth_connection_label` (authoritative). Document that custom labels on OAuth rows are overwritten on login (or: only overwrite if `label` matches previous auto brand pattern — choose **always sync** for simplicity unless product wants custom labels; if custom labels matter, only overwrite legacy generics).

   **Decision for this plan:**  
   - **Always set** `label = oauth_connection_label(provider, profile)` on login for **matching** OAuth connections (canonical brands).  
   - Custom marketing names on OAuth connections are not preserved across re-login (acceptable for alpha; note in CHANGELOG).

3. Keep create path using `unique_oauth_connection_id` + canonical label (already does).

**Implementation tasks:**

1. Refactor `ensure_oauth_connection_after_login` in `src/auth.rs`:
   - Collect all matching ids (sorted for stable UX).
   - Branch: 0 → create; 1 → refresh label + offer use; N → list + offer use of each or interactive select.
2. Extract pure helpers for unit tests:
   - `fn matching_oauth_connection_ids(config, provider, profile) -> Vec<String>`
   - `fn apply_oauth_label(connection, provider, profile)`
3. Tests in `src/auth.rs` or `src/config.rs` with temp config dirs:
   - no connection → creates one with Codex/Grok label
   - one connection → no second create; label forced canonical
   - two connections same profile → no create; both listed; no panic
4. PTY/manual on Hetzner: `aishe auth login openai --profile work` twice; second time does not duplicate; label shows `Codex - OAuth · work`.

**Acceptance:**

- [ ] No duplicate connection on re-login
- [ ] Multi-match path never silently ignores all but first without printing them
- [ ] Labels after login match `oauth_connection_label`
- [ ] Unit tests cover 0/1/N match cases

**Risk:** Overwriting a user-edited OAuth label. Mitigate with CHANGELOG note; optional follow-up: only overwrite if previous label was auto-derived.

**Effort:** M · **Risk:** Med

---

### A3. Mark design docs as superseded (unified `/model`)

**Problem:** `docs/design/NAMED_CONNECTIONS_PRD.md` and related still describe unified `/model` as the normal experience. Agents and humans reading design/ can reintroduce regressions.

**Design:**

1. Add a banner at the top of each superseded design doc:

   ```markdown
   > **Superseded (product truth as of 0.6.x):** account switching is `/connection`;
   > `/model` lists models for the **active** connection only. See
   > [Commands — connection vs model](../commands.md#connection-vs-model) and
   > the root README. This PRD remains historical context.
   ```

2. Do **not** rewrite entire PRDs.
3. Cross-link from `docs/design/PLAN.md` or design index if one exists.

**Files (minimum):**

- `docs/design/NAMED_CONNECTIONS_PRD.md`
- Any other design doc that still says “unified `/model` picker” as current UX (grep and banner)

**Acceptance:**

- [ ] Grep for “unified” + `/model` in `docs/design/` either historical-only or banner-present
- [ ] User docs (`docs/commands.md`, README) remain the authority (no conflicting “current” claims in design without banner)

**Effort:** S · **Risk:** Low

---

## 6. Workstream B — P2 engineering hygiene

### B1. Shared “Enter → optional default?” helper

**Problem:** Connection and model post-picker confirm logic is duplicated in `main.rs`. Future defaults could diverge again.

**Design:**

```rust
/// After a shell-local picker choice, optionally promote to durable default.
/// Returns true if caller should persist as default.
fn confirm_promote_to_default(prompt: &str) -> bool {
    matches!(
        aishe::promptui::confirm(prompt, false), // always [y/N]
        Ok(Some(true))
    )
}
```

Or slightly richer:

```rust
fn maybe_promote_default(
    differs_from_durable: bool,
    prompt: impl AsRef<str>,
    already_save_default: bool,
) -> bool
```

**Tasks:**

1. Extract helper next to picker commands in `main.rs` (or `promptui` if reused elsewhere).
2. Call from both connection and model paths.
3. Unit test helper with mock is hard (IO); prefer pure “differs + already_save” logic test + keep PTY coverage in `tests/model_picker_pty.py`.

**Acceptance:**

- [ ] Single call site for default `false` confirm policy
- [ ] PTY: bare Enter on confirm keeps shell-local; `y` promotes

**Effort:** S · **Risk:** Low

---

### B2. Banner PNG regression guard

**Problem:** Logo can silently regress via bad commit.

**Design (minimal):**

1. Store expected SHA-256 of `assets/aishe-banner.png` in a small checked-in file, e.g. `assets/SHA256SUMS` or `assets/aishe-banner.png.sha256`, **or** a test that embeds the hash.
2. CI / unit test:

   ```rust
   // tests or build script
   assert_eq!(sha256(assets/aishe-banner.png), EXPECTED);
   ```

3. When intentionally changing the banner: update hash in the same commit.

**Acceptance:**

- [ ] PR that only replaces PNG without hash update fails CI
- [ ] Document update procedure in `docs/development.md` one-liner

**Effort:** S · **Risk:** Low

---

### B3. Long CLI help / man page mentions connection vs model

**Problem:** Operators using only `aishe --help` / man may miss the split and AIShe branding.

**Design:**

1. In clap top-level `about` / `long_about` (or epilog): one short paragraph:

   - Product: AIShe (AI Shell); CLI: `aishe`
   - Interactive: `/connection` switches account; `/model` models on active account
   - See `aishe help` / in-shell `/help`

2. Ensure `aishe man` output includes the same epilog if generated from clap.

**Tasks:**

1. Find clap `Command` builder in `src/main.rs`.
2. Add epilog; run `aishe --help` and `aishe man | head`.
3. CLI test: `assert_cmd` stdout contains `/connection` and `AIShe` or `AI Shell`.

**Acceptance:**

- [ ] `aishe --help` mentions `/connection` and `/model` roles once
- [ ] Brand product name appears once in long help

**Effort:** S · **Risk:** Low

---

### B4. Asset source-of-truth (SVG vs PNG)

**Problem:** PNG banner is canonical; SVGs may still be older geometry even if titles say AIShe.

**Options (pick one in implementation):**

| Option | Action |
|--------|--------|
| **B4a (recommended)** | Document in `assets/README.md` (new, short): PNG canonical for GitHub/README; SVG optional; regenerate only when art changes |
| **B4b** | Regenerate `aishe-logo.svg` / `aishe-banner.svg` / `aishe-icon.svg` from the approved PNG/source and commit |

**Acceptance:**

- [ ] `assets/README.md` states canonical asset + brand spelling
- [ ] If B4b: visual check README still uses PNG; SVGs open and show AIShe wordmark

**Effort:** S (doc) or M (regen) · **Risk:** Low

---

### B5. Optional — dumb-terminal ASCII fallback

**Problem:** Half-block mark may render poorly on `TERM=dumb` or ancient fonts.

**Design:**

```rust
pub fn terminal_mark() -> &'static str {
    if std::env::var_os("TERM").as_deref() == Some(OsStr::new("dumb"))
        || std::env::var_os("AISHE_ASCII_LOGO").is_some()
    {
        ASCII_LOGO_FALLBACK
    } else {
        ASCII_LOGO // half-block
    }
}
```

Fallback: minimal pure ASCII glasses sketch (no face) or just:

```text
AIShe
AI Shell
```

**Acceptance:**

- [ ] `TERM=dumb aishe setup` does not print mojibake-heavy blocks if fallback enabled
- [ ] Default modern terminals still show half-blocks

**Effort:** S · **Risk:** Low · **Priority within P2:** lowest (do last)

---

## 7. Workstream C — Brand sweep

### C1. Scope matrix

| Scope | Action |
|-------|--------|
| `src/**/*.rs` user-visible strings, clap `about`/`help`, errors shown to users | `Aishe` → `AIShe`, `Aishe's` → `AIShe's` |
| `src/product_help.rs`, `src/promptui.rs` | Already mostly done; re-verify |
| `docs/*.md` (user guide, not design archive) | Product name → AIShe |
| `docs/design/*` | Banner for superseded UX; optional light renames in titles only |
| `README.md`, `CHANGELOG.md` **new** sections | AIShe |
| `packaging/aishe.rb` **desc/caveats** | AIShe (class name stays `Aishe`) |
| `tests/**` that assert user-visible product strings | Expect AIShe where product name is shown |
| Code comments / internal logs | Prefer AIShe for consistency when they say the product name |
| Identifiers (`struct`, env, paths) | **Do not** rename |

### C2. High-hit files (start here)

From current tree grep (non-exhaustive):

- `src/setup.rs` — many wizard strings (“your Aishe environment”, “Inside Aishe:”, …)
- `src/main.rs` — clap help text, uninstall/reset/resume messages
- `src/context.rs` — history blurb
- `src/capabilities.rs` — doctor/check messages
- `docs/installation.md`, `docs/development.md`, `docs/shell-integration.md`, `docs/modes.md`, …
- Tests asserting `"must run inside an active Aishe shell"` etc.

### C3. Procedure (safe sweep)

1. **Inventory:**  
   `rg -n '\bAishe\b|\bAIshe\b|\bAISHE\b' --glob '!target/**' --glob '!**/CHANGELOG.md'`  
   Classify each hit: product name / CLI / env / historical changelog / false positive.
2. **Mechanical replace only product-name hits** (not `aishe` command, not `AISHE_`).
3. **Avoid** replacing inside:
   - `CHANGELOG.md` historical version sections (optional: leave 0.6.2 text as written)
   - Third-party notices that quote upstream
   - Formula class `class Aishe`
4. **Build + tests** after Rust string changes (many are in clap and print paths).
5. **Spot-check:** `aishe setup --help`, `aishe --help`, first screen of setup, `/help`, doctor strings.

### C4. Commit half-block mark with brand sweep or just before

Uncommitted today:

- `src/promptui.rs` — half-block `ASCII_LOGO`
- `src/integration.rs` — test expects `█` not `| o   o |`

Ship these as part of **0.6.4** (or a small docs/brand PR). Redeploy Hetzner from tagged release after.

**Acceptance:**

- [ ] `rg '\bAishe\b' src/` returns only intentional exceptions (document any)
- [ ] User docs primary pages use AIShe
- [ ] Half-block mark on `main` and in release binary
- [ ] Hetzner can reinstall from release or rsync post-merge

**Effort:** M · **Risk:** Low (copy-only) with careful CLI/env exclusions

---

## 8. Suggested implementation order

Do in this order to minimize thrash and keep bisect-friendly commits:

| Step | Work | Commit message sketch |
|------|------|------------------------|
| 1 | Half-block logo + test fix | `ui: half-block AIShe glasses mark in terminal` |
| 2 | Brand sweep (src + user docs) | `docs/ui: lock product spelling to AIShe` |
| 3 | A1 suggest brief inject | `perf: inject product brief (not full skill) in suggest` |
| 4 | A2 OAuth multi-match + label sync | `fix: oauth login connection matching and labels` |
| 5 | A3 design superseded banners | `docs: mark unified /model design docs superseded` |
| 6 | B1 shared promote-default helper | `refactor: share post-picker default confirm` |
| 7 | B3 clap/man epilog | `docs: mention /connection vs /model in --help` |
| 8 | B2 banner hash + B4 assets README | `ci: pin aishe-banner.png checksum` |
| 9 | B5 dumb TERM fallback (optional) | `ui: ASCII logo fallback for TERM=dumb` |
| 10 | CHANGELOG + version **0.6.4** + tag | `release: prepare v0.6.4` |

Steps 1–5 are the **minimum** product-facing package. Steps 6–9 can ship same release or next.

## 9. Testing strategy

### Automated

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --lib
cargo test --test cli
# PTY (if zsh available):
python3 tests/model_picker_pty.py
# Optional broader:
python3 tests/pty_scenarios.py   # /help, brand strings
```

**New / extended tests (required for plan completion):**

| Area | Test |
|------|------|
| A1 | Suggest inject uses brief: unit test on helper or snapshot of built user message if extractable |
| A2 | Unit tests 0/1/N OAuth connection matches + label sync |
| B1 | Covered by existing model_picker_pty Enter/`[y/N]` behavior |
| B2 | Hash test for banner PNG |
| B3 | CLI `--help` contains connection/model hint |
| C | Grep-based CI optional: fail if `\bAishe\b` in `src/` except allowlist file |

### Manual (Hetzner or local)

1. Fresh `aishe` shell: first-run hint shows half-block glasses + AIShe + AI Shell.
2. `aishe setup` splash uses same mark and AIShe copy.
3. `/connection` Enter on non-default → `[y/N]`; bare Enter stays shell-local.
4. `/model` same.
5. Suggest: “how do I switch accounts in AIShe?” → useful answer without multi-KB skill dump (check token usage if easy).
6. `aishe auth login openai --profile work` twice → no duplicate connection; label Codex-branded.
7. `aishe --help` readable for connection vs model.

### Regression watchlist

- OAuth create still works when **no** connection exists
- `d` in picker still saves default without confirm
- Built-in skill `aishe-product` still listed in `aishe skills` / yolo

## 10. Rollout

1. Land steps 1–5 on `main` via PR or direct (repo convention).
2. Deploy to Hetzner for soak (`rsync` + `cargo build --release` or install release artifact).
3. Cut **v0.6.4** when automated + manual checklist green.
4. Confirm GitHub Release assets; update packaging formula version when release workflow fills SHAs.

**Hetzner note:** Current box may already run an uncommitted half-block build; after merge, reinstall from tag so version string is not `unknown`.

## 11. Definition of done

This plan is **complete** only when:

- [ ] All **P1** acceptance boxes (A1–A3) checked with evidence (test names or manual notes)
- [ ] **Brand sweep** C acceptance checked; allowlist documented if any `Aishe` remains in `src/`
- [ ] Half-block mark is on `main` and in a published release **or** explicitly deferred with issue link
- [ ] At least **B1** and **B3** done, or explicitly deferred in CHANGELOG Unreleased
- [ ] CHANGELOG documents user-visible behavior changes
- [ ] No intentional invent of plan quota metrics

## 12. Traceability back to review

| Review item | Plan section |
|-------------|--------------|
| Suggest full skill inject | A1 |
| OAuth multi-match / label refresh | A2 |
| Design PRDs unified `/model` | A3 |
| Shared Enter→default helper | B1 |
| Banner CI hash | B2 |
| man / `--help` connection vs model | B3 |
| SVG vs PNG source of truth | B4 |
| (Logo quality / Unicode mark) | C4 + optional B5 |
| Brand lock beyond 0.6.3 partial | C |
| Release 0.6.3 | Done — next is 0.6.4 |

## 13. Open decisions (resolve during implementation)

1. **OAuth custom labels:** always overwrite on login (this plan default) vs preserve custom labels?
2. **B4a vs B4b:** document only vs regenerate SVGs?
3. **B5:** ship dumb-TERM fallback in 0.6.4 or defer?
4. **Release train:** single 0.6.4 for all of the above, or brand+logo micro-patch first?

**Recommended defaults:** (1) always overwrite OAuth labels on login, (2) B4a docs only, (3) defer B5, (4) one **0.6.4** with steps 1–7 minimum.

---

## 14. Appendix — grep cheat sheet

```bash
# Product-name leftovers (review hits carefully)
rg -n '\bAishe\b|\bAIshe\b' src docs packaging README.md \
  --glob '!docs/design/**' --glob '!**/CHANGELOG.md'

# Stale design language
rg -n 'unified.*/model|/model.*connection.*picker' docs/design

# Suggest inject
rg -n 'product_skill_body|product_brief' src/modes

# OAuth ensure
rg -n 'ensure_oauth_connection_after_login' src
```

## 15. Appendix — out of scope reminders (already elite)

Do not “fix” these as part of this plan:

- Codex / Grok OAuth setup paths and brands
- Connection-scoped `/model` + OpenCode `/config/providers` discovery
- Task-first `/help` + `aishe-product` skill registration
- Unbuffered `/dev/tty` filter pickers
- Safety gate / workspace scope / managed runtime pin
- Clippy `-D warnings` baseline
