# T-003 reproduction

From the repository root, on macOS with an unlocked graphical session:

```sh
cargo build -p skm-app --example matrix_spike --release
system_profiler SPDisplaysDataType > docs/spike/t003-system-profiler.txt
./target/release/examples/matrix_spike > docs/spike/t003-datatable-stdout.txt
# After DONE, capture the window and close this process before the fallback run.
swift docs/spike/t003-display.swift > docs/spike/t003-display.txt
screencapture -l <window-id-from-display.txt> -x docs/spike/t003-matrix.png
./target/release/examples/matrix_spike --fallback > docs/spike/t003-uniform-list-stdout.txt
```

Each run warms up for 2 seconds, then records approximately 8 seconds of
`on_next_frame` intervals while advancing the vertical scroll handle by 24 logical
pixels each callback. The requested window is 1320 × 820 logical pixels on the
main display, but the window manager tiled both measured runs to 600 × 1129.
Check the saved CoreGraphics window bounds rather than assuming the requested size.
Do not cover, minimize, or move the window during measurement.

After scrolling, the harness applies 22 filter changes, one every 12 callbacks,
including incremental prefixes, empty queries, and no-match queries. These are
programmatic input changes, not hardware keyboard events. Both programmatic and
interactive changes use the same filter handler. The timer begins at handler
entry, after `InputState::set_value`; it excludes OS keyboard delivery and input
text editing. Rows-ready includes scanning all 5,000 names and refreshing the
table and resetting the scroll position. A last-child canvas paint callback records the CPU paint-phase marker
for that result. A separate timestamp records the next animation callback.
Neither is a Metal completion or scan-out timestamp.

Raw CSVs and stdout are saved per backend. Frame FPS quantiles are quantiles of
`1000 / interval_ms`, using nearest rank (ceiling index); aggregate FPS is frame
count divided by total elapsed time. High FPS percentiles describe the fastest
callbacks, not tail stalls; consult interval percentiles and maximum too.

`python3 docs/spike/t003-run.py [--fallback]` runs either benchmark and returns
after DONE while leaving the window open; it prints that process's PID for
subsequent cleanup. `python3 docs/spike/t003-summarize.py` recomputes the numbers
from CSVs and checks the expected result counts without opening the GUI.

This is a deliberately standalone example, not production matrix architecture.
The fallback shares the synthetic data and filter handler with DataTable, but
renders cells using core GPUI `uniform_list`. The first cell sits outside the
agent-column horizontal scroller. Accessibility, sorting, selection, resizing,
and production data/index integration are not benchmarked.
