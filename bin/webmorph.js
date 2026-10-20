#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');

const PLATFORMS = Object.freeze({
  'linux-x64': '@kaveh-zohrabi/webmorph-linux-x64',
  'linux-arm64': '@kaveh-zohrabi/webmorph-linux-arm64',
  'win32-x64': '@kaveh-zohrabi/webmorph-win32-x64',
  'win32-arm64': '@kaveh-zohrabi/webmorph-win32-arm64',
  'darwin-x64': '@kaveh-zohrabi/webmorph-darwin-x64',
  'darwin-arm64': '@kaveh-zohrabi/webmorph-darwin-arm64',
});

function platformPackage(platform = process.platform, architecture = process.arch) {
  return PLATFORMS[`${platform}-${architecture}`];
}

function binaryFor(platform = process.platform, architecture = process.arch, resolve = require.resolve) {
  const packageName = platformPackage(platform, architecture);
  if (!packageName) {
    const supported = Object.keys(PLATFORMS).map((key) => key.replace('-', '/')).join(', ');
    throw new Error(`WebMorph does not provide a binary for ${platform}/${architecture}. Supported targets: ${supported}.`);
  }

  const executable = platform === 'win32' ? 'webmorph.exe' : 'webmorph';
  try {
    return resolve(`${packageName}/bin/${executable}`);
  } catch (error) {
    if (error && (error.code === 'MODULE_NOT_FOUND' || error.code === 'ERR_PACKAGE_PATH_NOT_EXPORTED')) {
      throw new Error(
        `The WebMorph binary package for ${platform}/${architecture} is not installed. ` +
        'Reinstall @kaveh-zohrabi/webmorph for this platform, or download a binary from the project releases.',
      );
    }
    throw error;
  }
}

function run(args = process.argv.slice(2), platform = process.platform, architecture = process.arch) {
  let binary;
  try {
    binary = binaryFor(platform, architecture);
  } catch (error) {
    process.stderr.write(`webmorph: ${error.message}\n`);
    process.exitCode = 1;
    return;
  }

  const child = spawn(binary, args, {
    cwd: process.cwd(),
    env: process.env,
    stdio: 'inherit',
    windowsHide: false,
  });

  child.once('error', (error) => {
    process.stderr.write(`webmorph: could not start the native executable: ${error.message}\n`);
    process.exitCode = 1;
  });
  child.once('close', (code, signal) => {
    if (signal) {
      // Keep conventional signal exit codes when Node remains alive long enough
      // to observe the child's termination.
      process.exitCode = signal === 'SIGINT' ? 130 : signal === 'SIGTERM' ? 143 : 1;
    } else if (code !== null) {
      process.exitCode = code;
    }
  });
}

if (require.main === module) run();

module.exports = { binaryFor, platformPackage, run };
