---
name: release
description: Run the `/release [bumpsize]` process to prepare and publish a Volty release from commits since the latest release tag.
---

# Release

Use this skill when the user runs `/release [bumpsize]` or explicitly requests a release. Accept `patch`, `minor`, or `major` as the bump size; if it is missing or invalid, ask for a valid size before changing anything.

## Release process

1. **Check the repository state.**
   - Fetch tags and the default branch from `origin`. Identify the repository's default branch and require it to be checked out, clean, and synchronized with its remote tracking branch. Do not stash, overwrite, or include unrelated user changes. Stop if the branch has diverged, a merge/rebase is in progress, or the GitHub CLI or remote is unavailable.
   - Find the highest `vMAJOR.MINOR.PATCH` release tag and confirm it is an ancestor of `HEAD`. Stop if no release tag exists or the workspace version does not match that tag.
   - Confirm the next version's tag does not already exist.

2. **Review changes since the last release.**
   - Read `git log <last-tag>..HEAD`, including commit bodies and relevant diffs. Use the actual changes, not just commit subjects, to identify user-facing additions, changes, fixes, and removals.
   - Read `CHANGELOG.md`. Combine the relevant commits with existing `Unreleased` notes, remove duplicates, and keep only user-facing changes. If there are no unreleased changes to publish, stop instead of creating an empty release.

3. **Choose the next SemVer version.**
   - `patch`: increment the patch component.
   - `minor`: increment the minor component and reset patch to zero.
   - `major`: increment the major component and reset minor and patch to zero.
   - This follows SemVer for `0.x` versions too; a major bump from `0.x.y` produces `1.0.0`.

4. **Update release files.**
   - Update `[workspace.package].version` in the root `Cargo.toml` to the new version. Check for any other version references that must stay in sync, including `Cargo.lock`.
   - In `CHANGELOG.md`, replace the existing `## [Unreleased]` content with a dated section `## [<new-version>] - <YYYY-MM-DD>` containing the reviewed changes, then leave a fresh empty `## [Unreleased]` section above it. Preserve all prior release sections.
   - Regenerate `Cargo.lock` as needed, then review the complete diff to ensure it contains only the release version, lockfile, and changelog changes.

5. **Validate before publishing.**
   Run the same checks as `.github/workflows/release.yml`:

   ```powershell
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
   cargo test --workspace --all-features --locked
   cargo build --locked --release --target x86_64-pc-windows-msvc --package volty-tray
   ```

   Verify `cargo metadata --locked --no-deps --format-version 1` reports the new version for `volty-tray`, and that `v<new-version>` matches it. If any check fails, fix the release changes and rerun the checks; do not tag or push a failing release.

6. **Commit, tag, and push.**
   - Commit the reviewed release changes with message `Release v<new-version>`.
   - Create an annotated tag: `git tag -a v<new-version> -m "Release v<new-version>"`.
   - Push the release commit to the default branch and the tag to `origin` in one atomic push. Use explicit branch and tag refspecs; never force-push. If the atomic push fails, stop and report the error without retrying as separate pushes.
   - Verify the remote branch and tag point to the release commit. The `v*` tag triggers `.github/workflows/release.yml`, which builds and publishes the GitHub Release; do not create a second release manually.

Report the old and new versions, the changelog summary, the commit and tag, and the result of the push and release workflow.
