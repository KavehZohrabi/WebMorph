'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { test } = require('node:test');

const root = path.resolve(__dirname, '..');
const launcher = path.join(root, 'bin', 'webmorph.js');

test('npm wrapper passes --help to the Rust CLI', () => {
  const result = spawnSync(process.execPath, [launcher, '--help'], { cwd: root, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Transform a website into an interactive terminal view/);
  assert.match(result.stdout, /--engine/);
});

test('npm wrapper passes --version through without rewriting it', () => {
  const result = spawnSync(process.execPath, [launcher, '--version'], { cwd: root, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), `webmorph ${require('../package.json').version}`);
});

test('npm wrapper preserves native CLI argument errors and exit codes', () => {
  const result = spawnSync(process.execPath, [launcher, '--engine', 'invalid'], { cwd: root, encoding: 'utf8' });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /invalid value 'invalid'/);
});
