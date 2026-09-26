# Design — Spec-Driven Development

## Overview

A spec is a directory, not a blob. Three markdown files, written in order, plus
a small Rust module that reads and validates them.

```
.vibectl/specs/<slug>/
├── requirements.md   # what to build, and what is out of scope
├── design.md         # how, and why — the decision log
└── tasks.md          # the executable checklist
```

`src/agent/spec.rs` owns reading, writing, parsing, and slugging. The TUI
(`src/tui/mod.rs`) owns the command surface. `src/agent/mod.rs` only injects
context.

## Data model

```rust
pub struct Spec {
    pub slug: String,
    pub dir: PathBuf,
    pub requirements: Option<String>,
    pub design: Option<String>,
    pub tasks: Vec<TaskItem>,
}

pub struct TaskItem {
    pub number: usize,   // 1-based, stable across edits within a session
    pub done: bool,
    pub text: String,
    pub depth: usize,    // nesting, for display only
}
```

`tasks` is empty when `tasks.md` is absent, and that is the *only* case. A
present-but-unparseable `tasks.md` is an error, because "0 tasks" and "I could
not read your checklist" must not look the same.

## Decisions

- **Tasks are a GitHub-style checkbox list, not YAML or JSON.**
  Markdown checkboxes are what a human already knows how to edit by hand, and
  they survive copy-paste out of an issue tracker. The cost is a hand-written
  parser, which is ~40 lines and needs no dependency. Rejected: a
  `tasks.yaml`, because a human editing YAML during a code review is a worse
  experience than ticking a box.

- **`number` is a 1-based position, not a stable ID.**
  Tasks get inserted and removed mid-spec. A stable ID would need a persistent
  counter in the file; a position is what `/spec done 3` can be interpreted
  against today, and it is obvious to the user because they can see the list.
  The cost — renumbering after an insert — is acceptable because the command
  takes the number from the displayed list.

- **Only the `## Decisions` section is validated in `design.md`.**
  Enforcing headings everywhere would make the LLM's output brittle to
  reformat. Rationale capture is the one thing that silently degrades to
  nothing if it is not required, so it is the one thing that is required.

- **The parser skips non-checkbox list items instead of failing.**
  `tasks.md` will contain prose, headings, and notes. Only lines that look like
  a checkbox become tasks. A file with zero checkboxes is the error case
  (NFR3), not a file with prose plus checkboxes.

- **Injection is "pending only", capped, and always states the counts.**
  Showing completed tasks trains the model to re-read work it already did and
  to narrate progress instead of implementing. The cap drops from the *bottom*
  (deepest remaining tasks) so the next action is never the thing that got
  truncated.

- **Specs live under `.vibectl/` but are un-ignored via `!.vibectl/specs/`.**
  The blanket `.vibectl/` ignore exists to keep LMDB caches and JSONL session
  logs out of git. Specs are the opposite: they are the artifact meant to be
  reviewed. A negation rule carves out exactly the specs directory and leaves
  the caches ignored.

- **`/spec` is a new command; `/plan` is untouched.**
  `/plan` is one LLM call producing a single readable plan. That is the right
  cost for a small task. Repurposing `/plan` would either slow it down or make
  its output change under users who already rely on it. `/spec` was previously
  a stub that printed `PLAN.md`; that stub is replaced, since a command named
  `/spec` that shows a plan is worse than no command.

- **The agent may tick its own boxes.**
  `tasks.md` is inside the project root, so `write_file` passes the sandbox
  without a special case. Self-ticking keeps the checklist honest without
  asking the user to track progress. `/spec done <n>` remains for the case
  where the model forgets.

## Data flow

```
/spec add <task>
  └─ slug(text) ──────────────► .vibectl/specs/<slug>/requirements.md
                                 (LLM generates; gate: requirements exists)

/spec design
  └─ requires requirements.md ─► design.md  (must contain ## Decisions)

/spec tasks
  └─ requires design.md ──────► tasks.md

/spec                        ─► status: per-phase, per-task
/spec done 3                 ─► rewrite tasks.md with task 3 ticked

any turn while a spec exists ─► system message: counts + pending tasks (capped)
```

## Failure modes

| Situation | Behaviour |
|---|---|
| `tasks.md` has prose but no checkboxes | error naming the file, spec not treated as executable |
| `design.md` written before `requirements.md` | refuse, name the phase to run first |
| slug is empty after sanitising | fall back to `spec`, never an empty dir name |
| slug already exists | append `-2`, `-3`; do not clobber a spec in flight |
| more tasks than the injection cap | keep the first N pending, say how many were hidden |
| no spec at all | `/spec` prints how to start one |
