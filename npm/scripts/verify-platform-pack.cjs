'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const [platform, executablePath] = process.argv.slice(2);
const target = {
  'linux-x64': { os: 'linux', cpu: 'x64', file: 'bin/webmorph' },
  'linux-arm64': { os: 'linux', cpu: 'arm64', file: 'bin/webmorph' },
  'win32-x64': { os: 'win32', cpu: 'x64', file: 'bin/webmorph.exe' },
  'win32-arm64': { os: 'win32', cpu: 'arm64', file: 'bin/webmorph.exe' },
  'darwin-x64': { os: 'darwin', cpu: 'x64', file: 'bin/webmorph' },
  'darwin-arm64': { os: 'darwin', cpu: 'arm64', file: 'bin/webmorph' },
}[platform];
if (!target || !executablePath) throw new Error('Expected a supported platform and built executable path.');

let json = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', (chunk) => { json += chunk; });
process.stdin.on('end', () => {
  try {
    verify(JSON.parse(json)[0]);
  } catch (error) {
    process.stderr.write(`Platform package verification failed: ${error.message}\n`);
    process.exitCode = 1;
  }
});

function verify(pack) {
  const root = path.resolve(__dirname, '../..');
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'npm/platforms', platform, 'package.json'), 'utf8'));
  if (manifest.os?.[0] !== target.os || manifest.cpu?.[0] !== target.cpu) {
    throw new Error(`${manifest.name} has incorrect OS/CPU metadata.`);
  }
  const packedFiles = new Set(pack.files.map((file) => file.path));
  if (!packedFiles.has('package.json') || !packedFiles.has('LICENSE') || !packedFiles.has(target.file)) {
    throw new Error(`${pack.filename} is missing package metadata, license, or ${target.file}.`);
  }
  const archivePath = path.resolve(`target/npm-tarballs/${pack.filename}`);
  const archive = zlib.gunzipSync(fs.readFileSync(archivePath));
  const packagedBinary = readTarEntry(archive, `package/${target.file}`);
  const builtBinary = fs.readFileSync(path.resolve(executablePath));
  const digest = (buffer) => crypto.createHash('sha256').update(buffer).digest('hex');
  if (digest(packagedBinary) !== digest(builtBinary)) {
    throw new Error(`The executable in ${pack.filename} differs from the tested build.`);
  }
  process.stdout.write(`${manifest.name}@${manifest.version} contains the tested ${platform} executable (sha256 ${digest(packagedBinary)}).\n`);
}

function readTarEntry(buffer, expectedPath) {
  let offset = 0;
  while (offset + 512 <= buffer.length) {
    const header = buffer.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const name = header.subarray(0, 100).toString('utf8').replace(/\0.*$/, '');
    const prefix = header.subarray(345, 500).toString('utf8').replace(/\0.*$/, '');
    const entryPath = prefix ? `${prefix}/${name}` : name;
    const sizeText = header.subarray(124, 136).toString('ascii').replace(/\0.*$/, '').trim();
    const size = Number.parseInt(sizeText || '0', 8);
    const bodyStart = offset + 512;
    if (entryPath === expectedPath) return buffer.subarray(bodyStart, bodyStart + size);
    offset = bodyStart + Math.ceil(size / 512) * 512;
  }
  throw new Error(`Tarball does not contain ${expectedPath}.`);
}
