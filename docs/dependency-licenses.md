# Third-party dependency licenses

The package declares the WebMorph application license as MIT. The dependency
review used the locked Cargo package metadata from `Cargo.lock` (`cargo metadata
--locked --format-version 1`). At review time, the resolved graph contained 270
third-party package records and all supplied license expressions; no package had
missing license metadata.

The graph includes MIT, Apache-2.0, BSD, ISC, Zlib, Unlicense, Unicode-3.0,
CDLA-Permissive-2.0, and MPL-2.0 licensed packages, plus dual/multi-license
expressions. In particular, some HTML/CSS parser dependencies use MPL-2.0;
Unicode text processing dependencies carry Unicode-3.0 terms; and `webpki-roots`
identifies its root-certificate data under CDLA-Permissive-2.0. The target-
specific `r-efi` crate offers MIT, Apache-2.0, or LGPL-2.1-or-later. These are
license expressions from crate metadata, not a legal opinion about every
redistribution scenario.

The dependency graph includes transitive and platform-specific crates (and
optional-feature crates), not all of which are linked into every build. Before
redistributing binaries, retain notices and license texts required by the
selected dependencies and build features. For a release inventory, use the
exact committed lockfile and review the license files from each crate source;
re-run this review whenever dependencies or feature selections change.
