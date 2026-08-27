# 0010 — Preserving the clipboard across an insertion

**Status:** accepted
**Date:** 2026-08-27
**Applies to:** `text_insertion/`, `platform/*/clipboard.rs`, `platform/*/text.rs`, `dictation.rs`

Insertion is clipboard plus a synthetic paste (decision from plan §11, unchanged here), so
every dictation borrows the user's clipboard for about 150 ms and has to give it back. The
old code gave back only text, and inferred what it could not give back. Both halves were
wrong, and the second half was visible: users saw

> Text inserted. Your clipboard held an image, which could not be restored.

after dictating with no image anywhere near their clipboard.

## What the message actually meant

It was not a detection. It was an inference from two failures:

```rust
let previous = clipboard.read_text().ok();
let had_non_text = previous.is_none() && clipboard.read_image().is_ok();
```

The two reads are not symmetric, because the clipboard plugin's `arboard` implements them
against different APIs. `read_text` walks `pasteboardItems()` and asks each item for
`stringForType:`, which is a per-item read that performs no coercion. `read_image` asks the
*pasteboard* for `dataForType(public.tiff)`, which does coerce: a pasteboard carrying only
`public.png` answers that call, and answers it with a TIFF.

So `had_non_text` really meant "I could not read text, and something here can be turned
into an image". The first clause is where it went wrong.

## macOS declares flavours it has not produced

A pasteboard flavour can be a promise. An application copying a formatted selection writes
the RTF concretely and lets macOS advertise `public.utf8-plain-text` as derivable from it;
the bytes are produced later, by asking the application that did the copying. Once that
application has quit, the flavour is still listed and can no longer be produced, and a read
of it returns nothing.

That is a clipboard that holds text, says it holds text, and reads as holding no text. It
was reported as an image. Measured against the real pasteboard, the sequence is:

| | `stringForType(utf8-plain-text)` | `dataForType(tiff)` |
|---|---|---|
| before, owner gone | `None` | 229174 bytes |
| before, owner alive | `"Q1\tQ2\nrevenue\tprofit"` | 229174 bytes |

The read is also the fix: asking for a promised flavour while it can still be served both
fulfils it and caches it, so it survives the owner quitting afterwards.

Windows has the same shape under a different name. `GetClipboardData` on a delay-rendered
format asks the owning window to render it, and gets nothing once that window is gone.

## Capture every flavour, not just the text

So `platform/*/clipboard.rs` snapshots the whole clipboard, flavour by flavour and item by
item, and writes the whole thing back. This fixes three things at once.

The text case above is fixed twice over: the snapshot asks for every flavour, which
materialises the promise while it can be materialised, and it keeps the concrete RTF the
promise was derived from, so restoring re-establishes the derivation with this app as the
live owner.

An image is no longer destroyed at all. The old code read one, discarded it, and reported
the loss; the message was accurate and the loss was avoidable. Files, PDFs and rich text
are the same story, none of which the old wording admitted to.

And nothing has to be classified any more. The old code needed to know *what kind of thing*
was on the clipboard in order to describe what it had broken. Restoring everything makes
the question moot, which is the part worth keeping: a rule that never has to name a content
type cannot name it wrongly.

The cost is memory, bounded at `MAX_SNAPSHOT_BYTES` (32 MB) per snapshot, held for the
length of one paste. A copied artboard can exceed it; past the ceiling the snapshot stops
collecting and marks itself incomplete, which loses the tail of a very large clipboard
rather than the app.

## An incomplete snapshot is a log line, never an error

`ClipboardOutcome::PartlyRestored` exists and is never shown to anyone. A flavour that
could not be read is nearly always a derived one, and derived flavours come back on their
own once the flavour they derive from is restored: the measured run above ends with
`incomplete = true` and a pasteboard whose plain text reads correctly. Surfacing it would
reproduce the false alarm this decision removes, with new wording.

That leaves `RestoreFailed`, which means the clipboard could not be written back at all, as
the one outcome the user hears about. It goes through `fail_app_state` because that is the
only channel the app has, and it is worth the interruption: the user's clipboard is gone
and this app took it.

The insertion itself succeeded in both cases, which is why neither is an `InsertError`. The
state machine has no notion of a notice, and inventing one for a condition that should now
be close to unreachable was not worth the surface.

## Consequences

- `ClipboardOutcome::NonTextReplaced` is gone. `PartlyRestored` replaces it and means
  something different: not "this was not text" but "one flavour of this could not be read".
- `ClipboardOutcome::after_restore` and `lost_the_clipboard` are pure and carry the tests,
  the same split as `hotkey::decide` and `state::is_valid`. Both backends share them: what
  the platforms capture differs, what the user is owed does not.
- `platform/*/clipboard.rs` is a backend-private module, reached only as
  `platform::backend::clipboard`. Nothing outside `platform/` names a pasteboard type or a
  clipboard format, and `text_insertion/` still knows only `text -> InsertOutcome`.
- The clipboard plugin is still used to *write* the transcription, and only for that. It
  cannot express what these modules read.
- Windows skips formats whose handle is not global memory (bitmaps, palettes, metafiles).
  They are GDI objects that cannot be copied as bytes, and Windows synthesises them from
  formats that can be.
