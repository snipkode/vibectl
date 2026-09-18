# vibectl

A terminal-based agentic coding CLI ("vibe coding" agent), inspired by Kiro. Chat with a coding agent that can read, write, and search your codebase, run shell commands (with approval), and inspect git state.

## Features

- **Interactive TUI** (ratatui) with streaming responses
- **Multi-provider LLM** support:
  - OpenAI (`gpt-*` models, `OPENAI_API_KEY`)
  - Anthropic (`claude-*` models, `ANTHROPIC_API_KEY`)
  - Any OpenAI-compatible endpoint via `ollama` or custom base URL
- **Agent loop** with tool calling
- **Tools**: `read_file`, `write_file`, `glob`, `grep`, `git`, `shell_exec` (shell requires approval)
- **Agent steering** — reads `AGENTS.md`, `CLAUDE.md`, and `.vibectl/steer.md` as system context
- **Plan mode** — generate and save an implementation plan to `.vibectl/PLAN.md`
- **Headless mode** — for scripting and CI
- **Session persistence** — history saved to `~/.config/vibectl/history.json`

## Install

Requires Rust 1.85+ (edition 2024).

```sh
cargo build --release
# binary at target/release/vibectl
```

## Usage

```sh
# Interactive TUI
vibectl

# Run in a specific project directory
vibectl -c /path/to/project

# Pick a model explicitly
vibectl -m gpt-4o
vibectl -m claude-sonnet-4-20250514

# One-shot headless prompt (no TUI)
vibectl --headless "explain this codebase"

# Generate a plan only
vibectl --headless --plan "add a new feature"

# Allow shell commands without approval (dangerous, CI only)
vibectl --headless --dangerous-yes "run tests and fix failures"
```

## Configuration

Config is loaded from `~/.config/vibectl/config.yaml` and merged with project-level `.vibectl/config.yaml`.

```yaml
# ~/.config/vibectl/config.yaml
provider: auto            # auto | openai | anthropic | ollama | custom
model: gpt-4o
openai_base_url: https://api.openai.com/v1
ollama_base_url: http://localhost:11434
custom_base_url: https://your-endpoint/v1
custom_api_key: ""        # optional
temperature: 0.3
max_tokens: 4096
```

Provider resolution: model prefixes (`claude-*` → Anthropic, `gpt-*` → OpenAI) take precedence, then a configured provider, then Ollama as fallback.

## Steering

Place a `AGENTS.md` or `.vibectl/steer.md` in the project to inject rules into every request:

```md
# Rules
- Always run `cargo fmt` after editing Rust code.
- Never commit to main without a review.
```

## TUI keybindings / commands

| Key / Command | Action |
|---|---|
| Enter | Send message |
| Shift+Enter | Newline |
| ↑ / ↓ | Input history |
| `/quit` `/exit` | Quit |
| `/clear` | Clear conversation |
| `/provider <name>` | Switch provider |
| `/model <name>` | Switch model |
| `/cfg` | Show provider + model config |
| `/new` | New session |
| `/plan <task>` | Generate a plan |
| `/steer <rule>` | Add a steering rule |
| `/spec` | Show codebase summary prompt |

## Tools

| Tool | Description |
|---|---|
| `read_file` | Read a file with offset/limit |
| `write_file` | Create/overwrite a file |
| `glob` | Find files by pattern |
| `grep` | Search file contents |
| `git` | status/diff/log in project |
| `shell_exec` | Run a shell command (requires approval) |

## Project structure

```
src/
├── main.rs          # entrypoint
├── cli.rs           # argument parsing
├── config.rs        # config loading + merge
├── session.rs       # session state, history, steering
├── headless.rs      # non-interactive mode
├── llm/
│   ├── provider.rs  # trait ChatRequest/ChatChunk/dtos
│   ├── resolve.rs   # provider resolution
│   ├── openai.rs    # OpenAI-compatible + custom + Ollama
│   ├── anthropic.rs # Anthropic Messages API + SSE
├── tools/           # tool implementations
├── agent/
│   ├── mod.rs       # agent loop, tool merging, approval
│   └── steer.rs     # steering file discovery
└── tui/
    ├── mod.rs       # event loop, slash commands, approval UI
    ├── app.rs       # app state
    └── ui.rs        # rendering
```