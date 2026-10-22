# Fast Diff Viewer Performance Algorithms

This document explains the algorithms and guardrails that keep the app fast.
It complements the runtime map in [architecture.md](architecture.md) and the
measured gates in [benchmark-contract.md](benchmark-contract.md).

## Performance Model

The app optimizes for interactive editing of medium and large text buffers
inside stock `egui::TextEdit` widgets. The expensive work is split into two
groups:

- Diff work runs on a background thread and is coalesced so bursts of edits do
  not build a backlog.
- Paint work stays on the UI thread, but stale or unchanged inputs reuse cheap
  fallbacks and cached galleys instead of rebuilding styled text every frame.

The code deliberately avoids product-shape changes such as a virtualized custom
editor or an `egui` fork. Those could change the ceiling, but they also change
the product contract.

## Line Segmentation

Owner: [`src/segment.rs`](../src/segment.rs)

`segment(text)` is the single line segmentation entry point. It walks the input
bytes, splits only on literal `\n`, and returns byte ranges that exclude the
newline plus a flag that records whether the segment was newline-terminated.
An empty buffer returns zero segments, and a terminal newline does not create
an extra empty segment.

Why it is fast:

- The algorithm is linear in the byte length of the buffer.
- It stores byte ranges rather than allocating one `String` per line.
- Every caller uses the same segmentation rule, which avoids reconciliation
  passes between diffing and painting.

Important invariant:

- CRLF normalization is not done here. `\r\n` remains one segment whose range
  includes `\r`; compare-only normalization happens in `diff::tokenize()`.

## Diff Tokenization

Owner: [`src/diff.rs`](../src/diff.rs)

`tokenize(text)` converts canonical line segments into borrowed `DiffToken`s. A
token is a byte slice into the original buffer with one trailing `\r` stripped
for comparison plus the `terminated` bit. The termination bit is what lets
`"a\nb\n"` and `"a\nb"` compare as a changed final line even when the final
line bytes match.

Why it is fast enough today:

- It is linear in the number of bytes being tokenized.
- CRLF normalization is compare-only and local to the token, so segment counts
  never change.
- The downstream diff operates on line tokens rather than raw bytes or
  characters.
- Tokens borrow from the worker-owned job strings, avoiding one allocation and
  one byte copy per line.

Important invariant:

- Token content strips one trailing `\r` from any segment, including an
  unterminated final line ending in `\r`. That preserves the historical
  compare-only behavior; the implementation does not special-case only CRLF.

## Line Diff And Roles

Owner: [`src/diff.rs`](../src/diff.rs)

`capture_line_diff(left, right)` interns complete line tokens into integer IDs
for the current job, then calls `similar::capture_diff_slices` with
`Algorithm::RawMyers` over those IDs. In `similar` 3.2, `RawMyers` preserves
the exact shortest-edit-script behavior previously exposed as `Myers`; the
new `Myers` variant uses a bounded heuristic search.

`assign_roles_and_emphasis()` keeps exact nonblank matches as `Equal` anchors.
Blank matches, including ASCII-whitespace-only lines, stay inside the regions
between those anchors. They cannot force edited paragraphs into the wrong
pairing merely because both paragraphs have a blank separator.

Within a bounded region, an order-preserving dynamic program selects related
one-to-one line pairs or one-to-two / two-to-one split and join pairs. Split
and join candidates contain adjacent nonblank lines only. Candidate scores
use matched token content, excluding ASCII punctuation and whitespace from
the byte counts. A candidate needs more than 40% shared content and must pass
the substantial-line similarity guard described below.

Selected related pairs become `Changed`, with the pane's line tint and word
emphasis. Exact one-to-one pairs become `Equal`; unrelated substantial pairs
and excess lines remain `AddedOrRemoved`, with line tint only. Residual lines
are paired positionally while skipping unmatched blank lines. This also
preserves ordinary short-line edits that share no complete word token.

Why it is fast:

- Myers runs at line granularity, not character granularity.
- Interning compares full token content and termination flags, including when
  hashes collide. The initial exact diff compares integer IDs instead of
  repeatedly comparing line bytes, without changing its operations.
- Identical regions and singleton replacements bypass the alignment search.
- Refinement is limited to 64 lines per side and 16 KiB of combined content
  per region. Candidate word comparisons share an estimated work budget of
  262,144 units across the entire job, checked before a region is refined.
- Regions outside those limits use guarded positional pairing with unmatched
  blank lines skipped. A region with an empty side needs no word diff.

Tradeoff:

- Myers has good behavior when edits are small relative to the input, which
  matches the editor fixtures. Completely unrelated large files are still a
  harder case for any exact sequence diff.
- Interning allocates a map and an ID sequence proportional to the current
  job's line count. They are discarded when the line diff completes; no
  cross-job cache retains earlier buffers.
- Content alignment allocates a bounded score table and comparison-only text
  for split/join candidates. Larger or more expensive regions retain the
  positional fallback rather than searching without a limit.

## Inline Emphasis

Owner: [`src/diff.rs`](../src/diff.rs)

`paired_line_diff(left_line, right_line)` tokenizes a candidate line or block pair
into maximal runs of ASCII word bytes (alphanumeric or underscore) and
non-word bytes, then runs the same Myers diff at token granularity. It returns
byte ranges for deleted tokens on the left and inserted tokens on the right,
plus word counts, a similarity score, and the content-alignment score.
Adjacent spans are merged.

When both sides contain at least 12 tokens with ASCII alphanumeric bytes and
their similarity is below 0.4, the pair is treated as unrelated. Both lines
receive line tint only, avoiding scattered emphasis when a blank-line anchor
pairs different paragraphs. Similarity is twice the byte count of all matched
tokens, including non-word tokens, divided by the combined line byte length.
Short lines retain ordinary word diffing, so one-word edits still receive
inline emphasis. This guard remains separate from the content score used to
choose alignment candidates.

For a split or join, `block_text()` joins the candidate lines with synthetic
newlines for comparison only. `project_spans()` maps the resulting emphasis
back into each original line's content range and excludes those synthetic
separators. Buffer text, canonical segmentation, and CRLF normalization are
unchanged. Related split/join lines retain line tint while unchanged words
avoid the stronger inline tint.

Why it is fast:

- It skips tokenization when compared content is identical. Candidate work
  is bounded by the alignment limits above.
- Tokenization is one linear pass over each changed line.
- Word tokens borrow slices of the worker-owned text, avoiding a separate
  allocation and byte copy for every word and separator run.
- It reuses the existing `similar` dependency instead of introducing a second
  diff engine.

Correctness guard:

- Multi-byte UTF-8 bytes are classified as non-word and stay in the same run,
  so emphasis ranges do not split a UTF-8 code point.

## Bounded Async Pipeline

Owner: [`src/pipeline.rs`](../src/pipeline.rs)

The UI thread submits whole-buffer jobs to one worker thread. The pipeline keeps
at most one in-flight job and one queued job:

- If the worker is idle, the edit is sent immediately.
- If a job is already in flight, the newest edit overwrites the single queued
  slot.
- When the in-flight result lands, the queued slot is promoted.
- Results older than the last applied revision are dropped.

When both buffers are below 200,000 bytes, `App` checks for edits immediately
following the left editor and again after both editors. The first check lets
left-side diff computation overlap right-pane rendering; comparing against the
last submitted buffers prevents the second check from submitting duplicate
work. Right-side edits are still detected by the final check. Results remain
asynchronous and probes resolve only after matching styled paint.

The early check is skipped when either buffer reaches the shared large-buffer
threshold. Advancing both buffer stamps before the right pane renders would
otherwise risk two large neutral layout rebuilds in one frame. Large buffers
retain their previous dispatch order and staggered styled application.

Why it is fast:

- Typing bursts cannot enqueue an unbounded number of stale diff jobs.
- The worker always converges toward the newest buffers.
- The UI thread never computes the diff directly.

Correctness guard:

- Each result carries buffer stamps for the exact text that produced it. The
  renderer uses those stamps before it allows styled paint.

## Stamps And Styled-Paint Validity

Owners: [`src/buffer_stamp.rs`](../src/buffer_stamp.rs),
[`src/render.rs`](../src/render.rs)

`BufferStamp` is `(revision, byte_len, text_hash)`. The renderer builds a
`PaintIdentity` from the current pane text and the stamp that produced the
applied roles and emphasis.

`is_styled_paint_valid()` returns true only when:

- current stamp equals source stamp,
- current byte length matches the text being rendered,
- `segment(text).len()` equals `line_roles.len()`, and
- `segment(text).len()` equals `inline_emphasis.len()`.

Why it is fast:

- Stale same-line-count edits are rejected by stamp equality, not by expensive
  byte-by-byte verification during paint.
- Invalid styled input takes a cheap neutral path instead of attempting partial
  repair.

Correctness guard:

- A stale or misaligned frame paints no tint and no emphasis. The app never
  paints half-current, half-stale styling.

## LayoutJob Construction

Owner: [`src/render.rs`](../src/render.rs)

`build_layout_job()` converts roles and emphasis into an `egui::LayoutJob`.
This is a major UI-thread hot path because `egui` lays out the resulting job
into a `Galley`.

Fast paths:

- Empty text returns an empty job immediately.
- Invalid styled input emits neutral per-segment sections. This is visually
  equivalent to one neutral buffer section, but avoids feeding egui one giant
  word-wrap section on large buffers.
- Consecutive `Equal` lines are consolidated into one neutral section.
- Changed lines fold a trailing newline into the tinted tail section instead of
  adding another section.
- Inline emphasis is applied only on `Changed` lines and only when ranges are
  in bounds and ordered.

Why it is fast:

- Large mostly-equal buffers produce far fewer layout sections.
- Layout section count tracks changed regions rather than total line count in
  the common case.
- Corrupt emphasis ranges degrade one line to tint-only instead of forcing a
  whole-pane rebuild strategy.

## Galley Layout Cache

Owner: [`src/app.rs`](../src/app.rs)

Each pane memoizes one `Galley`. The cache key includes:

- paint identity,
- role hash,
- inline-emphasis hash,
- theme and pane paint hash, and
- wrap width bits from the egui layouter callback.

Why it is fast:

- Unchanged frames submit the cached layout job to egui's memoized font
  layout, which normally returns the existing `Arc<Galley>` and marks its
  paragraph entries as used. egui evicts entries not used in the preceding
  pass, regardless of whether the app still owns a galley. Keeping those
  entries alive lets the next edit reuse unchanged paragraphs instead of
  shaping the entire buffer again.
- The cache key includes every input that changes text layout or paint formats,
  so hits are cheap without risking stale styled paint.

Tradeoff:

- The cache stores only one galley per pane. That matches the app's steady
  state and avoids an unbounded layout cache.
- Each cache hit clones and hashes the layout job and touches egui's cached
  paragraphs. This keeps the displayed text's paragraph galleys alive in
  egui; a font or atlas reset can still require layout again.

## Large-Buffer Stagger

Owner: [`src/app.rs`](../src/app.rs)

When either buffer is at least 200 KB, the app applies the left pane result on
the current frame and defers the right pane result by one frame.

Why it is fast:

- A single large styled galley rebuild can dominate a frame. Deferring one pane
  prevents two large rebuilds from landing in the same `App::ui()` call.
- Smaller edit-latency fixtures stay on the immediate two-pane path because the
  extra frame would cost latency without improving the frame budget.

Tradeoff:

- Large buffers can take one extra frame before both panes are styled. The
  benchmark settle gate covers that behavior.

## Scroll Synchronization

Owner: [`src/app.rs`](../src/app.rs)

The app reads each pane's `ScrollAreaOutput.state.clamped_offset()`, clamps the shared
offset to the opposite pane's previous max extent, and writes at most one new
shared offset per frame.

Why it is fast:

- It uses egui's scroll-state output instead of intercepting input events.
- The one-writer-per-frame rule prevents scroll ping-pong.
- Clamping before `ScrollArea::begin()` avoids out-of-range offsets on shorter
  panes.

## Gutter Painting

Owner: [`src/app.rs`](../src/app.rs)

The gutter reuses the `TextEditOutput` galley rows, paints a cached marker
glyph, and skips rows outside the visible gutter band.

Why it is fast:

- It does not run its own text layout for every row.
- It culls scrolled-out rows before painting.
- It mirrors the styled-paint validity gate, so stale frames paint a blank
  gutter instead of doing corrective work.

## Alternatives Considered

### Borrow Diff Token Bytes

`DiffToken` borrows `&[u8]` from the original buffer. Tokens are produced and
consumed inside the worker while the job strings are still alive; the pipeline
result stores only stamps, roles, and emphasis ranges.

Benefit:

- Removes one allocation and one byte copy per line token.
- Keeps the same segmentation, CRLF compare-only normalization, Myers line
  diff, and inline-emphasis behavior.

Tradeoff:

- Adds lifetimes to `DiffToken` and token-related tests.
- Must keep token use scoped to the worker pass; tokens should not be stored in
  pipeline results or UI state.

Status:

- Implemented. This is a real hot-path allocation reduction, not a
  benchmark-specific shortcut.

### Use `memchr` For Segmentation

`segment()` could use a byte-search helper such as `memchr` instead of a manual
byte loop.

Expected benefit:

- Potentially faster newline discovery on large buffers.

Risk:

- Adds a dependency for a small helper.
- The current segmenter is simple, covered, and not known to dominate the
  release gates.

Decision:

- Only if profiling shows segmentation is a top contributor.

### Cache Tokens Across Jobs

The app could cache token vectors by `BufferStamp` and reuse the unchanged side
across sequential edits.

Expected benefit:

- Avoids retokenizing the side that did not change.

Risk:

- Increases retained memory and invalidation complexity.
- The pipeline's latest-wins behavior intentionally keeps only minimal state.
- `TextEdit` gives the app whole buffers, so a robust cache still needs stamp
  checks and ownership boundaries.

Decision:

- Accept for now. Borrowed tokens remove much of the tokenization allocation
  cost without introducing a retained cache.

### Incremental Diffing Or A Rope

An incremental text model could track edited ranges and diff only around
changed spans.

Expected benefit:

- Better asymptotic behavior for very large buffers with tiny edits.

Risk:

- Stock `TextEdit` owns the editing interaction and exposes the buffer as a
  `String`; building a second incremental model would add synchronization
  complexity without changing the editor surface.
- A rope or custom editor would be a product-design change, not a local
  algorithm swap.

Decision:

- Out of scope for prototype.

### Parallel Inline Emphasis

Changed-line pairs could be split across worker threads.

Expected benefit:

- May help when a diff contains many large paired replacements.

Risk:

- Adds dependency and scheduling overhead.
- Current fixtures have small changed-line pairs, so per-line work is not
  heavy enough to amortize parallelism.
- The UI bottleneck is often galley rebuild, not inline-emphasis CPU.

Decision:

- Out of scope for prototype.

### Replace Myers With Another Diff Strategy

Patience or histogram-style diffs can produce better hunks for some inputs and
avoid some worst cases.

Expected benefit:

- Potentially better behavior on highly repetitive or unrelated files.

Risk:

- Changes visible diff semantics.
- Requires new acceptance fixtures for role pairing and emphasis behavior.
- The current Myers line diff is predictable and already measured against the
  release fixtures.

Decision:

- Out of scope for prototype. Don't change without specific failing input class and visual acceptance criteria.

### Replace `TextEdit` Or Fork `egui`

A virtualized editor or custom galley pipeline could avoid laying out the full
buffer.

Expected benefit:

- Biggest possible change to the large-buffer performance ceiling.

Risk:

- Changes the product contract, editing behavior, and native input reliance.
- Requires a much larger test and accessibility surface.

Decision:

- Out of scope for a local performance pass.
