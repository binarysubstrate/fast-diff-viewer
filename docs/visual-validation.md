# Diff Highlight Validation

Use these acceptance criteria when changing line matching, inline emphasis,
or rendered diff output. Automated role/span and render tests complement a
native visual review; a successful capture command alone is not design
signoff.

## Acceptance Criteria

- A related changed line has the pane's line tint, with a stronger inline
  tint over the words that changed. Shared words retain only the line tint.
- Added or removed blank lines do not make nearby related paragraphs lose
  their word highlights or pair with unrelated text.
- Splitting one related line into two, or joining two into one, preserves
  content alignment and highlights the actual word edits on the correct
  side. Shared text must not become a solid inline-highlight block.
- Unchanged accented text beside a split/join boundary keeps its line tint
  without false inline emphasis. A split near the start of a large changed
  region must not shift the pairing of subsequent paragraphs.
- Unrelated paragraphs receive line tint only, without scattered inline
  emphasis from coincidental shared words or spaces.
- Equal lines stay transparent. Left-pane changes stay in the rose family;
  right-pane changes stay in the green family. Gutters agree with the line
  roles, and stale revisions remain neutral until matching results arrive.
- The original Graphite palette is unchanged: left line `#3A1F25`, left
  inline `#A0384A`, right line `#1F3327`, and right inline `#316D4C`.
  Selection, the caret, and text remain legible.

## Native Checks

Build the shipping binary and run the synthetic highlight cases on Windows:

```shell
cargo build --release --locked
cargo run --release --locked --example visual_contract_run_all -- --out-dir docs/visual-contract --suffix before --only G1,G2,G3,G4,G5,G6,G7,G8,G9,G10
```

Case inputs live in
[`scripts/visual_contract_run_all.rs`](../scripts/visual_contract_run_all.rs).
The large-region cases use synthetic text in
[`tests/fixtures/diff/`](../tests/fixtures/diff/).
Inspect every captured case against the criteria above and these specific
expectations:

| Case | Required result |
| --- | --- |
| G1 / G2 | A leading blank line on either side preserves word emphasis for `approved` / `pending` and `production` / `staging`. |
| G3 | The reordered short lines do not disrupt the final paragraph's localized emphasis on the appended `and tracking` text. |
| G4 / G5 | A paragraph split or join preserves `approved` / `pending` emphasis without emphasizing the unchanged continuation; the shared footer stays neutral. |
| G6 | Unrelated engineering and bakery paragraphs have line tint only. |
| G7 / G8 | A split or join after `café` emphasizes only the `old` / `new` edit; the unchanged accented word and footer receive no inline emphasis. |
| G9 / G10 | Splitting or joining the first paragraph of a 20-paragraph changed region keeps section numbers aligned and localizes `approved` / `pending` emphasis through Section 19. |

The PNG analysis
helpers described in [`scripts/README.md`](../scripts/README.md#visual-capture)
can assist inspection but do not establish that the text pairing is correct.

On Ubuntu or macOS, launch the shipping build and paste the same synthetic
case inputs into the left and right panes:

```shell
cargo run --release --locked
```

For both methods, also edit a word in each pane, insert and remove a blank
line, and reverse a split/join. Wait for the diff to settle after each edit;
check the resulting line and word tints, then scroll and select text to check
that highlights remain aligned. For G9 and G10, inspect both the top and the
bottom of the buffers in a native session; the initial screenshot alone does
not cover the later paragraphs.

Record the commit, platform, case IDs, observed failures, and screenshot
locations with the review result. Mark visual acceptance only after the
captures or native session have been reviewed. Performance signoff remains
the separate gate in [`benchmark-contract.md`](benchmark-contract.md).
