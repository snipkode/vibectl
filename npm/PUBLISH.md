# Publishing vibectl to npm

## Prerequisites

1. **npm account**: Create at https://www.npmjs.com/signup
2. **npm login**: Run `npm login`
3. **Built binary**: Run `cargo build --release`

## Pre-publish Checklist

- [ ] Update version in `package.json`
- [ ] Update version in `../Cargo.toml`
- [ ] Build release binary: `cargo build --release`
- [ ] Copy binary: `cp ../target/release/vibectl bin/` (Linux/Mac) or `copy ..\target\release\vibectl.exe bin\` (Windows)
- [ ] Test locally: `npm test`
- [ ] Update CHANGELOG.md

## Publishing Steps

### 1. Copy Binary to npm Package

```bash
# Linux/Mac
cp ../target/release/vibectl bin/vibectl

# Windows
copy ..\target\release\vibectl.exe bin\vibectl.exe
```

### 2. Test Package Locally

```bash
cd npm
npm test
npm link
vibectl --help
```

### 3. Publish to npm

```bash
# Dry run first
npm publish --dry-run

# Publish for real
npm publish
```

### 4. Verify Published Package

```bash
# Install globally from npm
npm install -g vibectl

# Test
vibectl --help
```

## Version Management

Update version in both files:
- `npm/package.json`
- `Cargo.toml`

```bash
# Patch version (0.1.0 -> 0.1.1)
npm version patch

# Minor version (0.1.0 -> 0.2.0)
npm version minor

# Major version (0.1.0 -> 1.0.0)
npm version major
```

## Publishing Pre-releases

```bash
# Beta
npm version prerelease --preid=beta
npm publish --tag beta

# Alpha
npm version prerelease --preid=alpha
npm publish --tag alpha
```

Install pre-release:
```bash
npm install -g vibectl@beta
```

## GitHub Releases (Future)

For automatic binary distribution from GitHub Releases:

1. Create release on GitHub: https://github.com/snipkode/vibectl/releases/new
2. Tag format: `v0.1.0`
3. Upload binaries for each platform:
   - `vibectl-windows-x86_64.exe`
   - `vibectl-macos-x86_64`
   - `vibectl-macos-aarch64`
   - `vibectl-linux-x86_64`
   - `vibectl-linux-aarch64`

The `download-binary.js` script will automatically download the correct binary during `npm install`.

## Platform-Specific Packages (Optional Future)

Create separate npm packages for each platform:
- `@vibectl/win32-x64`
- `@vibectl/darwin-x64`
- `@vibectl/darwin-arm64`
- `@vibectl/linux-x64`
- `@vibectl/linux-arm64`

Main `vibectl` package will automatically select the right one as optional dependency.

## Troubleshooting

### Package too large
- Check `.npmignore` is excluding unnecessary files
- Don't include source code, only binary

### Binary not executable
```bash
# Linux/Mac
chmod +x bin/vibectl
```

### Wrong binary architecture
Make sure you're building for the correct target:
```bash
# Cross-compile examples
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## Useful Commands

```bash
# Check package contents
npm pack --dry-run

# View package size
npm publish --dry-run | grep "size"

# Unpublish (within 72 hours)
npm unpublish vibectl@0.1.0

# Deprecate version
npm deprecate vibectl@0.1.0 "Use 0.1.1 instead"
```

## Automation (Future)

Create GitHub Action for automatic publishing:
- `.github/workflows/publish-npm.yml`
- Triggers on new release
- Builds binaries for all platforms
- Publishes to npm automatically

## Notes

- npm package includes ONLY the binary, not source code
- Source code lives in GitHub repository
- Users who want to build from source should clone the repo
- Pre-built binaries are for convenience

## Support

For issues: https://github.com/snipkode/vibectl/issues
