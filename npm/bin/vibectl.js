#!/usr/bin/env node

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

// Determine binary name based on platform
function getBinaryPath() {
  const platform = process.platform;
  const arch = process.arch;
  
  let binaryName = 'vibectl';
  
  if (platform === 'win32') {
    binaryName += '.exe';
  }
  
  // Check multiple possible locations
  const possiblePaths = [
    // Installed via npm (in node_modules)
    path.join(__dirname, '..', 'bin', binaryName),
    // Development mode (built in project)
    path.join(__dirname, '..', '..', 'target', 'release', binaryName),
    // Global installation
    path.join(__dirname, '..', '..', '..', 'bin', binaryName),
  ];
  
  for (const binPath of possiblePaths) {
    if (fs.existsSync(binPath)) {
      return binPath;
    }
  }
  
  // Fallback: assume it's in PATH
  return binaryName;
}

// Get binary path
const binaryPath = getBinaryPath();

// Check if binary exists
if (!fs.existsSync(binaryPath) && !binaryPath.endsWith('vibectl') && !binaryPath.endsWith('vibectl.exe')) {
  console.error('Error: vibectl binary not found!');
  console.error('Expected location:', binaryPath);
  console.error('');
  console.error('Installation may have failed. Try:');
  console.error('  npm install -g vibectl --force');
  console.error('');
  console.error('Or build from source:');
  console.error('  cargo build --release');
  process.exit(1);
}

// Forward all arguments to the Rust binary
const args = process.argv.slice(2);

const child = spawn(binaryPath, args, {
  stdio: 'inherit',
  shell: false,
});

child.on('error', (err) => {
  if (err.code === 'ENOENT') {
    console.error('Error: vibectl binary not found in PATH');
    console.error('Binary path attempted:', binaryPath);
    console.error('');
    console.error('Please ensure vibectl is properly installed or build it with:');
    console.error('  cargo build --release');
  } else {
    console.error('Error executing vibectl:', err.message);
  }
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exit(code || 0);
  }
});
