# Contributing

1. Fork and create a feature branch
2. cargo test --workspace must pass
3. cargo fmt --all and cargo clippy --workspace -- -D warnings must be clean
4. Open a PR - CI runs fmt, clippy, tests, and a RustSec security audit automatically


Listfile improvements and new deep-search plug-ins are especially welcome.

## Releasing

For maintainers with publish rights on crates.io. Users install from the
Releases page or crates.io and never need this.

```
./release.sh 0.7.0            # bump the workspace version, commit, tag, publish the library crates
./release.sh 0.7.0 --dry-run  # bump and tag only, no publishing
git push && git push origin v0.7.0
```

The script publishes rustydemon-blp2, rustydemon-gr2, rustydemon-mpk,
rustydemon-messiah and rustydemon-lib, in that order, so crates.io never
sees a crate whose dependency is not there yet. The pushed tag starts the
release workflow, which builds the GUI and CLI for Linux, macOS and Windows,
attaches them with SHA-256 sums and a build attestation, and publishes the
GitHub release. Installers are unsigned for now.
