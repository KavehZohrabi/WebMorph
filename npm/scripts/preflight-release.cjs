'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '../..');
const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
const platformPackages = fs.readdirSync(path.join(root, 'npm/platforms')).map((platform) => {
  const manifestPath = path.join(root, 'npm/platforms', platform, 'package.json');
  return JSON.parse(fs.readFileSync(manifestPath, 'utf8')).name;
});

async function main() {
  const packages = [...platformPackages, pkg.name];
  for (const name of packages) {
    const encodedName = name.replace('/', '%2f');
    const response = await fetch(`https://registry.npmjs.org/${encodedName}/${pkg.version}`);
    if (response.status === 200) {
      throw new Error(`${name}@${pkg.version} is already published. Refusing a duplicate or partial release.`);
    }
    if (response.status !== 404) {
      throw new Error(`npm registry preflight for ${name}@${pkg.version} failed with HTTP ${response.status}.`);
    }
  }
  process.stdout.write(`npm registry has no existing package versions for the ${packages.length}-package ${pkg.version} release.\n`);
}

main().catch((error) => {
  process.stderr.write(`Release preflight failed: ${error.message}\n`);
  process.exitCode = 1;
});
