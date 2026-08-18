# Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml)
on both platforms at once, because the Windows backend cannot be cross-compiled from
macOS. Everything is published as a **prerelease** for now, and both artifacts are
**unsigned** — there is no Apple Developer account and no Authenticode certificate.

## The version lives in one place

`src-tauri/Cargo.toml`. `tauri.conf.json` deliberately has no `version` key, so Tauri
falls back to the crate version. `package.json` carries the same number for tidiness — it
is `private: true` and nothing reads it — and the workflow fails the release if the two
drift apart, or if either disagrees with the tag.

## Cutting a release

1. Make sure the commit you are about to tag is green on `ci.yml`. Tag pushes do **not**
   trigger it, and the release workflow does not re-run clippy or the tests.
2. Bump the version in `src-tauri/Cargo.toml` and `package.json`.
3. `cd src-tauri && cargo check` — this updates the `local-dictation` entry in
   `Cargo.lock`. Commit all three files.
4. Tag and push:

   ```sh
   git tag v0.2.0
   git push origin master --tags
   ```

The workflow verifies the tag against the manifests, then builds
`LocalDictation_0.2.0_aarch64.dmg` and `LocalDictation_0.2.0_x64-setup.exe` and publishes
them as a prerelease, with install instructions in the body and GitHub's generated commit
notes underneath.

Budget **20–30 minutes per platform**. Release builds are cold every time: `lto = true`
with `codegen-units = 1`, plus `ort-sys` downloading a prebuilt ONNX Runtime. Caches are
not reusable here, since GitHub scopes them by ref and one tag cannot read another tag's
cache.

## Building without releasing

Run the workflow from the Actions tab (`workflow_dispatch`). It builds the same bundles on
both runners and uploads them as workflow artifacts without creating or touching a
release — the way to try a Windows build before committing to a tag.

## Things that can bite

- **`Cargo.lock` is load-bearing.** `windows-core` is pinned to 0.61 there and nowhere
  else; at 0.62 `cpal` stops compiling on Windows. The workflow runs `cargo fetch --locked`
  before building so a stale lockfile fails immediately instead of being silently updated.
- **Don't pass `--bundles` to the build.** Targets come from `tauri.macos.conf.json`
  (`app`, `dmg`) and `tauri.windows.conf.json` (`nsis`), which Tauri merges automatically;
  a CLI flag would override both.
- **Whichever platform finishes first creates the release**, and the other uploads into
  it. `tauri-action` does not retry that find-or-create, so in the (very unlikely) event
  both land in the same instant, one job fails with `already_exists`. Re-run the failed
  job — it will find the release the other one made. This is why the release is not
  pre-created: an empty prerelease would otherwise sit on the page for half an hour.
- **`APPLE_SIGNING_IDENTITY` is deliberately unset.** Tauri then skips `codesign` and the
  binary keeps the linker's ad-hoc signature, which is all an arm64 binary needs to launch.
  Setting it — even to `-` — risks sending Tauri down the notarization path.

## If signing is ever added

macOS needs an Apple Developer account, a `.p12` in secrets
(`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` — `tauri-action` reads all of them from the
environment), plus an entitlements file granting `com.apple.security.device.audio-input`,
which does not exist yet. Only then can the `xattr -cr` step disappear from the release
notes.
