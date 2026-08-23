# Releasing

Releases are built by [`.github/workflows/release.yml`](../.github/workflows/release.yml),
which runs **only when you start it** from the Actions tab — there is no trigger that fires
on a push or a tag. Both platforms build in one run, because the Windows backend cannot be
cross-compiled from macOS.

## Steps

1. Make sure the commit is green on `ci.yml`. The release workflow does not re-run clippy
   or the tests.
2. Bump the version in `src-tauri/Cargo.toml` and `package.json`. They must match, and the
   run fails immediately if they drift. `tauri.conf.json` has no `version` key on purpose —
   Tauri falls back to the crate version.
3. `cd src-tauri && cargo check` to update `Cargo.lock`. Commit and push all three files.
4. **Actions › Release › Run workflow**, pick the branch, and choose:

| Input | Default | What it does |
| --- | --- | --- |
| **What to publish** | `prerelease` | `prerelease` never becomes the Latest download, so **the updater never offers it**. `release` publishes as Latest, which is what reaches existing installs. `artifacts only` builds and attaches bundles to the run — no tag, no release. |
| **Draft** | off | Nothing is visible, and **no updater sees it**, until you press Publish. |
| **Which platforms** | `both` | One-platform builds are for smoke-testing without waiting on the other job. |
| **Tag** | blank | Blank means `v` + the version in `Cargo.toml`. Anything you type must match it. |

**`release` without `draft` is the act that ships to everyone** — that is the only
combination GitHub resolves as Latest, and the updater endpoint is
`releases/latest/download/latest.json`. The run summary states the plan before either
build starts.

Budget **20–30 minutes per platform**; release builds are cold every time.

Output: `WhisperFree_<version>_aarch64.dmg`, `WhisperFree.app.tar.gz`, and
`WhisperFree_<version>_x64-setup.exe`, plus a `.sig` beside each bundle and one
`latest.json` listing both platforms.

## Secrets the workflow needs

| Secret | Why |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Signs the update artifacts. The plugin refuses an unsigned update, so without it a release reaches nobody. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | |
| `APPLE_CERTIFICATE` | Apple code signing, base64 `.p12`. Gives the macOS build an identity stable across versions, which is what lets macOS keep Accessibility and Microphone granted through an update. |
| `APPLE_CERTIFICATE_PASSWORD` | |

The `resolve` job refuses to start without them, in seconds, rather than after two
half-hour builds that would produce bundles nothing can install or that would strip every
user's permissions. **None of the four can be regenerated** — replacing the update key
makes every installed copy reject every future update, and replacing the certificate costs
every installed copy its permissions once. See
[decision 0006](decisions/0006-in-app-updates.md).

First-time setup: `bun tauri signer generate -w ~/.tauri/whisper-free.key` for the update
key, whose public half goes in `tauri.conf.json` under `plugins.updater.pubkey`, and
`scripts/make-signing-cert.sh` for the certificate.

## Building locally

`createUpdaterArtifacts` is on, so `bun run tauri build` stops with *"A public key has been
found, but no private key"* without the update key. Copy `.env.example` to `.env`, which is
gitignored, and fill in the three values it documents.

`APPLE_SIGNING_IDENTITY` names the certificate in your login keychain — the certificate
material itself is only needed on CI, which starts blank. Omitting it still builds; it just
leaves the ad-hoc signature, which costs you Accessibility on every rebuild.

The `tauri` script is `dotenv -- tauri` because the variables have to be in the *real*
process environment before the CLI starts. The CLI is a native addon that reads its
environment through Rust's `std::env::var`; bun's own `.env` handling populates only the JS
`process.env`, which that addon cannot see. A missing `.env` is a no-op.

## Things that can bite

- **The bundler is configured in two files**, `tauri.conf.json` (`app`, `dmg`) and
  `tauri.windows.conf.json` (`nsis`), which Tauri merges. A CLI flag would override both.
- **Whichever platform finishes first creates the release**, and the other uploads into it.
  In the unlikely event both land at once, one job fails with `already_exists` — re-run it.
- **`latest.json` is written the same racy way, but fails quietly.** Each job merges its
  platform into the copy already attached. If both read before either writes, one platform
  vanishes from the manifest and its users are told no update is published for their
  computer. The `verify` job catches this; the fix is to re-run the build job that lost.
- **Downloads are not notarized**, so a fresh `.dmg` from the Releases page still needs
  `xattr -cr`. An *update* does not — it replaces the bundle without marking it
  quarantined.
- **A one-platform build still publishes a release** if you ask it to.
