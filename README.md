# WhisperFree

Fully local dictation for macOS and Windows. Press a hotkey, speak, and the text
appears where your cursor is. Nothing you say leaves your machine: no cloud API,
no account, no telemetry.

## Install

Take the installer for your platform from the
[Releases page](https://github.com/bartektricks/whisper-free/releases):

| Platform | File |
| --- | --- |
| macOS 13 or later, Apple Silicon | `WhisperFree_<version>_aarch64.dmg` |
| Windows 10/11, 64-bit | `WhisperFree_<version>_x64-setup.exe` |

Neither build is notarised or Authenticode signed, so each system warns you once.

On macOS, drag the app to Applications and then clear the quarantine flag, or it
is reported as damaged:

```sh
xattr -cr /Applications/WhisperFree.app
```

On Windows, SmartScreen blocks the installer until you choose **More info**, then
**Run anyway**. It installs for the current user, so no admin rights are needed.

The installers are around 10 MB because no speech model is bundled. You choose
one and download it from inside the app.

## Or build it yourself

You need [Rust](https://rustup.rs) and [Bun](https://bun.sh), plus the Xcode
command line tools on macOS or the MSVC build tools on Windows.

```sh
bun install
bun run tauri dev      # run it from source
bun run tauri build    # build your own .dmg or -setup.exe
```

`bun run dev` on its own starts Vite without the Rust backend, where every
command fails. `bun run tauri dev` is the real app.

## Using it

There is no Dock icon by design: the app lives in the macOS menu bar, or the
Windows notification area.

- The default hotkey is **⌥Space** on macOS and **Ctrl+Alt+Space** on Windows,
  where Alt+Space belongs to the system window menu.
- Hold it while you speak, or switch to toggle mode in Settings.
- It can also be a two-step chord, ⌘K then K, written the way VS Code writes
  them. If the second key does not follow within 800 ms, the first is passed on
  to the app you were using, so ⌘K keeps working everywhere else.
- **Escape** abandons a dictation. The recording is dropped, and a transcription
  already running is discarded rather than pasted.
- A floating indicator shows what the app is doing. It never takes focus, its
  corner is yours to choose, and it can be switched off.

The first launch walks through microphone access, permission to paste into other
apps, and the model download. Nothing in it is mandatory, and Settings › General
has **Run setup again**.

Also in Settings: a dictionary of word replacements for names and jargon, muting
the rest of the machine while the microphone is open, keeping the transcription
on the clipboard, an optional local history of what you dictated, start at login,
and an optional daily update check.

## Models

Nothing is bundled and nothing downloads until you ask. Every file is verified
against a pinned SHA-256. Pick a model in Settings › Models.

| Model | Size | Language |
| --- | --- | --- |
| NVIDIA Parakeet TDT 0.6B v3 (default) | 671 MB | Detects one of 25 European languages by itself |
| NVIDIA Canary 1B v2 | 1.03 GB | More accurate and slower; you pin the language |
| NVIDIA Canary 180M Flash | 213 MB | The quickest; English, German or French, pinned |

Measured on an M1 Pro, Parakeet transcribes about 23× faster than real time and
holds ~1.4 GB while loaded. The execution provider is CPU on purpose: CoreML
measured 2.9× slower and used 4.5× the memory on this graph.

A Canary model given the wrong language does not fail visibly, it translates into
the language you pinned, so the app refuses a language the chosen model cannot
honour rather than quietly ignoring it.

### Optional cleanup

Settings › Cleanup runs each transcription past
["S1-mini" by "Superwhisper"](https://huggingface.co/superwhisper/s1-mini) (412 MB), which
writes what you dictated the way you would have typed it: fillers dropped, false starts
resolved to whatever you settled on, and spoken numbers, dates, times and email addresses
written out. "so um i need to like send the the report by uh friday no wait make that
thursday" becomes "I need to send the report by Thursday."

It is off by default, because it costs about half a second per dictation and about
400 MB of memory on top of the speech model. Two settings shape it: **how much to change**
(full cleanup, or light touch for punctuation and misheard words only) and **style**, from
casual through to formal.

A cleanup is only ever a suggestion. The result is measured against what you actually said,
and if a word appears that you never spoke, the whole thing is thrown away and your own
words are pasted instead. That covers the model answering you, translating you, or guessing
at a name it did not know. Your dictionary is applied afterwards either way.

**English only.** If you have pinned a different language in Settings › Speech, cleanup is
skipped rather than attempted.

## Privacy

- Microphone audio is held in memory and **never written to disk**.
- Transcription runs on your machine, through a local ONNX model. There is no
  server and no account.
- **No telemetry, no analytics, no crash reporting.**
- Logs record durations, sample counts and event names, never audio,
  transcription text or clipboard contents.
- The only network calls are a model download you asked for, and the update check
  if you switch it on. With both done, the app works with the Wi-Fi off.
- One thing is written down, and only if you say so: Settings › History keeps a
  local list of what you dictated. It is off by default, you choose how long
  entries last, and switching it off deletes the file.

## Permissions

macOS asks for **Microphone** to record, and **Accessibility** to paste into the
app you are typing in. Both are granted in System Settings › Privacy & Security.

Windows asks for neither, with one exception: it will not let WhisperFree paste
into a window belonging to a program running as administrator. When that happens
the text is left on the clipboard and the app says so, so nothing is lost.

## Where things live

```
macOS    ~/Library/Application Support/com.bartek.whisperfree/
Windows  %APPDATA%\com.bartek.whisperfree\
```

Settings, the dictionary, the history if you enabled it, and downloaded models.
Logs are alongside, in `~/Library/Logs/com.bartek.whisperfree/` on macOS.

## Under the hood

A Rust backend and a Svelte 5 settings window on Tauri 2, with speech running
through ONNX Runtime. Why each significant choice was made, with the measurements
behind it, is written up in [`docs/decisions/`](docs/decisions).

`bun run check` typechecks the frontend and `cd src-tauri && cargo test` runs the
Rust tests; a pre-commit hook runs both. Releases are cut by hand from the
Actions tab, as described in [`docs/RELEASING.md`](docs/RELEASING.md).

## Licence

MIT. Model weights are downloaded separately and carry their own licences.
