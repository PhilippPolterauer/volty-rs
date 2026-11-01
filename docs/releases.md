# Releases and changelog

User-facing changes are tracked in the root
[CHANGELOG.md](https://github.com/PhilippPolterauer/volty-rs/blob/main/CHANGELOG.md).

To prepare a release:

1. Bump the version in `[workspace.package]` in the root `Cargo.toml`.
2. Move the `[Unreleased]` changelog entries under a heading with that version
   and release date.
3. Commit and push a matching tag such as `v0.4.0`.

The Windows release workflow checks that the tag matches the `volty-tray`
package version, runs formatting, Clippy, and tests, then publishes a standalone
`x86_64-pc-windows-msvc` executable and SHA-256 checksum to the GitHub Release.
No installer or sidecar runtime files are required.
