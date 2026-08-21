# 0006 — In-app updates

**Status:** accepted
**Date:** 2026-08-20
**Applies to:** the updater, the release workflow, and the privacy invariant in CLAUDE.md

Releases are cut by hand from the Actions tab and land on a GitHub Releases page nobody
revisits. A user who installed 0.1.0 stays on 0.1.0 — not because updating is hard, but
because nothing ever tells them there is anything to update to. Decision 0002 parked
Tauri's updater plugin as "real future value, but a product change rather than porting
work". This is that product change.

## The constraint that shaped it

CLAUDE.md said, before this decision:

> No telemetry, and no network call other than an explicitly requested model download.

An update check is a network call, and a *useful* update check is one nobody asks for. The
two cannot both be true, so the invariant is amended rather than quietly broken:

> No telemetry, and no network call other than an explicitly requested model download or an
> update check the user has switched on.

`Settings::check_for_updates` is **off by default**, and `update::watch` reads it on every
tick rather than capturing it once. With the setting off the watchdog takes two locks,
finds nothing to do, and goes back to sleep — no request is made, ever, until a user either
presses the button or ticks the box. That is what keeps the amended sentence honest, and it
is why the box is not on by default "because everyone wants updates".

## Options evaluated

| | Signing needed | Reaches existing installs | Code we write | Failure mode |
|---|---|---|---|---|
| **A. `tauri-plugin-updater`** | update key only | yes, in place | ~300 lines | a bad update replaces a working app |
| B. Notify + open the releases page | none | only if the user re-downloads | ~80 lines | user repeats `xattr -cr` every time |
| C. Homebrew cask | Apple Developer ID | `brew upgrade` only | cask formula | macOS only |

**A.** The signing the plugin requires is *update* signing — a minisign keypair from
`tauri signer generate` — and is unrelated to Apple code signing or Authenticode. It costs
nothing, needs no developer account, and is not optional: the plugin refuses an unverifiable
download. That distinction is worth stating plainly, because "we cannot sign" was the
reason to think this was blocked, and it turned out not to apply.

**B** was tempting for exactly one run of the argument: no key to manage and nothing that
can go wrong on the user's disk. It loses because it does not actually update anything — it
hands the user back the manual path, including the `xattr -cr` step, every time.

**C** is macOS-only, and the app ships on two platforms.

## What the choice buys, and what it costs

The macOS install path is the interesting one. The plugin downloads the `.app.tar.gz`,
extracts it in-process with `tar`/`flate2`, renames the current bundle aside and moves the
new one in. Extracting that way means **the replacement bundle carries no
`com.apple.quarantine` attribute**, because quarantine is applied by the frameworks
browsers use, not by writing files. So an updated app does not need `xattr -cr` and does
not show the "damaged" dialog — the update path is strictly nicer than the manual one it
replaces.

The cost sits next to it. The app is ad-hoc signed (`docs/RELEASING.md`: `APPLE_SIGNING_IDENTITY`
is deliberately unset), so macOS TCC has no stable Developer ID to key a permission grant
to. **Whether Accessibility and Microphone survive an in-place replacement has to be
measured on a real installed copy, not assumed** — and for an app whose whole job is
pasting into other applications, silently losing Accessibility is the worst outcome
available. The verification list in `docs/RELEASING.md` makes that the check that matters;
if it reproduces, the panel gains a post-update prompt built on the `can_insert_text` and
`request_insert_permission` commands that already exist.

## Decisions inside the decision

**Rust-driven, not JS-driven.** `@tauri-apps/plugin-updater` was installed by
`tauri add updater` and then removed, along with the capability the CLI wrote. Two reasons:
the tray has to be able to start a check when no window exists, and a download has to
survive the settings window being closed — which it is, most of the time, in a menu-bar
app. `update/plugin.rs` is the only file that may name `tauri_plugin_updater`, the same
boundary `asr/parakeet.rs` and `refine/onnx.rs` keep.

**Stable only.** The endpoint is `releases/latest/download/latest.json`, and GitHub resolves
`latest` to neither a prerelease nor a draft. Since the workflow's default is `prerelease`,
that means the default dispatch is invisible to the updater and *choosing `release` is the
act that ships to users* — which is the right shape for a hand-cut release. A beta channel
would need a second manifest at a fixed tag the workflow force-updates, and a second thing
that can go stale; there is no user asking for one.

**Advisory, like refinement.** Nothing in `update/` touches `AppState`. The status is its
own struct on `AppContext` with its own event, so a failed check is a line in the Updates
panel rather than an error banner over an app that still transcribes perfectly well. A
scheduled check that finds nothing, or fails, says nothing at all: an unprompted "Up to
date" is noise, and an unprompted "could not reach the update server" reports a problem
with a feature the user is not using.

**The app never restarts itself.** A download ends at `ReadyToRestart` and waits. A
dictation app that disappears mid-sentence is worse than an old one. `update::install` also
refuses outright while `ctx.audio.is_recording()` or the state machine is mid-pipeline —
checked against the real thing rather than `AppState`, for the same reason `dictation`
does.

**Nothing may run on the main thread.** When the app cannot be overwritten in place, the
plugin's macOS path raises an admin prompt through `run_on_main_thread` and blocks waiting
for the answer. Called from the tray menu handler — which *is* the main thread on macOS —
that deadlocks, exactly the way registering a shortcut from the hotkey handler does
(decision 0003). Both public verbs return immediately and work on the async runtime.

**The manifest's `notes` field is not shown.** `tauri-action` fills it with the workflow's
`releaseBody`, which for this project is install instructions rather than a changelog. The
panel links to the release page instead, through the `opener` plugin from Rust — the
settings window holds no `opener` capability and does not need one.

## Consequences

- A new secret pair, `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
  and a public key committed in `tauri.conf.json`. **Losing the private key ends the update
  path**: no later build can produce a signature the installed copies accept, and every user
  reinstalls by hand. The `resolve` job fails in seconds when the secret is missing rather
  than after two half-hour builds.
- `uploadUpdaterJson` writes `latest.json` by reading the copy already on the release,
  merging in the platform it built, and re-uploading. Two matrix jobs can both read before
  either writes, losing one platform — silently, since the loser's users simply hear that
  nothing is published for their computer. The `verify` job checks for both platform keys
  rather than hoping.
- `Settings` now carries four independent booleans, over the `struct_excessive_bools`
  threshold. Allowed with a comment: they are four checkboxes, and `settings.json` staying
  a flat mirror of the panel is the only debugging tool that file has.
