# vibectl

An autonomous terminal coding agent. Chat with an AI that reads, writes, and searches your codebase, runs shell commands with approval, inspects git state, and discovers your project's own rules before acting.

```
╭──────────────────────────────────────────────────────────────────╮
│  ❯  Ask vibectl…▋                              ⊘   [/]   [↵]  │
╰──────────────────────────────────────────────────────────────────╯
  /help  ·  Ctrl+K  ·  ↑↓  history
```

## Features

- **Modern chat TUI** — compact input box, chat bubbles with timestamps, syntax-aware code blocks, animated cursor, dark theme with proper contrast
- **Evidence-based agent** — discovers project rules, steering files, and conventions before modifying anything; tags claims as `[CONFIRMED]`, `[LIKELY]`, or `[UNKNOWN]`
- **Multi-provider LLM**
  - OpenAI (`gpt-*`, `OPENAI_API_KEY`)
  - Anthropic (`claude-*`, `ANTHROPIC_API_KEY`)
  - Ollama or any OpenAI-compatible endpoint
- **Extended steering discovery** — automatically finds and loads:
  - `AGENTS.md`, `AGENT.md`, `CLAUDE.md`, `GEMINI.md`, `.cursorrules`
  - `.vibectl/steering/*.md`, `.kiro/steering/*.md`, `steering/*.md`
  - `docs/ARCHITECTURE.md`, `docs/CONTRIBUTING.md`
- **`vibectl audit`** — scan repo, detect languages, inventory features, flag security gaps, list unknowns
- **Plan mode** — generate and save a step-by-step plan to `.vibectl/PLAN.md`
- **Headless / CI mode** — non-interactive, pipeable, scriptable
- **Tools**: `read_file`, `write_file`, `patch_file`, `create_dir`, `glob`, `grep`, `list_symbols`, `git`, `shell_exec`, `run_command`, `run_tests`, `web_fetch`
- **Spec-driven development** — `/spec` writes requirements, design, and an executable task list before code
- **AST symbol index** — `list_symbols` parses Rust/Python/JS/TS/Go with tree-sitter instead of grepping
- **Path sandbox** — every file access is confined to the project root; `..` traversal and symlink escapes are refused
- **Retry with backoff** — 429/5xx are retried with exponential backoff, honouring `Retry-After`; 4xx fails fast
- **Shell approval** — all shell commands and file writes pause for `[y]/[n]` confirmation
- **Interrupt safety** — Ctrl+C or Esc cancels a running agent; stale in-flight events are discarded

## Install

Requires Rust 1.85+ (edition 2024).

```sh
cargo build --release
# binary at target/release/vibectl
```

## Usage

```sh
# Interactive TUI (default)
vibectl

# Run in a specific directory
vibectl -c /path/to/project

# Pick a model
vibectl -m gpt-4o
vibectl -m claude-sonnet-4-20250514
vibectl -m llama3.2          # Ollama

# Audit the repository
vibectl audit

# Audit + immediately scaffold steering files
vibectl audit --init-steering

# Generate skeleton .vibectl/steering/ files
vibectl --init-steering

# Headless (non-interactive)
vibectl --headless "explain this codebase"
vibectl --headless --plan "add pagination to the users endpoint"
vibectl --headless --dangerous-yes "run tests and fix all failures"  # CI

# Pipe a prompt
echo "what does session.rs do?" | vibectl --headless

# Roll back the last agent run (pops its git-stash checkpoint)
vibectl undo
```

## Configuration

Global config: `~/.config/vibectl/config.yaml`
Project config: `.vibectl/config.yaml` (merged over global)

```yaml
# Defaults shown. Everything is optional.
model: llama3.2          # prefix decides the provider: claude-* / gpt-*
temperature: 0.2
max_tokens: 4096
system_prompt: ""        # replaces the built-in agent prompt when set
allow_any_path: false    # lift the project-root sandbox (see below)

# Provider credentials. Env vars are used as a fallback:
#   OPENAI_API_KEY, ANTHROPIC_API_KEY, OLLAMA_URL
openai_api_key: ""
anthropic_api_key: ""
ollama_base_url: http://localhost:11434

# Any OpenAI-compatible endpoint
custom_providers:
  - name: internal
    base_url: https://llm.internal/v1
    api_key: ""
    models: [house-model, house-model-small]
```

Provider resolution order: model prefix (`claude-*` → Anthropic, `gpt-*` → OpenAI) → matching `custom_providers` entry → Ollama fallback. Resolution is case-insensitive, so `GPT-4o` and `gpt-4o` route identically.

A project config (`.vibectl/config.yaml`) is merged **over** the global one. Only the keys it actually mentions are overridden — a project file that sets just `temperature` leaves your global model alone. Note the one-way latch: a project config can widen `allow_any_path` but never re-lock it.

### The path sandbox

All filesystem tools are confined to the detected project root. `read_file` included — a read-only tool is not a safe tool, and pulling `~/.ssh/id_rsa` into the prompt leaks it without ever raising an approval prompt.

Traversal (`../../etc/passwd`) and symlink escapes (a directory inside the project linking to `/etc`) are both refused. Lift it with `allow_any_path: true`, or per-run with `--dangerous-yes` in headless mode.

`shell_exec`, `run_command` and `run_tests` are **not** filtered by the sandbox — they are gated only by the approval prompt, so `--dangerous-yes` gives the agent unrestricted process execution. Treat it accordingly.

Override model via env: `VIBECTL_MODEL=gpt-4o-mini vibectl`

## Specs — deciding before coding

Vibe coding asks the most guidance exactly where it is hardest: complex tasks,
and work on top of a large codebase. It also loses the reasoning — a long run
makes a dozen judgement calls and none of them survive the session.

`/spec` writes the reasoning down *before* any code exists, then makes that
document the thing the agent executes.

```
/spec add <task>     →  .vibectl/specs/<slug>/requirements.md
/spec design         →  .vibectl/specs/<slug>/design.md
/spec tasks          →  .vibectl/specs/<slug>/tasks.md
```

The phases are gated: `design` refuses to run before `requirements.md` exists,
and `tasks` before `design.md`. A spec that skips the reasoning is the problem
this is meant to prevent, so the ordering is enforced rather than suggested.

**The decision log is the point.** `design.md` must carry a `## Decisions`
section, one bullet per choice stating what was chosen and what was rejected.
That is the section a teammate reads months later to understand why the code
looks the way it does.

**Tasks are executable, not prose.** `tasks.md` is a checkbox list, parsed into
per-task state. While a spec is active, the agent is told what is done and what
is next — pending tasks only, capped so a long checklist cannot crowd out the
conversation — and it works the list in order instead of inventing its own
sequence. Tick a box by editing the file, or with `/spec done <n>`.

```
/spec
  ✓ add-docker-support  requirements  ✓ design  ✓ tasks  (2/5 done, 3 pending)

    ✓ 1. Add the path sandbox guard
    ✓ 2. Route shell tools through one approval gate
    · 3. Detect docker-compose.yml
    · 4. Emit a Compose plan in the status output
    … 1 more task(s)

  next: [3] Detect docker-compose.yml
```

Specs are meant to be reviewed, so unlike the rest of `.vibectl/` they are
**not** gitignored: `.gitignore` ignores `.vibectl/*` and then re-includes
`.vibectl/specs/`. Session state and caches stay local; the documents a teammate
needs land in the pull request.

`/plan` is unchanged and still the right tool for a small task — one LLM call,
one readable plan, no ceremony.

## Steering — Teaching vibectl about your project

Steering files inject project knowledge into every agent request. vibectl auto-discovers them on startup.

### Quick scaffold

```sh
vibectl --init-steering
```

Creates `.vibectl/steering/` with starter files:

```
.vibectl/steering/
├── product.md            # what the project does, goals
├── architecture.md       # module boundaries, key decisions
├── coding-conventions.md # style, naming, error handling
├── security.md           # secret handling, auth rules
├── cli-ux.md             # output format, interaction style
└── testing.md            # test strategy, coverage
```

Fill these in. vibectl reads them every run and uses them to:
- Choose the right abstractions
- Follow project conventions without being told
- Know which things are `[CONFIRMED]` vs `[UNKNOWN]`

### Manual steering

Append a rule without editing files:

```sh
# From TUI
/steer Always run cargo fmt after editing Rust files.

# Appends to .vibectl/steer.md
```

### Discovery priority

```
GLOBAL (built-in defaults)
  ↓
REPOSITORY  (AGENTS.md, CLAUDE.md, .vibectl/steer.md)
  ↓
DOMAIN      (.vibectl/steering/security.md, architecture.md, …)
  ↓
DIRECTORY   (src/auth/AGENTS.md, …)
  ↓
CURRENT TASK
```

## Audit

```sh
vibectl audit
```

Scans the repository and prints a structured report:

```
  vibectl Audit Report
  ════════════════════════════════════════════════

  Project
  ────────────────────────────────────────────────
  ✓  Project root detected    (.git)
  ✓  Language detected        (Rust, edition 2024)
  ✓  Build manifest           Cargo.toml

  Agent Instructions
  ────────────────────────────────────────────────
  ✗  AGENTS.md
  ✓  .vibectl/steer.md        ↳ Project Rules

  Steering Files
  ────────────────────────────────────────────────
  ✓  .vibectl/steering/architecture.md
  ✗  .vibectl/steering/security.md

  Feature Analysis
  ────────────────────────────────────────────────
  ✓  DONE     Automated tests         test suite detected
  ✗  MISSING  CI pipeline             no pipeline definition found
  ✗  MISSING  Lint / format config    clippy/rustfmt/eslint/prettier/biome
  ✓  DONE     Dependency locking      Cargo.lock / package-lock.json / go.sum
  ✗  MISSING  Environment template    .env.example documents required config
  ✗  MISSING  License                 LICENSE file at repo root

  Unknown / Needs Verification
  ────────────────────────────────────────────────
  ?  Security requirements — no security.md found
  ?  Deployment target — no CI/CD config found
```

## TUI — Keys & Commands

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Shift+Enter` | Insert newline (multiline) |
| `Ctrl+Enter` | Send multiline message |
| `Ctrl+C` (busy) | Cancel running agent |
| `Ctrl+C` twice (idle) | Exit vibectl |
| `Esc` (busy) | Cancel running agent |
| `Esc` (multiline) | Collapse to single line |
| `Esc` (text) | Clear input |
| `↑` / `↓` | Input history |
| `Ctrl+K` or `?` | Toggle help |
| `PgUp` / `PgDn` | Scroll conversation |
| `Scroll wheel` | Scroll conversation |
| `Ctrl+L` | Jump to latest message |
| `Ctrl+D` | Quit |

| Command | Action |
|---------|--------|
| `/model <name>` | Switch model |
| `/plan <task>` | Generate implementation plan (one-shot) |
| `/spec` | Spec status: phases, per-task state, next task |
| `/spec add <task>` | Start a spec: writes `requirements.md` |
| `/spec design` | Write `design.md`, including the decision log |
| `/spec tasks` | Break the design into an executable checklist |
| `/spec done <n>` | Tick task `n` |
| `/spec list` | List every spec with progress |
| `/undo` | Rollback last agent file changes (git stash pop) |
| `/steer <rule>` | Append rule to `.vibectl/steer.md` |
| `/new` | Reset conversation history |
| `/cfg` | Print effective config |
| `/provider` | Show provider + model |
| `/clear` | Clear message view |
| `/help` `/?` | Toggle help |
| `/quit` `/exit` | Exit vibectl |

There is no session persistence: `/new` clears the in-memory conversation, and closing the terminal discards it. History recall (`↑`/`↓`) covers prompts typed in this session only.

## Tools

| Tool | Description | Approval |
|------|-------------|----------|
| `read_file` | Read file with offset/limit | no |
| `glob` | Find files by pattern | no |
| `grep` | Search file contents by regex | no |
| `list_symbols` | AST symbol index (Rust, Python, JS, TS, Go) | no |
| `git` | status / diff / log in the project | no |
| `web_fetch` | Fetch and extract content from a URL | no |
| `write_file` | Create or overwrite a file | yes |
| `patch_file` | Apply a unified diff hunk | yes |
| `create_dir` | Create a directory (for scaffolding) | yes |
| `shell_exec` | Run any shell command | yes |
| `run_command` | Run a command, returns exit code / stdout / stderr / duration | yes |
| `run_tests` | Run the project's test suite (command auto-detected) | yes |

The six read-only tools run without a prompt but are still confined to the project root. `patch_file` shows a diff preview before writing.

The approval classification is enforced, not advisory: a tool listed as mutating is refused outright if it ever reaches the dispatcher without a prompt, so adding a tool cannot silently skip the gate.

## Project structure

```
src/
├── main.rs           # entrypoint, CLI routing
├── cli.rs            # clap argument parsing (audit + undo subcommands, flags)
├── config.rs         # config loading, ConfigOverlay merge (global + project)
├── session.rs        # session state, model switching
├── headless.rs       # non-interactive / CI mode
├── audit.rs          # repository audit engine
├── llm/
│   ├── provider.rs   # Provider trait, message types, retry policy
│   ├── resolve.rs    # provider resolution logic
│   ├── openai.rs     # OpenAI-compatible + SSE streaming
│   ├── anthropic.rs  # Anthropic Messages API + SSE streaming
│   └── ollama.rs     # Ollama provider
├── tools/
│   ├── mod.rs        # Tool trait, registry, path sandbox (normalize/is_within)
│   ├── read_file.rs
│   ├── write_file.rs
│   ├── patch_file.rs  # unified-diff application
│   ├── create_dir.rs  # directory scaffolding
│   ├── symbols.rs     # tree-sitter AST symbol extraction
│   ├── search.rs     # glob + grep
│   ├── git.rs
│   ├── shell.rs      # shell_exec + run_command + run_tests
│   └── web.rs        # web_fetch
├── workspace.rs      # project type detection + build/test command inference
├── agent/
│   ├── mod.rs        # agent loop, tool dispatch, approval, run_id
│   ├── intent.rs     # 7-way intent classification (rules + LLM fallback)
│   ├── checkpoint.rs # git-stash checkpoints for /undo
│   ├── steer.rs      # discovery engine, DiscoveryReport, system prompt
│   ├── spec.rs       # requirements / design / tasks, checklist parsing
│   ├── autonomous.rs # autonomous run loop
│   ├── executor.rs   # grouped implementation with confirmation
│   ├── validation.rs # post-change validation gates
│   └── report.rs     # run reports
└── tui/
    ├── mod.rs        # event loop, key handling, slash commands
    ├── app.rs        # app state, run_id, ctrl_c_count, @-mentions
    ├── ui.rs         # rendering — header, chat body, input box, modals
    └── backend.rs    # resilient crossterm backend
```

## Development

```sh
cargo build --release
cargo test              # 183 tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) gates every push on fmt, clippy with `-D warnings`, the test suite, a 1.85 MSRV build check, and `cargo audit`.
