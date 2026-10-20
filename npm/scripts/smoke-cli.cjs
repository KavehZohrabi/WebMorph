'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const path = require('node:path');

const launcher = path.resolve('bin/webmorph.js');
const expectedVersion = `webmorph ${require('../../package.json').version}`;

for (const [flag, expected] of [
  ['--version', expectedVersion],
  ['--help', 'Usage: webmorph [OPTIONS] [URL]'],
]) {
  const result = spawnSync(process.execPath, [launcher, flag], {
    encoding: 'utf8',
    timeout: 15_000,
    windowsHide: true,
  });
  assert.ifError(result.error);
  assert.equal(result.status, 0, `${flag} exited ${result.status}: ${result.stderr}`);
  assert.ok(result.stdout.includes(expected), `${flag} output did not include ${JSON.stringify(expected)}: ${result.stdout}`);
  process.stdout.write(`${flag} passed through the npm launcher.\n`);
}
