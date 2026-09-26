# NPM Package Summary

## 📦 Package Structure

```
npm/
├── package.json           # npm package metadata
├── index.js              # Main entry point (exports version info)
├── README.md             # npm-specific documentation
├── PUBLISH.md            # Publishing guide
├── .npmignore           # Files to exclude from npm
├── bin/
│   └── vibectl.js       # Wrapper script that calls Rust binary
├── scripts/
│   └── download-binary.js  # Post-install script
└── test/
    └── version.test.js  # Basic tests
```

## 🚀 Installation Methods

### 1. Global Installation (End Users)
```bash
npm install -g vibectl
```

This will:
- Install the npm package globally
- Run `postinstall` script to download/build binary
- Make `vibectl` command available system-wide

### 2. Local Testing (Development)
```bash
cd npm
npm link
vibectl --help
```

### 3. From Source
```bash
git clone https://github.com/snipkode/vibectl.git
cd vibectl
cargo build --release
cd npm
npm link
```

## 📝 Publishing to npm

### Prerequisites
1. Create npm account: https://www.npmjs.com/signup
2. Login: `npm login`
3. Build binary: `cargo build --release`

### Steps

```bash
# 1. Copy binary to npm package
cd npm
mkdir -p bin
cp ../target/release/vibectl bin/  # Linux/Mac
# or
copy ..\target\release\vibectl.exe bin\  # Windows

# 2. Test locally
npm test
npm link
vibectl --help

# 3. Check package contents
npm pack --dry-run

# 4. Publish
npm publish
```

### Version Management

Update version in BOTH files:
- `npm/package.json`
- `Cargo.toml`

```bash
cd npm
npm version patch  # 0.1.0 -> 0.1.1
npm version minor  # 0.1.0 -> 0.2.0
npm version major  # 0.1.0 -> 1.0.0
```

## 🔧 How It Works

### Installation Flow

```
npm install -g vibectl
    ↓
postinstall script runs (download-binary.js)
    ↓
Try to download pre-built binary from GitHub Releases
    ↓
If fails: Try to build from source with cargo
    ↓
If fails: Show error with instructions
    ↓
Binary installed to npm/bin/
    ↓
vibectl command available globally
```

### Execution Flow

```
User runs: vibectl --headless "create api"
    ↓
npm/bin/vibectl.js wrapper script
    ↓
Locates Rust binary (vibectl or vibectl.exe)
    ↓
Spawns child process with all arguments
    ↓
Forwards stdio (stdin, stdout, stderr)
    ↓
Exits with same code as Rust binary
```

## 📦 Binary Distribution

### Option 1: Include Binary in npm Package (Current)
**Pros:**
- Works immediately after `npm install`
- No external dependencies

**Cons:**
- Large package size (~10-30 MB)
- Need platform-specific packages or multi-platform binary

### Option 2: Download from GitHub Releases (Recommended for Production)
**Pros:**
- Small npm package size (~50 KB)
- Binaries hosted on GitHub
- Automatic platform detection

**Cons:**
- Requires GitHub Releases setup
- Network required during installation

**Setup:**
1. Create GitHub Release: `v0.1.0`
2. Upload binaries:
   - `vibectl-windows-x86_64.exe`
   - `vibectl-macos-x86_64`
   - `vibectl-macos-aarch64`
   - `vibectl-linux-x86_64`
   - `vibectl-linux-aarch64`
3. The `download-binary.js` script will automatically download correct binary

## 🎯 Platform Support

### Supported Platforms
- **Windows**: x64, arm64
- **macOS**: x64 (Intel), arm64 (Apple Silicon)
- **Linux**: x64, arm64

### Binary Naming Convention
```
vibectl-{platform}-{arch}{ext}

Examples:
- vibectl-windows-x86_64.exe
- vibectl-macos-x86_64
- vibectl-macos-aarch64
- vibectl-linux-x86_64
- vibectl-linux-aarch64
```

## 📊 Package Metadata

```json
{
  "name": "vibectl",
  "version": "0.1.0",
  "description": "Autonomous coding agent",
  "repository": "https://github.com/snipkode/vibectl.git",
  "license": "MIT",
  "engines": {
    "node": ">=14.0.0"
  },
  "os": ["darwin", "linux", "win32"],
  "cpu": ["x64", "arm64"]
}
```

## 🧪 Testing

```bash
cd npm
npm test
```

Tests verify:
- Binary exists and is executable
- Help command works
- Exit codes are correct

## 🔍 Troubleshooting

### Binary not found
```bash
# Reinstall
npm install -g vibectl --force

# Or build from source
cd path/to/vibectl
cargo build --release
cd npm
npm link
```

### Permission denied (Linux/Mac)
```bash
chmod +x ~/.npm-global/lib/node_modules/vibectl/bin/vibectl
```

### Wrong architecture
Make sure you're on a supported platform:
```bash
node -p "process.platform + '-' + process.arch"
# Should output: win32-x64, darwin-x64, darwin-arm64, linux-x64, etc.
```

## 📚 Files Published to npm

From `.npmignore`, these files are **included**:
- `bin/` - Binary files
- `scripts/` - Installation scripts
- `README.md` - Documentation
- `package.json` - Metadata
- `index.js` - Main entry

These are **excluded**:
- `node_modules/`
- Source code (`src/`, `Cargo.toml`)
- Development files (`.git/`, `.vscode/`)
- Tests (`test/`, `*.test.js`)

## 🎉 Usage After Installation

```bash
# Install
npm install -g vibectl

# Use
vibectl --help
vibectl --headless "Create Express API" --dangerous-yes
vibectl -m llama3.2

# Update
npm update -g vibectl

# Uninstall
npm uninstall -g vibectl
```

## 🚀 Next Steps

1. **Build release binary**:
   ```bash
   cargo build --release
   ```

2. **Copy to npm package**:
   ```bash
   cp target/release/vibectl npm/bin/
   ```

3. **Test locally**:
   ```bash
   cd npm
   npm link
   vibectl --help
   ```

4. **Publish to npm**:
   ```bash
   npm login
   npm publish
   ```

5. **Create GitHub Release** (for automatic binary downloads)

---

**Ready to publish!** 🎉

Repository: https://github.com/snipkode/vibectl
npm package: https://www.npmjs.com/package/vibectl (after publishing)
