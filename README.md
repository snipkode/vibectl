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
- **Tools**: `read_file`, `write_file`, `glob`, `grep`, `git`, `shell_exec`, `web_fetch`
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
```

## Configuration

Global config: `~/.config/vibectl/config.yaml`
Project config: `.vibectl/config.yaml` (merged over global)

```yaml
model: gpt-4o           # or claude-*, llama3.2, etc.
temperature: 0.2
max_tokens: 4096
openai_base_url: https://api.openai.com/v1
ollama_base_url: http://localhost:11434
custom_base_url: https://your-endpoint/v1
custom_api_key: ""
```

Provider resolution order: model prefix (`claude-*` → Anthropic, `gpt-*` → OpenAI) → configured provider → Ollama fallback.

Override model via env: `VIBECTL_MODEL=gpt-4o-mini vibectl`

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
  ✓  DONE     Interactive TUI
  ✓  DONE     Multi-provider LLM
  ✗  MISSING  JSON output
  ⚠  PARTIAL  Progress indicators

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
| `/plan <task>` | Generate implementation plan |
| `/undo` | Rollback last agent file changes (git stash pop) |
| `/steer <rule>` | Append rule to `.vibectl/steer.md` |
| `/new` | Reset conversation history |
| `/spec` | Show current plan |
| `/cfg` | Print effective config |
| `/provider` | Show provider + model |
| `/clear` | Clear message view |
| `/help` | Toggle help |

## Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file with offset/limit |
| `write_file` | Create or overwrite a file (requires approval) |
| `glob` | Find files by pattern |
| `grep` | Search file contents by regex |
| `git` | status / diff / log in the project |
| `shell_exec` | Run any shell command (requires approval) |
| `web_fetch` | Fetch and extract content from a URL |

## Project structure

```
src/
├── main.rs           # entrypoint, CLI routing
├── cli.rs            # clap argument parsing (audit subcommand, flags)
├── config.rs         # config loading + merge (global + project)
├── session.rs        # session state, model switching
├── headless.rs       # non-interactive / CI mode
├── audit.rs          # repository audit engine
├── llm/
│   ├── provider.rs   # Provider trait, message types
│   ├── resolve.rs    # provider resolution logic
│   ├── openai.rs     # OpenAI-compatible + Ollama + custom
│   └── anthropic.rs  # Anthropic Messages API + SSE streaming
├── tools/
│   ├── read_file.rs
│   ├── write_file.rs
│   ├── search.rs     # glob + grep
│   ├── git.rs
│   ├── shell.rs
│   └── web.rs        # web_fetch
├── agent/
│   ├── mod.rs        # agent loop, tool dispatch, approval, run_id
│   └── steer.rs      # discovery engine, DiscoveryReport, system prompt
└── tui/
    ├── mod.rs        # event loop, key handling, slash commands
    ├── app.rs        # app state, run_id, ctrl_c_count
    ├── ui.rs         # rendering — header, chat body, input box, modals
    └── backend.rs    # resilient crossterm backend
```
