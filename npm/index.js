#!/usr/bin/env node

// This file is here for npm package structure
// The actual CLI is in bin/vibectl.js

module.exports = {
  version: require('./package.json').version,
  name: 'vibectl',
  description: 'Autonomous coding agent CLI',
};
