# Fix: Copy Mode Arrow Keys Trigger History

## Problem
Di **copy mode**, ketika user mencoba scroll atau select text dengan arrow keys (↑↓), malah trigger **input history navigation** instead of allowing text selection.

### Expected Behavior
- Copy mode: Arrow keys → text selection/navigation
- Normal mode: Arrow keys → input history (↑ previous, ↓ next)

### Actual Behavior (Before Fix)
```
User: Alt+C (activate copy mode)
User: Press ↑ to scroll up
Result: ❌ Input field filled with previous history entry
        ❌ Cannot select text properly
```

## Root Cause
Di `src/tui/mod.rs` line ~444-461, `KeyCode::Up` dan `KeyCode::Down` handler **tidak check** apakah `copy_mode` aktif:

```rust
KeyCode::Up => {
    if app.at_visible {
        app.at_prev();
    } else if app.suggestion_visible {
        app.suggestion_prev();
    } else {
        app.history_prev();  // ❌ Always triggered
    }
}
```

## Solution

### Added Copy Mode Guard
```rust
KeyCode::Up => {
    // In copy mode, let terminal handle arrow keys for text selection
    if app.copy_mode {
        return;  // ✅ Skip all handling
    }
    if app.at_visible {
        app.at_prev();
    } else if app.suggestion_visible {
        app.suggestion_prev();
    } else {
        app.history_prev();
    }
}

KeyCode::Down => {
    // In copy mode, let terminal handle arrow keys for text selection
    if app.copy_mode {
        return;  // ✅ Skip all handling
    }
    // ... rest of handling
}
```

### Updated User Message
```rust
"Copy mode ON — select text with mouse or arrow keys. PgUp/PgDn to scroll. Alt+C or /copy to exit."
```

## Impact

### Before Fix
```
Mode: Copy Mode (Alt+C pressed)
Action: Press ↑
Result: Input = "previous command from history"
UX: ❌ Frustrating, cannot select text properly
```

### After Fix
```
Mode: Copy Mode (Alt+C pressed)
Action: Press ↑
Result: Cursor moves up in terminal (text selection)
UX: ✅ Natural text selection behavior
```

## Files Modified
1. `src/tui/mod.rs`
   - Added `copy_mode` guard for `KeyCode::Up` handler
   - Added `copy_mode` guard for `KeyCode::Down` handler
   - Updated copy mode activation message

## Testing

```bash
cd /mnt/d/works/vibectl
cargo build --release
./target/release/vibectl

# In TUI:
1. Send a few messages to create history
2. Press Alt+C (activate copy mode)
3. Press ↑↓ arrow keys
   Expected: Cursor moves in chat content, NO history navigation
4. Press PgUp/PgDn
   Expected: Scroll works normally
5. Press Alt+C again (exit copy mode)
6. Press ↑↓ arrow keys
   Expected: History navigation works (previous/next commands)
```

## Related Features

### Copy Mode (Alt+C or /copy)
- **Purpose:** Allow user to select and copy terminal text
- **How it works:**
  - Disables mouse capture (wheel scrolling off)
  - Now also disables arrow key history navigation
  - User can freely select text with mouse/keyboard
- **Navigation:**
  - PgUp/PgDn: Scroll chat content
  - Arrow keys: Text selection cursor movement
  - Alt+C or /copy: Exit copy mode

### Normal Mode (default)
- **Mouse capture:** ON (wheel scrolling enabled)
- **Arrow keys:** Input history navigation (↑ previous, ↓ next)
- **@ autocomplete:** Arrow keys navigate suggestions
- **File suggestions:** Arrow keys navigate file list

## Design Notes
When `copy_mode` is active:
1. Mouse capture disabled → user can click/drag to select
2. Arrow keys passthrough → terminal handles them natively
3. PgUp/PgDn still handled by app → consistent scroll behavior
4. All other keys work normally (Ctrl+C, Alt+C, typing, etc.)

This creates intuitive UX where copy mode behaves like standard terminal text selection.
