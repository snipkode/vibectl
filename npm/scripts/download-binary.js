#!/usr/bin/env node

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const GITHUB_REPO = 'snipkode/vibectl';
const VERSION = require('../package.json').version;

function getPlatformInfo() {
  const platform = process.platform;
  const arch = process.arch;
  
  const platformMap = {
    'win32': 'windows',
    'darwin': 'macos',
    'linux': 'linux',
  };
  
  const archMap = {
    'x64': 'x86_64',
    'arm64': 'aarch64',
  };
  
  return {
    platform: platformMap[platform] || platform,
    arch: archMap[arch] || arch,
    ext: platform === 'win32' ? '.exe' : '',
  };
}

function getDownloadUrl() {
  const { platform, arch, ext } = getPlatformInfo();
  const binaryName = `vibectl-${platform}-${arch}${ext}`;
  
  // GitHub Releases URL format
  return `https://github.com/${GITHUB_REPO}/releases/download/v${VERSION}/${binaryName}`;
}

function downloadBinary(url, dest) {
  return new Promise((resolve, reject) => {
    console.log('Downloading vibectl binary...');
    console.log('URL:', url);
    
    const file = fs.createWriteStream(dest);
    
    https.get(url, (response) => {
      if (response.statusCode === 302 || response.statusCode === 301) {
        // Follow redirect
        return downloadBinary(response.headers.location, dest)
          .then(resolve)
          .catch(reject);
      }
      
      if (response.statusCode !== 200) {
        reject(new Error(`Download failed with status ${response.statusCode}`));
        return;
      }
      
      response.pipe(file);
      
      file.on('finish', () => {
        file.close();
        resolve();
      });
    }).on('error', (err) => {
      fs.unlink(dest, () => {}); // Delete partial file
      reject(err);
    });
  });
}

async function install() {
  const binDir = path.join(__dirname, '..', 'bin');
  const { ext } = getPlatformInfo();
  const binaryPath = path.join(binDir, `vibectl${ext}`);
  
  // Create bin directory if it doesn't exist
  if (!fs.existsSync(binDir)) {
    fs.mkdirSync(binDir, { recursive: true });
  }
  
  // Check if binary already exists (e.g., from cargo build)
  if (fs.existsSync(binaryPath)) {
    console.log('✓ vibectl binary already exists');
    return;
  }
  
  // Try to download from GitHub releases
  try {
    const url = getDownloadUrl();
    await downloadBinary(url, binaryPath);
    
    // Make binary executable on Unix-like systems
    if (process.platform !== 'win32') {
      fs.chmodSync(binaryPath, 0o755);
    }
    
    console.log('✓ vibectl binary downloaded successfully');
  } catch (error) {
    console.warn('⚠ Could not download pre-built binary:', error.message);
    console.log('');
    console.log('Attempting to build from source...');
    
    // Try to build from source as fallback
    try {
      const projectRoot = path.join(__dirname, '..', '..');
      
      // Check if Cargo.toml exists
      if (!fs.existsSync(path.join(projectRoot, 'Cargo.toml'))) {
        throw new Error('Not in a Rust project directory');
      }
      
      // Build with cargo
      console.log('Building vibectl with cargo...');
      execSync('cargo build --release', {
        cwd: projectRoot,
        stdio: 'inherit',
      });
      
      // Copy binary to npm bin directory
      const builtBinary = path.join(projectRoot, 'target', 'release', `vibectl${ext}`);
      if (fs.existsSync(builtBinary)) {
        fs.copyFileSync(builtBinary, binaryPath);
        console.log('✓ Built and installed vibectl from source');
      } else {
        throw new Error('Built binary not found');
      }
    } catch (buildError) {
      console.error('✗ Failed to build from source:', buildError.message);
      console.error('');
      console.error('Please install Rust and cargo, then run:');
      console.error('  cargo build --release');
      console.error('');
      console.error('Or download a pre-built binary from:');
      console.error(`  https://github.com/${GITHUB_REPO}/releases`);
      process.exit(1);
    }
  }
}

// Run installation
install().catch((err) => {
  console.error('Installation failed:', err);
  process.exit(1);
});
