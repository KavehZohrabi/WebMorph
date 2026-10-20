'use strict';

const target = process.argv[2];
const expected = {
  'linux-x64': ['linux', 'x64'],
  'linux-arm64': ['linux', 'arm64'],
  'win32-x64': ['win32', 'x64'],
  'win32-arm64': ['win32', 'arm64'],
  'darwin-x64': ['darwin', 'x64'],
  'darwin-arm64': ['darwin', 'arm64'],
}[target];
if (!expected) throw new Error(`Unknown release target: ${target}`);
if (process.platform !== expected[0] || process.arch !== expected[1]) {
  throw new Error(`Runner is ${process.platform}/${process.arch}, but ${target} requires ${expected[0]}/${expected[1]}.`);
}
process.stdout.write(`Confirmed native runner ${target}.\n`);
