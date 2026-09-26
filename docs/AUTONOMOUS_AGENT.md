# Autonomous Coding Agent Architecture

## Overview

Vibectl has been refactored into an **autonomous coding agent** that can:
1. Understand user requirements
2. Plan implementation structure
3. Create/modify files automatically
4. Run validation commands
5. Analyze errors and fix them automatically
6. Iterate until success or blocking issue
7. Generate comprehensive implementation reports

## Core Principles

### 1. Evidence-Based Execution

The agent NEVER claims something worked without actually executing it.

```
❌ BAD: "I'll create package.json..."
✅ GOOD: [calls write_file tool] "Created package.json"

❌ BAD: "Tests should pass now"
✅ GOOD: [calls run_tests] "Tests passed (exit code: 0)"
```

### 2. Iterative Error Recovery

When validation fails, the agent:
1. Analyzes the error output
2. Extracts affected files from error messages
3. Identifies the root cause
4. Makes targeted fixes
5. Re-runs validation
6. Repeats up to MAX_ITERATIONS (5)

### 3. Project-Aware Behavior

The agent detects project type and adapts:
- Node.js → `npm install`, `npm test`
- Rust → `cargo check`, `cargo test`
- Go → `go mod download`, `go test ./...`
- Python → `pip install -r requirements.txt`, `pytest`

## Components

### WorkspaceContext (`src/workspace.rs`)

Provides project intelligence:

```rust
let workspace = WorkspaceContext::new(&cwd)?;

// Project type detection
match workspace.project_type {
    ProjectType::NodeJs => // use npm
    ProjectType::Rust => // use cargo
    ProjectType::Go => // use go
    ProjectType::Python => // use pip
    _ => // unknown
}

// Safety validation
let safe_path = workspace.validate_path("src/app.js")?;

// Get validation plan
let steps = workspace.validation_plan();
// Returns: install deps, lint, build, test
```

### ExecutionLoop (`src/agent/executor.rs`)

Runs validation with error analysis:

```rust
let mut executor = ExecutionLoop::new(workspace);

// Run validation steps
let results = executor.run_validation_steps(&steps, &tx).await?;

// Analyze failures
let analysis = executor.analyze_failures(&results);

if analysis.has_failures {
    println!("Error: {}", analysis.error_summary);
    println!("Affected files: {:?}", analysis.affected_files);
    println!("Suggestions: {:?}", analysis.suggestions);
}
```

**Error Pattern Detection**:
- Missing dependencies → suggests install command
- Syntax errors → identifies file and line
- Type errors → suggests type fixes
- Test failures → points to failing tests

### ValidationWorkflow (`src/agent/validation.rs`)

Orchestrates the autonomous loop:

```rust
let mut workflow = ValidationWorkflow::new(workspace);

// Run with automatic retry on failure
let report = workflow.run(&tx).await?;

// report contains:
// - All validation results
// - Iteration count
// - Error summaries
// - Blocking issues
```

**Iteration Flow**:
```
Iteration 1: Run validation
  ↓
  Failed: Missing dependency
  ↓
[Agent fixes: npm install express]
  ↓
Iteration 2: Run validation again
  ↓
  Failed: Syntax error in src/app.js:15
  ↓
[Agent fixes: corrects syntax]
  ↓
Iteration 3: Run validation again
  ↓
  Success! ✅
```

### ImplementationReport (`src/agent/report.rs`)

Generates comprehensive status reports:

```rust
let mut builder = ReportBuilder::new(workspace);

builder
    .add_created_file("src/app.js".to_string())
    .add_created_file("src/routes/users.js".to_string())
    .add_dependency("express".to_string())
    .add_implementation("Express REST API".to_string())
    .add_note("JWT authentication implemented".to_string())
    .set_validation_results(results);

let report = builder.build();

// Generate Markdown
let markdown = report.to_markdown();
fs::write("IMPLEMENTATION_SUMMARY.md", markdown)?;

// Terminal summary
println!("{}", report.to_summary());
```

## Tools Available to Agent

### File Operations
- `create_dir` — Create directory structure
- `read_file` — Read file contents
- `write_file` — Create or overwrite file
- `patch_file` — Apply targeted edits
- `glob` — Find files by pattern
- `grep` — Search file contents

### Command Execution
- `run_command` — Execute with structured output
  ```rust
  {
    "command": "npm install",
    "timeout_seconds": 300
  }
  ```
  Returns: exit_code, stdout, stderr, duration

- `run_tests` — Auto-detect and run tests
  ```rust
  {
    "test_command": "npm test"  // optional
  }
  ```
  Auto-detects: npm test, cargo test, go test, pytest

### Information Gathering
- `git` — Inspect git status/diff/log
- `list_symbols` — Extract function signatures
- `web_fetch` — Fetch documentation

## System Prompt Strategy

The enhanced system prompt (`DEFAULT_SYSTEM_PROMPT`) enforces:

1. **Conversational vs Task Mode**
   - Greetings → No tools
   - Questions → Read-only tools
   - "Create/Implement" → Full tool access + validation loop

2. **Mandatory Validation**
   ```
   After creating files, YOU MUST:
   1. Run dependency installation
   2. Run tests
   3. Analyze failures
   4. Fix issues
   5. Re-run tests
   6. Repeat until success or MAX_ITERATIONS
   ```

3. **No Hallucination Rules**
   ```
   ❌ Never claim a file exists without reading it
   ❌ Never assume a dependency is available
   ❌ Never skip error analysis
   ❌ Never retry without changes
   ```

4. **Error Recovery Protocol**
   ```
   1. READ error output completely
   2. IDENTIFY root cause
   3. EXTRACT affected files
   4. READ affected files if needed
   5. MAKE targeted fixes
   6. RE-RUN validation from beginning
   ```

## Integration with Existing Agent

### Current Agent Flow

```rust
// agent/mod.rs::run_inner()

loop {
    // 1. Send request to LLM
    let stream = provider.chat_stream(&req).await?;
    
    // 2. Receive tool calls
    if !tool_calls.is_empty() {
        for call in tool_calls {
            // 3. Execute tool
            let result = execute_tool(&call);
            
            // 4. Send result back to LLM
            push(Message::tool_result(call.id, result));
        }
        continue;
    }
    
    // 5. Done
    break;
}
```

### Enhanced Autonomous Flow

```rust
// agent/mod.rs::run_inner()

// Detect workspace
let workspace = WorkspaceContext::new(&self.cwd)?;

loop {
    let stream = provider.chat_stream(&req).await?;
    
    if !tool_calls.is_empty() {
        // Track file operations
        let mut files_created = Vec::new();
        
        for call in tool_calls {
            let result = execute_tool(&call);
            
            // Track what was created
            if call.name == "write_file" {
                files_created.push(extract_path(&call.arguments));
            }
            
            push(Message::tool_result(call.id, result));
        }
        
        // After file operations, trigger validation
        if !files_created.is_empty() {
            let mut workflow = ValidationWorkflow::new(workspace.clone());
            let report = workflow.run(&tx).await?;
            
            // If validation failed, send analysis to LLM
            if report.status != ImplementationStatus::Completed {
                push(Message::user(
                    "Validation failed. Analyze and fix:\n\n"
                    + &report.error_summary.unwrap_or_default()
                ));
                continue; // Let LLM fix and retry
            }
            
            // Success! Generate report
            let markdown = report.to_markdown();
            fs::write("IMPLEMENTATION_SUMMARY.md", markdown)?;
            
            send(AgentEvent::Text(report.to_summary()));
        }
        
        continue;
    }
    
    break;
}
```

## Usage Examples

### Example 1: Create New Project

```bash
vibectl --headless "Create an Express REST API for user management with JWT authentication"
```

**Agent Behavior**:
1. Detects empty workspace
2. Generates plan:
   - Project structure
   - Dependencies (express, jsonwebtoken, bcrypt)
   - Files (app.js, routes/, controllers/, middleware/)
   - Tests
3. Creates directories: `src/`, `src/routes/`, `tests/`
4. Creates `package.json`
5. Writes all source files
6. Runs `npm install`
7. Runs `npm test`
8. If tests fail: analyzes errors, fixes code, retries
9. Generates `IMPLEMENTATION_SUMMARY.md`

### Example 2: Fix Existing Project

```bash
vibectl --headless "Fix all failing tests"
```

**Agent Behavior**:
1. Detects existing project (Node.js)
2. Runs `npm test`
3. Analyzes test failures
4. Extracts affected files from stack traces
5. Reads those files
6. Identifies issues (e.g., missing assertion, wrong import)
7. Fixes the files
8. Re-runs `npm test`
9. Repeats until all tests pass or MAX_ITERATIONS

### Example 3: Add Feature

```bash
vibectl --headless "Add pagination to the /users endpoint"
```

**Agent Behavior**:
1. Detects Rust project (Cargo.toml)
2. Searches for users endpoint: `grep "users" src/**/*.rs`
3. Reads relevant files
4. Plans changes (add pagination params, modify query)
5. Patches files
6. Runs `cargo test`
7. If compilation fails: fixes syntax, retries
8. If tests fail: adjusts implementation, retries
9. Success: generates report

## Configuration

### Max Iterations

```rust
// src/agent/executor.rs
pub const MAX_ITERATIONS: u32 = 5;
```

Prevents infinite loops. Adjust based on complexity tolerance.

### Command Timeouts

```rust
// Per tool
run_command {
    command: "npm install",
    timeout_seconds: 300  // 5 minutes
}

run_tests {
    timeout_seconds: 300  // 5 minutes
}
```

### Workspace Safety

```rust
// Automatic - no configuration needed
// All paths validated against workspace root
workspace.validate_path("../etc/passwd")? // ❌ Error: outside workspace
workspace.validate_path("src/app.js")?    // ✅ OK
```

## Error Handling

### Compilation Errors

Pattern detected: `error: expected`, `syntax error`, `unexpected token`

**Agent Action**:
1. Extract file path and line number
2. Read file around error line
3. Fix syntax
4. Re-compile

### Missing Dependencies

Pattern detected: `cannot find module`, `unresolved import`

**Agent Action**:
1. Identify package name from error
2. Add to package.json / Cargo.toml / requirements.txt
3. Run install command
4. Re-run original command

### Test Failures

Pattern detected: `test failed`, `assertion failed`, `expected X but got Y`

**Agent Action**:
1. Read test output
2. Identify failing test name
3. Read test file
4. Read implementation file
5. Fix logic error
6. Re-run tests

### Timeout

Pattern: Command exceeds timeout

**Agent Action**:
1. Report timeout in analysis
2. Suggest: increase timeout or optimize code
3. Agent may choose to optimize if possible

## Monitoring & Debugging

### Execution Logs

Each iteration logs:
```
━━━ ITERATION 1/5 ━━━

=== INSTALL DEPENDENCIES ===
✓ Install Dependencies completed in 12.35s

=== RUN TESTS ===
✗ Run Tests failed (exit code: 1)

Error output:
FAIL src/app.test.js
  ✕ should return 404 for unknown routes (5ms)
  
Expected: 404
Received: 500
```

### Analysis Reports

```
=== VALIDATION FAILURE ANALYSIS ===

Failed Step: Run Tests
Command: npm test
Exit Code: 1

Affected Files:
  - src/app.js
  - src/app.test.js

Key error output:
[last 30 lines of stderr]

Suggested Actions:
1. Review failing test output and fix the implementation
2. Fix type mismatches in the code

IMPORTANT: You must analyze this error and take corrective action.
Do NOT retry the same command without making changes to fix the issue.
```

### Final Reports

See `IMPLEMENTATION_SUMMARY.md` for:
- What was implemented
- Files created/modified
- Dependencies installed
- Validation results table
- Test summary
- Execution stats
- Error summaries
- Blocking issues

## Best Practices

### For LLM Configuration

- **Temperature**: 0.1-0.3 for code generation (deterministic)
- **Max Tokens**: 4096+ (for complex generations)
- **Model**: Use capable models (GPT-4, Claude Sonnet 3.5+, Llama 3.2 70B+)

### For Complex Projects

- Break into phases: structure → implementation → tests
- Use `--plan` flag to review plan before execution
- Increase MAX_ITERATIONS for complex scenarios

### For Safety

- Always use workspace validation
- Enable approval mode for production environments
- Review generated files before committing
- Use version control (agent auto-creates checkpoints)

## Troubleshooting

### Agent Doesn't Run Validation

**Check**: Intent classification
- Is the request conversational? → Add explicit "create" or "implement"
- Try: "Implement feature X and run tests"

### Infinite Loop Despite MAX_ITERATIONS

**Check**: Is the agent making changes?
- Review iteration logs
- Ensure error analysis is being sent to LLM
- Check if LLM has tool access

### Validation Always Fails

**Check**: Are validation commands correct?
- Review `workspace.validation_plan()`
- Check if project type detected correctly
- Manually run commands to verify they work

### Reports Missing Information

**Check**: Is ReportBuilder being populated?
- Track file operations and call `add_created_file()`
- Set validation results with `set_validation_results()`
- Provide blocking issue if max iterations reached

## Future Enhancements

Potential improvements:

1. **Streaming Validation**: Show validation output in real-time
2. **Parallel Validation**: Run independent steps concurrently
3. **Adaptive Iteration Limit**: Increase limit for complex tasks
4. **Failure Classification**: Distinguish between "fixable" and "blocking" errors
5. **Learning from History**: Cache successful fix patterns
6. **Multi-File Diffs**: Show unified diff of all changes
7. **Rollback on Failure**: Auto-rollback if max iterations exceeded
8. **Cost Tracking**: Monitor LLM token usage per iteration

---

*Documentation generated for vibectl autonomous coding agent*
