'use strict';

const fs = require('node:fs');
const path = require('node:path');

const [platform, executablePath, destinationPath] = process.argv.slice(2);
if (!platform || !executablePath || !destinationPath) {
  throw new Error('Usage: node npm/scripts/stage-platform.cjs <platform> <executable> <destination>');
}
const source = path.resolve(executablePath);
if (!fs.existsSync(source) || !fs.statSync(source).isFile()) {
  throw new Error(`Rust executable is missing: ${source}`);
}
const root = path.resolve(__dirname, '../..');
const template = path.join(root, 'npm/platforms', platform);
const destination = path.resolve(destinationPath);
if (!fs.existsSync(path.join(template, 'package.json'))) {
  throw new Error(`No npm platform package template exists for ${platform}.`);
}
fs.rmSync(destination, { recursive: true, force: true });
fs.mkdirSync(path.join(destination, 'bin'), { recursive: true });
fs.copyFileSync(path.join(template, 'package.json'), path.join(destination, 'package.json'));
fs.copyFileSync(path.join(root, 'LICENSE'), path.join(destination, 'LICENSE'));
const filename = platform.startsWith('win32-') ? 'webmorph.exe' : 'webmorph';
const target = path.join(destination, 'bin', filename);
fs.copyFileSync(source, target);
if (!platform.startsWith('win32-')) fs.chmodSync(target, 0o755);
process.stdout.write(`Staged ${platform} package at ${destination}.\n`);
