# Coding Conventions & Auto-Verify Rules

## Auto-Verify After Every Code Change

After writing ANY file (write_file, patch_file), you MUST immediately:

1. **Install deps** — run_command("npm install") / "pip install -r requirements.txt" / "go mod tidy"
2. **Build/syntax-check** — run_command("node --check index.js") / "cargo build" / "go build ./..."
3. **Run tests** — run_tests, or run_command("npm test") / "cargo test" / "pytest"
4. **Review diff** — git_diff to confirm what was written

You are NOT done until all four steps pass with exit_code 0.

## Per-Language Quick Reference

### Node.js / Express
```
After writing files:
  run_command("npm install")
  run_command("node --check <entrypoint>.js")   ← syntax check, no server start
  run_command("npm test")                        ← if tests exist
  git_diff
```

### Rust
```
After writing files:
  run_command("cargo build")
  run_command("cargo test")
  git_diff
```

### Python
```
After writing files:
  run_command("pip install -r requirements.txt")
  run_command("python -m py_compile <file>.py")
  run_command("pytest") or run_tests
  git_diff
```

### Go
```
After writing files:
  run_command("go mod tidy")
  run_command("go build ./...")
  run_command("go test ./...")
  git_diff
```

## Hard Stop Rules

- NEVER end your turn with "I have created the project" without running steps 1-4 above
- NEVER say "the API is ready" without a successful run_command result (exit_code 0)
- NEVER skip `npm install` for a new Node.js project — it will fail at runtime
- If any step fails: read stderr, fix the file, re-run ALL steps from step 1
- Maximum 5 retry cycles before reporting the blocker

## Code Quality

- Use exact/pinned dependency versions in manifests (no open ranges like `"^1.0"` without reason)
- Add error handling for every async operation (try/catch in JS, Result in Rust, try/except in Python)
- Never write TODO comments as a substitute for actual implementation
- Validate all external input (request bodies, env vars, CLI args)
- Never hardcode credentials, ports, or environment-specific values — use env vars

## New Project Structure

For a new Node.js/Express project, create files in this order:
1. `package.json` (with name, version, main, scripts.test, dependencies)
2. Source files (`index.js`, `routes/`, `middleware/`, etc.)
3. `README.md`
4. Then immediately: npm install → syntax check → test
