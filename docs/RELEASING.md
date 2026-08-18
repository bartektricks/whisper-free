# Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml),
which runs **only when you start it** from the Actions tab — there is no trigger that fires
on a push or a tag. Both platforms build in one run, because the Windows backend cannot be
cross-compiled from macOS. Both artifacts are **unsigned**: there is no Apple Developer
account and no Authenticode certificate.

## The version lives in one place

`src-tauri/Cargo.toml`. `tauri.conf.json` deliberately has no `version` key, so Tauri falls
back to the crate version. `package.json` carries the same number for tidiness — it is
`private: true` and nothing reads it — and the run fails immediately if the two drift apart.

## Cutting a release

1. Make sure the commit you are releasing is green on `ci.yml`. The release workflow does
   not re-run clippy or the tests.
2. Bump the version in `src-tauri/Cargo.toml` and `package.json`.
3. `cd src-tauri && cargo check` — this updates the `local-dictation` entry in
   `Cargo.lock`. Commit and push all three files.
4. **Actions › Release › Run workflow**, pick the branch, and choose:

| Input | Default | What it does |
| --- | --- | --- |
| **What to publish** | `prerelease` | `prerelease` flags it as such so it never becomes the Latest download. `release` publishes it as Latest. `artifacts only (no release, no tag)` builds and attaches the bundles to the run — no tag, no release, nothing public. |
| **Draft** | off | Publishes as a draft, so nothing is visible until you press Publish on the release page. Combines with either release type; ignored for artifacts only. |
| **Which platforms** | `both` | `macOS only` / `Windows only` for a one-platform build. Handy for smoke-testing Windows without waiting on the Mac job. |
| **Tag** | blank | Blank means `v` + the version in `Cargo.toml`, which is almost always what you want. Anything you type must still match that version. |

The tag does not need to exist beforehand — GitHub creates it at the commit you dispatched
from. The run's summary page states the plan (tag, commit, release type, platforms) before
either build starts.

Output: `LocalDictation_<version>_aarch64.dmg`, `LocalDictation.app.tar.gz`, and
`LocalDictation_<version>_x64-setup.exe`, with install instructions in the release body and
GitHub's generated commit notes underneath.

Budget **20–30 minutes per platform**. Release builds are cold every time: `lto = true`
with `codegen-units = 1`, plus `ort-sys` downloading a prebuilt ONNX Runtime. Caches are
scoped by ref and are not usefully reusable here.

## Trying a build without releasing

Choose **artifacts only (no release, no tag)**. The bundles land as workflow artifacts on
the run, and nothing on the Releases page is created or touched. This is the way to test
the Windows installer before committing to a tag.

## Things that can bite

- **`Cargo.lock` is load-bearing.** `windows-core` is pinned to 0.61 there and nowhere
  else; at 0.62 `cpal` stops compiling on Windows. The workflow runs `cargo fetch --locked`
  before building, so a stale lockfile fails immediately instead of being silently updated.
- **Don't pass `--bundles` to the build.** Targets come from `tauri.macos.conf.json`
  (`app`, `dmg`) and `tauri.windows.conf.json` (`nsis`), which Tauri merges automatically;
  a CLI flag would override both.
- **Whichever platform finishes first creates the release**, and the other uploads into it.
  `tauri-action` does not retry that find-or-create, so in the very unlikely event both
  land in the same instant, one job fails with `already_exists`. Re-run the failed job — it
  will find the release the other one made.
- **`APPLE_SIGNING_IDENTITY` is deliberately unset.** Tauri then skips `codesign` and the
  binary keeps the linker's ad-hoc signature, which is all an arm64 binary needs to launch.
  Setting it — even to `-` — risks sending Tauri down the notarization path.
- **A one-platform build still publishes a release** if you ask it to; the run summary warns
  you that the other platform's artifact will be missing.

## If signing is ever added

macOS needs an Apple Developer account, a `.p12` in secrets (`APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`,
`APPLE_TEAM_ID` — `tauri-action` reads all of them from the environment), plus an
entitlements file granting `com.apple.security.device.audio-input`, which does not exist
yet. Only then can the `xattr -cr` step disappear from the release notes.
