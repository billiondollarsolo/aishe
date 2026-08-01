# Route overrides and prefix lifecycle

AIShe routes locally. Prefixes override that decision for one submitted line;
they never become sticky mode or authority state.

| Prefix | Meaning | Safety and compatibility |
| --- | --- | --- |
| `?` | Force the agent route | Canonical on every maintained front-end. The prefix is stripped before the request is sent. |
| `!` | Force the shell route | Bypasses the AI command safety gate for this line only. AIShe prints a non-color `shell override` cue. |
| `#` | Legacy force-agent alias | Deprecated in the zsh/Rust compatibility paths, native comment syntax in Bash, and planned for removal in AIShe 0.9. Use `?`. |

The `#` transition spans two minor releases: 0.7 introduces an explicit local
migration cue, 0.8 retains the cue and alias, and 0.9 removes the alias. After
removal, leading `#` follows the active shell's comment semantics. There is no
configuration switch because a hidden per-shell grammar toggle would make route
diagnostics and support evidence ambiguous.

Examples:

```sh
? install kubectl please   # agent, even though install is a real command
! printf 'hello\n'         # shell; AIShe safety gate bypassed for this line
# explain this repository  # deprecated in zsh/Rust; a Bash comment
```

Use `aishe route -- LINE` to inspect a decision without executing it or starting
a provider/backend. JSON diagnostics use stable reason codes such as
`forced_agent` and `forced_shell`; the human explanation carries the `#`
migration message.

The route corpus contains both the compatibility alias and the ordinary-comment
collision. Bash qualification requires `#` to remain a comment. A removal
change must first update the versioned corpus, shell predicates, docs, and PTY
fixtures together.

## Local typo assistance

The zsh and Bash command-not-found hooks may show a local spelling cue before
asking a model about an unknown line. The cue is deliberately separate from the
route decision: it never executes the candidate, never changes the selected
route, and never transmits the input merely to decide whether the spelling is
close. Submit the corrected command yourself, or prefix the original request
with `?` when you intended to ask the agent.

The enabled v1 policy uses only commands already visible in the process-local
command cache, permits edit distance one, excludes common prose/question heads,
and requires command-shaped argument evidence for phrases longer than two
words. Presentation is once per unique misspelling in a live shell, after
command-not-found classification and before provider/backend initialization.
The labeled fixture is
`tests/fixtures/routing/typo-assistance-v1.json`; its default-on false-positive
budget is at most 1%, with zero tolerated in the maintained v1 corpus. Changing
the policy, threshold, or corpus schema requires an explicit version review.
