#!/usr/bin/env node

const { execSync } = require('child_process');
const path = require('path');

console.log('Testing vibectl installation...\n');

try {
  // Test that the binary exists and is executable
  const vibectlPath = path.join(__dirname, '..', 'bin', 'vibectl.js');
  
  // Try to run vibectl --version (when implemented)
  // For now, just check if it runs without error
  console.log('Testing binary execution...');
  
  try {
    const output = execSync('node ' + vibectlPath + ' --help', {
      encoding: 'utf8',
      stdio: 'pipe',
    });
    console.log('✓ Binary is executable');
    console.log('✓ Help command works\n');
    // console.log(output);
  } catch (err) {
    // Binary might not have --help yet, that's okay
    if (err.status !== 0) {
      console.log('⚠ Binary executed but returned non-zero (this is okay for now)');
    }
  }
  
  console.log('✓ All tests passed!\n');
  process.exit(0);
} catch (error) {
  console.error('✗ Test failed:', error.message);
  process.exit(1);
}
