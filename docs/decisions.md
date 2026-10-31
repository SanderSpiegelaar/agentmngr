# Decisions

## T-003 GPUI matrix spike

**Date:** 2026-10-31. **Decision: NO-GO against the stated performance gate.**
Both implementations build and run, custom styling works, and filter-handler to
CPU-paint time passes 16 ms in this sample. Neither demonstrates sustained
120 fps scrolling or the “no frame >8 ms” requirement. Do not mark T-003's
performance acceptance as passed. This is not evidence that GPUI can never meet
the budget; platform scheduling, ProMotion and system load remain unisolated.

**Pinned candidate:** `gpui-kit = "=0.8.1"`, default features disabled, with
`component` and `assets` enabled in skm-app. Cargo.lock resolves gpui-component
0.8.1 and gpui-pre 0.3.10. Keep this pin for further investigation, not an approved
production matrix choice. Spike code lives only in
`crates/skm-app/examples/matrix_spike.rs`; the real application is unchanged.

### Machine and display

- Actual host: Apple **M5 Pro**, 20 GPU cores, Rust **1.96.1**, release build.
  This is not the M1 reference machine mentioned in spec §14.
- `system_profiler SPDisplaysDataType` reports the internal Liquid Retina XDR
  panel but omits its refresh rate. Both attached LG displays report **144 Hz**.
- CoreGraphics current-mode query reports the **main built-in display at 120 Hz**.
  Both measured windows were on that display. This reports the configured mode,
  not a measurement of instantaneous adaptive-refresh scan-out.
- Requested window: 1320 × 820 logical pixels. The window manager actually tiled
  each to **600 × 1129**, origin (600, 39), as recorded in the display text files.
  The matrix has all 12 columns, but only a horizontal subset is visible at once.
  This is not a full-width, all-columns-visible benchmark.

### Measured numbers

FPS below means **animation-callback-equivalent FPS**, not verified GPU presents.
Aggregate FPS is count / elapsed seconds. Arithmetic mean and percentile FPS
apply to the individual reciprocals `1000 / interval_ms`. High FPS percentiles
describe the fastest intervals, so the interval tails are included as well.

| Scroll metric | DataTable | Core uniform_list fallback |
|---|---:|---:|
| Intervals / measured seconds | 646 / 7.979 | 242 / 8.025 |
| Aggregate FPS / arithmetic mean FPS | 80.961 / 107.661 | 30.156 / 42.300 |
| FPS p50 / p95 / p99 | 119.974 / 130.588 / 151.794 | 29.323 / 120.397 / 133.035 |
| Interval ms p50 / p95 / p99 / max | 8.335 / 41.609 / 41.695 / 41.774 | 34.131 / 41.709 / 41.879 / 41.997 |

Intervals over 8 ms: **592/646** for DataTable, **235/242** for the fallback.
Some individual intervals correspond to ≥120 FPS, but **sustained 120 FPS was not
reached**. Callback reciprocals above the panel's 120 Hz mode indicate callback
timing jitter/bunching, not faster display presentation. Also, a perfectly even
120 Hz display period is 8.333 ms, so a literal ≤8 ms interval gate is stricter
than nominal 120 FPS. Both implementations miss either interpretation here.

Each backend applied the same **22 filter changes**. Times are milliseconds.

| Filter metric, p50 / p95 / max | DataTable | Core uniform_list fallback |
|---|---:|---:|
| Handler entry → filtered rows ready | 0.087 / 0.134 / 0.137 | 0.077 / 0.259 / 0.451 |
| Handler entry → CPU paint marker | 2.180 / 3.358 / 5.950 | 2.082 / 5.044 / 6.201 |
| Handler entry → next animation callback | 40.648 / 41.541 / 41.620 | 41.467 / 41.683 / 41.733 |

Rows-ready and CPU paint markers pass 16 ms in this sample. The next-callback
proxy does not. Hardware-keypress-to-photon latency is **inconclusive**, not a
claimed pass.

### Method

1. Built with `cargo build -p skm-app --example matrix_spike --release`.
2. Generated 5,000 deterministic names and 12 columns (name plus 11 glyph
   columns). DataTable uses a delegate, a fixed-left 250 px name column and 92 px
   agent columns. Cells alternate ●/○/◐. A real Input field drives filtering.
3. Applied an application-owned graphite/sage palette, system text font, Menlo
   names, 3 px general radius, 4 px maximum radius and no shadows through
   `Theme::update`, including table tokens. Screenshots verify a non-stock theme.
4. Warmed up for 2 seconds. For approximately 8 seconds, an `on_next_frame` loop
   advanced the vertical scroll handle by 24 logical pixels per callback and
   notified the views. Used `Instant` between callbacks; saved every interval.
   The callback schedules its successor; no timer-based synthetic FPS estimate.
5. After scrolling, applied incremental prefixes (r → rust-skill-000 and g → git),
   clears, and zero-match z/zz queries, separated by 12 callbacks. The script uses
   `InputState::set_value` then the same filter handler used by interactive input.
   Timing starts at **filter-handler entry**, excluding input editing and OS
   keystroke delivery. Rows-ready includes scanning all names, updating visible
   indices, refreshing DataTable and resetting scroll position.
6. A final, non-drawing canvas child's paint callback timestamps the CPU paint
   phase of each changed result. This is after building/layout/painting the
   matrix's preceding siblings, but **not Metal completion or display scan-out**.
   Separately recorded the following animation callback, which need not be a
   presentation timestamp and can follow the CPU paint marker much later.
7. Saved stdout and CSVs, then independently recomputed all summaries and
   asserted the 22 expected row counts with `python3 docs/spike/t003-summarize.py`.
8. Captured the actual DataTable window using
   `screencapture -l 3258 -x docs/spike/t003-matrix.png`; fallback window 3280
   using the same command into `docs/spike/t003-fallback.png`. Closed only the
   spike processes afterwards. Window IDs are ephemeral.

### Fallback and limits

The fallback is a core GPUI `uniform_list` grid with the name cell outside a
shared horizontal agent-column scroller. It shares the data/filter path, theme,
glyphs and sampling scheme. It is **not chosen**: its measured aggregate callback
rate is lower, and it does not rescue the acceptance gate. It is a minimal
comparison, not a tuned or feature-equivalent replacement: 28 px rows versus
DataTable's built-in small row height, different column-rendering work, and no
sorting/resizing/selection/accessibility implementation.

Only one successful run per backend is recorded; no statistical confidence
interval, controlled thermal/load state, Instruments/Metal tracing, or external
photon measurement. The approximately 41.7 ms tail could include adaptive
refresh/scheduling effects; these results do not isolate their cause. Full-width
rendering, real item descriptions, hardware input, idle power, memory, startup,
and the rest of §14 are unmeasured. No 120 FPS production claim is justified.

**Next investigation:** rerun on an untiled, full-width window with controlled
system load and presentation tracing before selecting a production matrix.
The reproduction commands and raw evidence are in [`spike/README.md`](spike/README.md).

### Validation

- **PASS:** release build; both GUI runs completed; screenshots captured.
- **PASS:** independent CSV summary and expected filter row-count assertions.
- **PASS:** `cargo fmt --all -- --check` and `git diff --check`.
- **WARNING:** upstream `block 0.1.6` emits Cargo's future-incompatibility warning.
- **FAIL:** scroll performance gate for both implementations.
- **NOT MEASURED:** GPU presentation rate and hardware-keystroke latency.
