# CLAUDE.md

@AGENTS.md

The above is the whole contract; everything below is Claude Code specifics.

## Subagents

Give a subagent the ADRs it needs by name, not "read the docs" — `docs/adr/` is
long and an agent that skims it will contradict a decision without noticing.

Tell it explicitly which files not to touch when work is in flight elsewhere.
Parallel agents in this repository have collided on `window.rs` and the native
shims more than once.

## Read mocktail before guessing

`~/Projects/mocktail` is a working implementation of most of what this project
is still working out, in plain source, Apache-2.0. **Consult it whenever you are
about to infer a platform contract** — a JNI descriptor, a JS calling
convention, a call ordering, the shape of a payload. It is not a shortcut around
thinking; it is the same class of evidence as `docs/traces/`, and the one rule
at the top of AGENTS.md applies for the same reason.

This is written down because it was learned expensively on 2026-08-20, twice in
one day:

- `ForceNativeFlagsLoadedForTaskScheduler` was read as proof that mocktail
  memory-patches the engine and is therefore no model for Cordial. It is gated
  to one legacy build and inert on ours. That mistake put an entire section into
  `flag-init.md` and three agent briefs before the gate was read.
- The Join button was guessed at twice — first as a bare WebKit message handler
  nothing calls, then as Android's `addJavascriptInterface` shape. The page
  actually wants `window.__globalRobloxAndroidBridge__.executeRoblox(json)`,
  which is in `src/webview/webview_helper_policy.cc` in plain JavaScript and
  took two minutes to find once somebody looked.

Sober is a second working reference and mocktail a third implementation on the
same Build ID, on this host, not inside Android. Where they disagree with an
inference drawn here, they are usually right.

**What may be taken is the idea, not the transcription.** Adapting a documented
shape and crediting it is right; copying an implementation wholesale is not, and
the same line that governs Roblox's binary governs this — see AGENTS.md. Note
also that mocktail's architecture differs in ways that matter: its web view is a
*separate process* forwarding over a socket, so "how mocktail does it" is not
always transplantable, only informative.

## Reporting back

The rule in AGENTS.md about never stating an unobserved result applies hardest
here, because a summary is where it slips. This project's history includes
claiming "no FastFlag reaches the engine", "~1 fps", and "stays up twelve
seconds" — all wrong, all measured with a broken instrument, all retracted in
later commits.

When a subagent reports success, that is a claim and not a result. Check it.
