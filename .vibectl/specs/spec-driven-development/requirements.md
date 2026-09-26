# Spec-Driven Development

## Problem

Vibe coding asks a lot of the user in the situations that are hardest:

- **Complex tasks need too much guidance.** The model guesses at scope and
  architecture, then implements the wrong thing thoroughly.
- **Large codebases lose context.** What exists, where, and which parts are
  load-bearing is not in the prompt, so changes land in the wrong layer or
  duplicate something that already exists.
- **Decisions evaporate.** During a long run the model makes a dozen judgement
  calls — which layer owns a concern, which error type to propagate, what to
  name a thing. None of it survives the session, so the team cannot review why
  the code looks the way it does.
- **Misread context is expensive.** A wrong premise discovered 40 tool calls
  later wastes the whole run, not just one step.

## Goal

Add a `/spec` workflow that produces reviewable requirements, design, and
tasks *before* any code is written, and then makes that task list the thing the
agent actually executes.

## Requirements

### Functional

- **FR1 — Three gated phases.** A spec lives in `.vibectl/specs/<slug>/` and is
  built in order: `requirements.md`, then `design.md`, then `tasks.md`. A
  phase cannot be written before the previous one exists.
- **FR2 — Slug from the user's words.** The spec directory name is derived
  from the task text (lowercase, alphanumeric + dashes, ~6 words max) so
  `.vibectl/specs/` stays browsable.
- **FR3 — Executable tasks.** `tasks.md` is a GitHub-style checkbox list.
  vibectl parses it into a task list with per-task status, so progress is
  queryable rather than re-read from prose.
- **FR4 — Pending tasks are injected into the agent's context.** While a spec is
  active, the agent is told what is done and what is next, so it works the list
  instead of inventing its own order.
- **FR5 — Ticking.** A task can be marked done by the agent writing to
  `tasks.md` (it is inside the project root, so the sandbox permits it) or
  explicitly via `/spec done <n>`.
- **FR6 — Phase report.** `/spec` with no active spec lists what exists; `/spec`
  with one shows per-task status and the next phase to run.
- **FR7 — Explicit decisions.** `design.md` must contain a `## Decisions`
  section with at least one decision that states a choice *and* its rationale.
  This is the artifact the team reads months later.
- **FR8 — Specs are committable.** Caches stay gitignored; `specs/` must be
  negotiable so a spec can be reviewed in a pull request.

### Non-functional

- **NFR1 — No new dependencies.** The parser is hand-written `std`; no regex or
  markdown crate.
- **NFR2 — Bounded context.** Injected spec text is capped so a 60-task spec
  cannot blow the context window the agent carefully trims to 80k tokens.
- **NFR3 — No silent data loss.** A malformed `tasks.md` surfaces as a visible
  error, never as an empty task list that reads as "nothing to do".
- **NFR4 — `/plan` keeps working.** It stays the one-shot path for small tasks.
  `/spec` is opt-in and heavier.

## Out of scope

- Multiple concurrent specs.
- Spec templates beyond the three generated files.
- Task dependencies / DAGs. Tasks are an ordered list.
- Syncing specs to an issue tracker.

## Acceptance

- `/spec add <task>` creates the directory and `requirements.md`.
- Running `/spec` in a bare directory explains what to do next instead of
  erroring.
- A hand-written `tasks.md` with mixed `- [ ]`/`- [x]`, nested indentation, and
  a non-checkbox bullet parses to the right statuses.
- The agent's injected context lists pending tasks and omits completed ones once
  the count exceeds the cap.
- `cargo test` covers the parser, slug generation, phase gating, and the cap.
