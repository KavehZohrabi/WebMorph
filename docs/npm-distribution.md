# npm distribution

WebMorph remains a Rust application. The npm packages distribute its native
executable and a minimal Node.js launcher; installation does not compile Rust
or run lifecycle scripts. npm obtains the matching binary as a regular optional
package dependency, without a separate post-install download.

## Package layout

- `package.json` exposes the `webmorph` command and optional dependencies for
  the six supported targets.
- `bin/webmorph.js` selects the platform package, starts its Rust executable
  with inherited stdin/stdout/stderr, and returns the executable's exit status.
- `npm/platforms/*/package.json` declares one native package per target with
  matching npm `os` and `cpu` metadata. Release jobs add the executable to
  these package templates before packing them.
- `npm/scripts` checks Rust/npm/tag version alignment, stages tested binaries,
  checks package contents, and rejects already-published package versions.

The six targets are Linux x64 and ARM64, Windows x64 and ARM64, macOS Intel
(x64), and macOS Apple Silicon (ARM64). Linux binaries are built on Ubuntu
22.04 and require glibc 2.35 or newer; Alpine/musl is not packaged. Packages
are produced on matching GitHub-hosted runners, so the release build does not
rely on cross-compilers.

## Local checks

Run the launcher tests after staging a locally built executable for the host:

```sh
cargo build --release --locked
node npm/scripts/install-local-binary.cjs linux-x64 target/release/webmorph
npm test
npm pack --dry-run --json | node npm/scripts/verify-pack.cjs
```

Replace `linux-x64` with the host target; on Windows, point the script at
`target/release/webmorph.exe`. The release workflow runs the equivalent checks
on all six supported targets. Linux additionally launches the TUI in a
pseudo-terminal and sends the quit key to check terminal startup and exit.

## First publication and release process

All seven 0.1.0 package names currently return 404 from the public npm
registry. npm requires a package to exist before a Trusted Publisher can be
configured, so the very first publication cannot use this repository's OIDC
publisher. Bootstrap 0.1.0 from the npm account after the maintainer has
explicitly approved publication:

1. Run `.github/workflows/npm-release.yml` with `workflow_dispatch` on `master`.
   This builds/tests all six native binaries on matching GitHub-hosted runners,
   packs and verifies the main package, and uploads seven package tarballs.
   `workflow_dispatch` does not run the publish job.
2. Download the artifacts from that successful run. Publish the six
   platform-specific tarballs first, one by one, with `npm publish <tarball>
   --access public`; publish the main `@kavehzohrabi/webmorph` tarball last.
   Do not rebuild or substitute a binary for another platform. Stop if any
   publish fails and inspect registry state before continuing; npm cannot
   atomically roll back a partial seven-package publication.
3. After all packages exist, add a Trusted Publisher separately in the npm
   settings for each of the seven packages, using the fields below.
4. Create the GitHub `npm-release` environment. Set repository variable
   `NPM_RELEASE_READY=true` only after all seven publisher entries and the
   environment are checked.
5. For later releases, bump the Rust and all npm package versions together,
   run the workflow with `workflow_dispatch` to verify the build, and then
   create the matching version tag only after explicit release authorization.
   The tag workflow preflights all seven versions and stops if any already
   exists, so the bootstrap version `v0.1.0` must not be run through its
   publish job after the manual bootstrap.

### Trusted Publisher settings

Set up one GitHub Actions Trusted Publisher entry on **each package**, after
that package has been bootstrapped:

| npm setting | Value |
| --- | --- |
| Provider | GitHub Actions |
| GitHub user or organization | `KavehZohrabi` |
| Repository | `WebMorph` |
| Workflow filename | `npm-release.yml` (filename only, not the `.github/workflows/` path) |
| Environment | `npm-release` |
| Allowed publisher action | Direct `npm publish` |

The seven package settings are `@kavehzohrabi/webmorph` and
`@kavehzohrabi/webmorph-{linux-x64,linux-arm64,win32-x64,win32-arm64,darwin-x64,darwin-arm64}`.
Choose the direct-publish permission because the release job invokes
`npm publish`; stage-only permission will not authorize this workflow.

The publish job has the least GitHub permissions it needs: `contents: read`
and `id-token: write`. It selects Node 24, installs npm 11.5.1, sets the npm
registry, disables package-manager caching, and publishes with provenance.
Current npm Trusted Publishing requirements are npm 11.5.1+ and Node 22.14.0+;
the release runtime satisfies both. See the [npm Trusted Publishers guide](https://docs.npmjs.com/trusted-publishers/).

The version preflight rejects a package/version already on the registry and
fails closed on registry responses other than 404. Platform artifacts are
published before the main package. `workflow_dispatch` is build-only and never
publishes. Do not use a tag to bootstrap 0.1.0: the tag path requires all
Trusted Publisher entries and the authorization variable, and its preflight
will reject a version already manually bootstrapped.

### Registry verification after publication

After the authorized bootstrap, verify each exact version with `npm view
<package>@0.1.0 version` for the seven names above. Then install the main
package in a fresh temporary directory on each supported OS/architecture and
check `webmorph --version` and `webmorph --help`. A platform package must only
be tested on its matching OS/CPU runner. The local checks before publication
cannot verify a registry installation while the package names are absent.

During preparation, `npm whoami` reported `kavehzohrabi`, matching the
requested personal scope. `npm view <package>@0.1.0 version` returned 404 for
all seven names, and no package write permission or published content can be
verified until bootstrap. `npm access list packages @kavehzohrabi` returned no
published package entries. Trusted Publisher configuration and post-publish
clean-install checks therefore remain pending.

## Troubleshooting

- **Unsupported operating system/architecture:** use a listed target or build
  the Rust project from source with Cargo.
- **Native package missing:** rerun `npm install --global
  @kavehzohrabi/webmorph` on the target machine and check that optional npm
  dependencies were not disabled with `--omit=optional` or `--no-optional`.
- **`spawn` or permission error:** reinstall the package. On Unix-like systems,
  the release package marks its executable as executable; avoid extracting the
  tarball with a tool that discards file permissions.
- **`npm publish` OIDC error:** verify the per-package Trusted Publisher
  settings, GitHub user and repository, workflow filename (`npm-release.yml`),
  environment name, GitHub `id-token: write`, and npm CLI support. Do not add a
  long-lived token to the repository as a workaround.
- **A platform version was published but the main version was not:** registry
  publication cannot be rolled back atomically. Do not rerun the same release
  blindly; inspect the package versions and resolve the partial release with
  the npm package owner.
