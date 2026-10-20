'use strict';

const fs = require('node:fs');
const path = require('node:path');

const [platform, executablePath] = process.argv.slice(2);
if (!platform || !executablePath) {
  throw new Error('Usage: node npm/scripts/install-local-binary.cjs <platform> <executable>');
}
const packageName = `@kavehzohrabi/webmorph-${platform}`;
const destination = path.join(process.cwd(), 'node_modules', ...packageName.split('/'), 'bin');
fs.mkdirSync(destination, { recursive: true });
const filename = platform.startsWith('win32-') ? 'webmorph.exe' : 'webmorph';
const target = path.join(destination, filename);
fs.copyFileSync(path.resolve(executablePath), target);
if (!platform.startsWith('win32-')) fs.chmodSync(target, 0o755);
process.stdout.write(`Installed local ${platform} test binary.\n`);
