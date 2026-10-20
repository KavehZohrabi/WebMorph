'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '../..');
const cargo = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const rustVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!rustVersion) throw new Error('Could not read the Rust package version from Cargo.toml.');
const npmScope = '@kavehzohrabi';

const manifests = [
  ['main npm package', path.join(root, 'package.json')],
  ...fs.readdirSync(path.join(root, 'npm/platforms')).map((platform) => [
    `${platform} npm package`,
    path.join(root, 'npm/platforms', platform, 'package.json'),
  ]),
];
const platformNames = manifests.slice(1).map(([, file]) => JSON.parse(fs.readFileSync(file, 'utf8')).name).sort();
const mainManifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
if (mainManifest.name !== `${npmScope}/webmorph`) {
  throw new Error(`Main npm package must be named ${npmScope}/webmorph.`);
}
const declaredOptionalNames = Object.keys(mainManifest.optionalDependencies || {}).sort();
if (JSON.stringify(platformNames) !== JSON.stringify(declaredOptionalNames)) {
  throw new Error('Main package optionalDependencies do not match the available platform package manifests.');
}

for (const [label, file] of manifests) {
  const manifest = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (manifest.version !== rustVersion) {
    throw new Error(`${label} version ${manifest.version} does not match Cargo version ${rustVersion}.`);
  }
  for (const version of Object.values(manifest.optionalDependencies || {})) {
    if (version !== rustVersion) {
      throw new Error(`${label} optional dependency version ${version} does not match Cargo version ${rustVersion}.`);
    }
  }
  if (label !== 'main npm package') {
    if (manifest.os?.length !== 1 || manifest.cpu?.length !== 1) {
      throw new Error(`${label} must declare exactly one supported operating system and CPU architecture.`);
    }
    const targetName = path.basename(path.dirname(file));
    const expectedName = `${npmScope}/webmorph-${targetName}`;
    if (manifest.name !== expectedName) {
      throw new Error(`${label} must be named ${expectedName} to match its ${targetName} directory and release scope.`);
    }
  }
}

const expectedTag = `v${rustVersion}`;
const suppliedTag = process.argv[2];
if (suppliedTag && suppliedTag !== expectedTag) {
  throw new Error(`Git tag ${suppliedTag} does not match package version ${expectedTag}.`);
}
process.stdout.write(`All npm manifests match Rust version ${rustVersion}${suppliedTag ? ` (${suppliedTag})` : ''}.\n`);
