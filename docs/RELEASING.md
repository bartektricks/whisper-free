# Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml),
which runs **only when you start it** from the Actions tab — there is no trigger that fires
on a push or a tag. Both platforms build in one run, because the Windows backend cannot be
cross-compiled from macOS. Both artifacts are **unsigned**: there is no Apple Developer
account and no Authenticode certificate. They are, separately, **update-signed** — see
below; the two are unrelated and only the second one exists here.

## The version lives in one place

`src-tauri/Cargo.toml`. `tauri.conf.json` deliberately has no `version` key, so Tauri falls
back to the crate version. `package.json` carries the same number for tidiness — it is
`private: true` and nothing reads it — and the run fails immediately if the two drift apart.

## The update signing key

In-app updates (decision 0006) need a minisign keypair. This is **not** Apple code signing
or Authenticode: it costs nothing, needs no developer account, and exists only so an
installed copy can tell that a download came from this repository. The plugin will not
accept an unsigned update, so it is not optional.

Set up once:

```sh
bun tauri signer generate -w ~/.tauri/whisper-free.key
```

- The public half (`~/.tauri/whisper-free.key.pub`, its **contents**, not the path) goes in
  `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`, and is committed.
- The private half and its password become the repository secrets
  `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`:

  ```sh
  gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/whisper-free.key
  gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # paste it, or an empty line for none
  ```

**Back the private key up somewhere you will still have it in two years.** It is the one
piece of this repository that cannot be regenerated: replace it and every installed copy
rejects every future update, because it is verifying against the public key baked into the
build it is already running. There is no recovery except telling people to reinstall.

The `resolve` job refuses to publish without the secret, in seconds, rather than after two
half-hour builds that would produce bundles nothing can install.

## Cutting a release

1. Make sure the commit you are releasing is green on `ci.yml`. The release workflow does
   not re-run clippy or the tests.
2. Bump the version in `src-tauri/Cargo.toml` and `package.json`.
3. `cd src-tauri && cargo check` — this updates the `whisper-free` entry in
   `Cargo.lock`. Commit and push all three files.
4. **Actions › Release › Run workflow**, pick the branch, and choose:

| Input | Default | What it does |
| --- | --- | --- |
| **What to publish** | `prerelease` | `prerelease` flags it as such so it never becomes the Latest download — **and so the updater never offers it**. `release` publishes it as Latest, which is what puts it in front of existing installs. `artifacts only (no release, no tag)` builds and attaches the bundles to the run — no tag, no release, nothing public. |
| **Draft** | off | Publishes as a draft, so nothing is visible until you press Publish on the release page — and **a draft reaches no one's updater** until you do. Combines with either release type; ignored for artifacts only. |
| **Which platforms** | `both` | `macOS only` / `Windows only` for a one-platform build. Handy for smoke-testing Windows without waiting on the Mac job. |
| **Tag** | blank | Blank means `v` + the version in `Cargo.toml`, which is almost always what you want. Anything you type must still match that version. |

**Choosing `release`, without `draft`, is now the act that ships to everyone.** That is the
only combination GitHub resolves as Latest, and the updater endpoint is
`releases/latest/download/latest.json`. The run summary says which of the two it is before
either build starts.

The tag does not need to exist beforehand — GitHub creates it at the commit you dispatched
from. The run's summary page states the plan (tag, commit, release type, platforms) before
either build starts.

Output: `WhisperFree_<version>_aarch64.dmg`, `WhisperFree.app.tar.gz`, and
`WhisperFree_<version>_x64-setup.exe`, with install instructions in the release body and
GitHub's generated commit notes underneath. Alongside them, for the updater: a `.sig` next
to each bundle and one `latest.json` listing both platforms.

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
- **`latest.json` is written the same racy way, but fails quietly.** Each job reads the
  copy already attached to the release, merges in the platform it built, and re-uploads. If
  both read before either writes, one platform vanishes from the manifest and its users are
  simply told no update is published for their computer. The `verify` job downloads the
  finished manifest and fails if either `darwin-aarch64` or `windows-x86_64` is missing;
  the fix is to re-run the build job for the platform that lost.
- **`APPLE_SIGNING_IDENTITY` is deliberately unset.** Tauri then skips `codesign` and the
  binary keeps the linker's ad-hoc signature, which is all an arm64 binary needs to launch.
  Setting it — even to `-` — risks sending Tauri down the notarization path.
- **A one-platform build still publishes a release** if you ask it to; the run summary warns
  you that the other platform's artifact will be missing.

## If Apple signing is ever added

Separate from the update key above, and still missing. macOS needs an Apple Developer
account, a `.p12` in secrets (`APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`,
`APPLE_TEAM_ID` — `tauri-action` reads all of them from the environment), plus an
entitlements file granting `com.apple.security.device.audio-input`, which does not exist
yet. Only then can the `xattr -cr` step disappear from the release notes — it is already
unnecessary for an *update*, which replaces the bundle without ever marking it quarantined,
but a fresh download from the Releases page still needs it.

It would also settle the open question in decision 0006: an ad-hoc signed app has no stable
identity for macOS to key a permission grant to, so **whether Accessibility and Microphone
survive an in-place update has to be checked on a real installed copy after every change to
the update path**. A Developer ID would make the answer "yes" by construction.
