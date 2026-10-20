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

## Release process

The `.github/workflows/npm-release.yml` workflow builds/tests native packages
for every target, checks that all package versions and the `vX.Y.Z` tag match
the Rust crate, verifies the root tarball, and checks the registry for any
already-published version before publishing. Platform packages publish first;
the main package publishes only after all six succeed. A failure stops the
release and is reported in the Actions job summary. npm does not support an
atomic multi-package publish, so a partial platform publish requires manual
recovery before retrying that version.

The workflow requests only `contents: read` and `id-token: write` for the
publish job. It uses npm Trusted Publishing and provenance, not a long-lived
token. Before the first release, an npm maintainer must:

1. Confirm they control the `@kavehzohrabi` npm scope and can publish public
   packages in it.
2. Configure Trusted Publisher records for the repository and the exact
   workflow file `.github/workflows/npm-release.yml` on the main and each of
   the six platform packages. Set the trusted environment name to
   `npm-release`.
3. Create/protect the `npm-release` GitHub environment as required by the
   project's release policy.
4. Set the repository Actions variable `NPM_RELEASE_READY` to `true` only
   after verifying scope ownership and all seven Trusted Publisher entries.
   A tagged workflow fails with a clear message while this variable is absent.
5. Run the workflow with `workflow_dispatch` and confirm all package builds
   and checks pass. That path never publishes. Only then review and create a
   version tag to request publication.

The npm registry returned 404 for both `webmorph` and
`@kavehzohrabi/webmorph` during implementation. A 404 means the exact package
version/name is not currently present in the public registry; it does not
prove ownership of the npm scope. No npm publisher identity was authenticated
in the implementation environment, so scope ownership and OIDC authorization
remain release setup gates.

## Troubleshooting

- **Unsupported operating system/architecture:** use a listed target or build
  the Rust project from source with Cargo.
- **Native package missing:** rerun `npm install --global
  @kavehzohrabi/webmorph` on the target machine and check that optional npm
  dependencies were not disabled with `--omit=optional` or `--no-optional`.
- **`spawn` or permission error:** reinstall the package. On Unix-like systems,
  the release package marks its executable as executable; avoid extracting the
  tarball with a tool that discards file permissions.
- **`npm publish` OIDC error:** verify Trusted Publisher settings, repository
  name, workflow filename, npm environment, GitHub `id-token: write`, and npm
  CLI support. Do not add a long-lived token to the repository as a workaround.
- **A platform version was published but the main version was not:** registry
  publication cannot be rolled back atomically. Do not rerun the same release
  blindly; inspect the package versions and resolve the partial release with
  the npm package owner.
