'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { binaryFor, platformPackage } = require('../bin/webmorph.js');

test('maps all advertised operating system and architecture pairs', () => {
  const expected = [
    ['linux', 'x64', '@kaveh-zohrabi/webmorph-linux-x64'],
    ['linux', 'arm64', '@kaveh-zohrabi/webmorph-linux-arm64'],
    ['win32', 'x64', '@kaveh-zohrabi/webmorph-win32-x64'],
    ['win32', 'arm64', '@kaveh-zohrabi/webmorph-win32-arm64'],
    ['darwin', 'x64', '@kaveh-zohrabi/webmorph-darwin-x64'],
    ['darwin', 'arm64', '@kaveh-zohrabi/webmorph-darwin-arm64'],
  ];
  for (const [platform, arch, packageName] of expected) {
    assert.equal(platformPackage(platform, arch), packageName);
  }
});

test('reports unsupported operating systems and architectures clearly', () => {
  assert.throws(
    () => binaryFor('freebsd', 'x64'),
    /does not provide a binary for freebsd\/x64.*Supported targets:/,
  );
});

test('reports a missing optional binary package with a recovery action', () => {
  assert.throws(
    () => binaryFor('linux', 'x64', () => {
      const error = new Error('missing');
      error.code = 'MODULE_NOT_FOUND';
      throw error;
    }),
    /binary package.*not installed.*Reinstall/s,
  );
});

test('selects the platform executable path', () => {
  const requests = [];
  const result = binaryFor('win32', 'arm64', (request) => {
    requests.push(request);
    return 'C:\\node_modules\\webmorph.exe';
  });
  assert.equal(result, 'C:\\node_modules\\webmorph.exe');
  assert.deepEqual(requests, ['@kaveh-zohrabi/webmorph-win32-arm64/bin/webmorph.exe']);
});
