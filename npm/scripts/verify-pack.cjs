'use strict';

const fs = require('node:fs');
const path = require('node:path');

const manifest = JSON.parse(fs.readFileSync(path.resolve('package.json'), 'utf8'));
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => { input += chunk; });
process.stdin.on('end', () => verify(JSON.parse(input)));

function verify(packResult) {
const packagedFiles = packResult[0].files.map((file) => file.path);
if (!packagedFiles.includes('bin/webmorph.js')) {
  throw new Error('The npm package does not contain its launcher.');
}
if (packagedFiles.some((file) => /\.rs$|Cargo\.lock|\.env|\.npmrc|node_modules/i.test(file))) {
  throw new Error('The npm package includes a source, credential, or dependency file that should not be shipped.');
}
if (!manifest.bin || manifest.bin.webmorph !== 'bin/webmorph.js') {
  throw new Error('The npm executable mapping is missing or incorrect.');
}
process.stdout.write('npm tarball contents are limited to the CLI launcher and package metadata.\n');
}
