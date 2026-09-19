# Vibectl Bug Fixes Summary

## Fix #1: Filter JSON Tool Calls dari UI Output ✅

### Problem
Agent menampilkan internal system execution JSON ke user:
```
}; {"name":"write_file","parameters":{"path":"package.json",...}}; 
Berikut adalah contoh project Express.js...
```

### Root Cause
LLM (Llama 3.2 via Ollama) mengeluarkan tool calls sebagai **text chunks** dalam streaming response, bukan sebagai structured tool calls.

### Solution
**Source-level filtering** di `src/agent/mod.rs`:
```rust
if let Some(c) = chunk.content {
    text.push_str(&c);  // Keep for LLM context
    if !should_filter_text_chunk(&c) {  // Filter before display
        let _ = tx.send(AgentEvent::Text(c)).await;
    }
}
```

Function `should_filter_text_chunk()` detects:
- JSON patterns: `{"name":"...","parameters":{...}}`
- Artifacts: `};`, whitespace chunks

### Files Modified
- `src/agent/mod.rs` - Added filter function + applied in streaming
- `src/tui/mod.rs` - Backup filter layer
- `Cargo.toml` - Added `lazy_static = "1.4"`

---

## Fix #2: Missing Tools in Intent Mapping ✅

### Problem
Agent mencoba memanggil tool yang "not defined":
```
{"name":"create","parameters":{"path":"express"}}
[agent invented a tool "create" that is not defined. 
 Available tools: git, read_file, write_file, patch_file, 
 list_symbols, glob, grep, shell_exec, web_fetch]
```

### Root Cause
Tools `create_dir`, `run_command`, `run_tests` sudah **registered** di `all_tools()` tapi **tidak available** untuk intent tertentu karena tidak ada di `tools_for_intent()` mapping.

### Solution
Added missing tools to intent mapping di `src/agent/mod.rs`:

#### CodeWrite + Refactor
```rust
+ "create_dir",  // Untuk scaffold directory structure
```

#### ShellExec + Deploy
```rust
+ "run_command",  // Structured command execution
+ "run_tests",    // Run tests with validation
```

### Files Modified
- `src/agent/mod.rs` - Updated `tools_for_intent()` function

---

## Fix #3: Copy Mode Arrow Keys Trigger History ✅

### Problem
Di **copy mode** (Alt+C), arrow keys (↑↓) trigger input history navigation instead of allowing text selection/scrolling.

### Expected vs Actual
- **Expected:** Arrow keys → text selection cursor movement
- **Actual:** Arrow keys → fill input with history entries

### Root Cause
`KeyCode::Up` dan `KeyCode::Down` handlers tidak check apakah `copy_mode` aktif.

### Solution
Added guard di `src/tui/mod.rs`:
```rust
KeyCode::Up => {
    if app.copy_mode {
        return;  // Let terminal handle for text selection
    }
    // ... normal history handling
}
```

### Files Modified
- `src/tui/mod.rs` - Added copy_mode guards for arrow keys
- `src/tui/mod.rs` - Updated copy mode activation message

---

## Combined Testing

```bash
cd /mnt/d/works/vibectl
cargo build --release

# Test Fix #1 & #2: Agent functionality
./target/release/vibectl -m llama3.2 "buatkan project express api dengan structure yang rapi"
# Expected: Clean output, create_dir works, files created

# Test Fix #3: Copy mode
./target/release/vibectl
# 1. Send some messages
# 2. Press Alt+C (copy mode)
# 3. Press ↑↓ → should move cursor, NOT history
# 4. Press PgUp/PgDn → should scroll
# 5. Press Alt+C → back to normal
# 6. Press ↑↓ → should navigate history
```

---

## Impact Summary

### Before Fixes
```
Issue #1: }; {"name":"write_file",...}}; [messy JSON in chat]
Issue #2: [tool "create" not defined]
Issue #3: Copy mode → ↑↓ fills input with history
Result: ❌ Poor UX, broken functionality
```

### After Fixes
```
Issue #1: Clean professional output ✅
Issue #2: create_dir, run_command, run_tests available ✅
Issue #3: Copy mode → ↑↓ moves cursor for selection ✅
Result: ✅ Professional UX, full functionality
```

---

## Next Steps
1. ✅ Compile & test all fixes
2. ✅ Verify with Llama 3.2 via Ollama
3. 🔄 Build release binary
4. 🔄 Update npm package with new binary
5. 🔄 Publish to npm

## Dependencies Added
- `lazy_static = "1.4"` - For efficient regex compilation

## Documentation
- `FIXES_SUMMARY.md` - This file (comprehensive overview)
- `FIX_COPY_MODE_SCROLL.md` - Detailed copy mode fix documentation
