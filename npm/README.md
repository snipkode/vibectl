# vibectl

> Autonomous coding agent - AI that writes, tests, and fixes code automatically

Vibectl is an autonomous software engineering agent that can:
- 🤖 **Create complete projects** from scratch
- 🔄 **Auto-fix errors** with iterative testing (up to 5 attempts)
- 🎯 **Detect project types** automatically (Node.js, Rust, Go, Python)
- 📊 **Generate comprehensive reports** with validation results
- 🛡️ **Safe by design** with workspace boundaries and validation

## Installation

### Global Installation (Recommended)

```bash
npm install -g vibectl
```

### From Source

```bash
git clone https://github.com/snipkode/vibectl.git
cd vibectl
cargo build --release
npm link ./npm
```

## Quick Start

### Interactive Mode (TUI)

```bash
vibectl
```

Then type your request:
```
> Create an Express API for user management with JWT authentication
```

The agent will:
1. ✅ Inspect workspace
2. ✅ Plan implementation
3. ✅ Create all files
4. ✅ Install dependencies
5. ✅ Run tests
6. ✅ Fix errors automatically
7. ✅ Generate report

### Headless Mode (Automation)

```bash
# Create new project
vibectl --headless "Create a REST API for todo management" --dangerous-yes

# Fix existing project
vibectl --headless "Fix all failing tests"

# Add feature
vibectl --headless "Add pagination to /users endpoint"
```

### With Ollama (Local LLM)

```bash
vibectl -m llama3.2 --headless "Explain this codebase"
```

## Features

### 🤖 Autonomous Agent
- Creates projects and writes code automatically
- Runs validation commands (install, build, test)
- Analyzes errors and fixes them iteratively
- Never claims success without actual execution

### 🎯 Project-Aware
Auto-detects and handles:
- **Node.js**: `npm install` → `npm test` → `npm run build`
- **Rust**: `cargo check` → `cargo test` → `cargo clippy`
- **Go**: `go mod download` → `go test ./...` → `go build`
- **Python**: `pip install -r requirements.txt` → `pytest`

### 🔄 Error Recovery Loop
```
Iteration 1: npm test → FAILED (missing dependency)
    ↓ Agent installs dependency
Iteration 2: npm test → FAILED (syntax error)
    ↓ Agent fixes syntax
Iteration 3: npm test → PASSED ✅
```

### 📊 Comprehensive Reports
Generates `IMPLEMENTATION_SUMMARY.md` with:
- Status (✅ Completed / ⚠️ Partially / 🚫 Blocked / ❌ Failed)
- Files created/modified
- Dependencies installed
- Validation results table
- Test summary
- Execution statistics

### 🛡️ Safety Features
- Workspace boundary enforcement (no `../`, `/tmp`, `~`)
- Command timeouts (default: 300s)
- Iteration limits (max: 5)
- Evidence-based reporting

## Configuration

### Global Config
`~/.config/vibectl/config.yaml`

```yaml
model: llama3.2              # or gpt-4o, claude-sonnet-4
temperature: 0.2             # lower for deterministic code
max_tokens: 4096
ollama_base_url: http://localhost:11434
```

### Project Config
`.vibectl/config.yaml` (overrides global)

```yaml
model: gpt-4o-mini
temperature: 0.1
```

### Environment Variables
```bash
export VIBECTL_MODEL=llama3.2
export OPENAI_API_KEY=sk-...
export ANTHROPIC_API_KEY=sk-ant-...
```

## Usage Examples

### Create Express API
```bash
vibectl --headless \
  "Create an Express REST API for product management with CRUD operations and JWT auth" \
  --dangerous-yes
```

**Result**:
- ✅ Complete project structure
- ✅ All files created (routes, controllers, middleware, tests)
- ✅ Dependencies installed
- ✅ Tests passing
- ✅ Report saved to `IMPLEMENTATION_SUMMARY.md`

### Fix Failing Tests
```bash
vibectl --headless "Fix all failing tests"
```

**Agent will**:
1. Run tests
2. Analyze failures
3. Extract affected files
4. Fix code
5. Re-run tests
6. Repeat until all pass

### Add Feature to Existing Project
```bash
vibectl --headless "Add pagination to the /products endpoint with limit and offset parameters"
```

**Agent will**:
1. Detect Rust/Node.js/Go/Python
2. Find relevant files
3. Modify code
4. Update tests
5. Run validation
6. Report status

## Commands

### TUI Mode

| Key | Action |
|-----|--------|
| `Enter` | Send message |
| `Ctrl+C` | Cancel / Exit |
| `Ctrl+K` or `?` | Toggle help |
| `↑` / `↓` | History navigation |
| `PgUp` / `PgDn` | Scroll conversation |

| Slash Command | Description |
|---------------|-------------|
| `/help` | Toggle help panel |
| `/model <name>` | Switch model |
| `/plan <task>` | Generate plan |
| `/undo` | Rollback changes |
| `/new` | Reset conversation |

### Headless Mode Flags

```
--headless              Non-interactive mode
--dangerous-yes         Auto-approve all actions
--plan                  Generate plan only
-m, --model <name>      Use specific model
-c, --cwd <path>        Working directory
```

## Supported LLM Providers

- **Ollama** (local): `llama3.2`, `codellama`, `deepseek-coder`
- **OpenAI**: `gpt-4o`, `gpt-4o-mini`, `gpt-4-turbo`
- **Anthropic**: `claude-sonnet-4`, `claude-opus-4`
- **Custom OpenAI-compatible** endpoints

## Requirements

- **Node.js**: 14.0.0 or higher (for npm package)
- **Rust**: 1.70+ (for building from source)
- **LLM**: Ollama installed OR API keys for OpenAI/Anthropic

## Architecture

```
User Request
     ↓
WorkspaceContext (detect project type)
     ↓
Planning Phase (generate comprehensive plan)
     ↓
Implementation (create_dir, write_file, run_command)
     ↓
ValidationWorkflow
     ├── ExecutionLoop (run validation steps)
     ├── AnalysisReport (analyze failures)
     └── IterationHistory (track attempts)
     ↓
ImplementationReport (Markdown summary)
```

## Troubleshooting

### Binary not found
```bash
npm install -g vibectl --force
```

Or build from source:
```bash
cd path/to/vibectl
cargo build --release
npm link ./npm
```

### Agent doesn't validate
Make sure you're using `--dangerous-yes` flag:
```bash
vibectl --headless "your request" --dangerous-yes
```

### Tests fail after max iterations
Check `IMPLEMENTATION_SUMMARY.md` for:
- Error messages
- Blocking issues
- Affected files

Then manually fix or adjust the request.

## Contributing

Contributions welcome! Please:
1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Run tests: `cargo test`
5. Submit a pull request

## License

MIT License - see [LICENSE](../LICENSE) file for details

## Links

- **GitHub**: https://github.com/snipkode/vibectl
- **Issues**: https://github.com/snipkode/vibectl/issues
- **Documentation**: https://github.com/snipkode/vibectl/blob/main/README.md

---

Made with ❤️ by the vibectl community
