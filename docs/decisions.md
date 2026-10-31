# Decisions

## T-003 GPUI matrix spike, iteration 3

**Date:** 2026-10-31. **Final decision: NO-GO against the unchanged gate:**
120 fps scrolling on this Mac's 120 Hz display and filter response ≤16 ms/keystroke.
No acceptance tolerance or CPU-only substitute has been introduced.

**Recommended investigation config:** DataTable, exact **gpui-kit 0.8.1** /
gpui-component 0.8.1, **gpui-fast 0.1.12**, thin-LTO `spike` profile,
precomputed SharedString cells, **CAMetalLayer.maximumDrawableCount=2**.
This is an experimental configuration, not approval of the production matrix.
Double buffering remains worth investigating for scroll cadence, but does not
establish repeatable 120 fps or improve the measured filter presentation tail.
The core GPUI uniform_list grid remains the unapproved fallback from iteration 2.

### 1. Double-buffer repeatability

Five **valid, serial, unprofiled** repeats of the iteration-2 executable built from
`e972771761e62a894e31428b88c37760682b8bfa`, before adding scene/GPU instrumentation.
Same M5 Pro, internal 120 Hz display, foreground **1400×900 at (80,80)**, five-second
warm-up, twelve-second scroll at **2,880 logical pixels/second**, 5,000×12 cells.
The Metal probe and aggregate interval-count/elapsed-time FPS method are unchanged.
A hitch remains a presentation interval **>12.5 ms**, not an 8 ms approximation.

| Trial / raw suffix | Avg Metal fps | Hitches | Presentation intervals |
|---|---:|---:|---:|
| double-1 | 119.334403 | 8 | 1432 |
| double-2 | 119.917772 | 1 | 1439 |
| double-3 | 119.834517 | 2 | 1438 |
| double-9 | 119.250428 | 9 | 1430 |
| double-10 | 119.250964 | 9 | 1431 |

**Zero-hitch 120 fps reproduced in 0/5 valid runs.** Pooled: **119.517654 fps**,
**29/7170 hitches**, **59.991138 s** of within-run presentation intervals;
worst interval **16.666651 ms**. All **7170/7170 scroll callbacks were active**,
with no zero-presented-time drawables during scrolling. This does not prove
120 fps impossible, but the single clean iteration-2 run was not repeatable here.

There were ten repeat attempts, not five silently selected successes. Trials
4/5 failed geometry (1400×932; 900×900 at (900,39)), 6/7 lost focus, and 8 ended on
an external monitor. Trials 9/10 explicitly moved only the new process's window
to the built-in monitor/workspace **before** native sizing and warm-up; the measured
geometry/workload stayed the same. No focus or geometry correction runs during
measurement. Final CoreGraphics bounds, active counts, setup and exclusion reasons
for **all 17 attempts**, including later tagged trials, are in
[`spike/t003-i3-runs.csv`](spike/t003-i3-runs.csv). Bounds were checked at the end,
not continuously; brief geometry changes inside an otherwise valid run are unproven.

### 2. Residual hitch correlation

Every one of the **29** missed-refresh intervals was joined to its drawable
acquisition and the preceding last-child CPU paint marker. Paint-to-acquire gaps
were **0.048–1.785 ms**, keeping the matching within the same scroll frame.
Those frames' callback-start-to-paint durations were **1.224 ms median,
2.326 ms p95, 2.526 ms maximum**: none exceeded one 8.333 ms refresh period.
Their drawable acquisition waits were **9.112 ms median, 13.479 ms p95,
15.367 ms maximum**; **21/29** blocked in `nextDrawable` for more than one period.

**The majority's root cause remains unattributed.** Drawable-availability waiting
is the measured common symptom, not proof of which scheduler/GPU/compositor event
caused it. Rows enter view during scrolling, but this CPU timeline does not split
element creation, text shaping or allocation. It excludes post-marker renderer work
and the later element-arena cleanup, so it cannot exonerate or blame those phases.
No per-hitch process/scheduler trace exists. Unrelated Rust compilation and Defender
load were observed between attempts, not correlated to individual hitches; they
are not claimed as the cause. Our own builds did not overlap benchmark runs.

`xcrun xctrace version` still fails **exit 72**, “unable to find utility xctrace”.
No profiler was installed. The existing CPU timeline was used instead, with
per-hitch joins emitted by `python3 docs/spike/t003-iter3-summarize.py`.

### 3. Filter result presentation, not just rows-ready

Separate, explicitly instrumented trials tag the **last-child paint of each
filtered scene** with its generation and row count. The next drawable copies that
tag; `presentedTime` identifies its presentation. A `presentDrawable:` observer
also records submission and the command buffer's **GPUStartTime/GPUEndTime**.
The pinned source orders draw/paint → acquire drawable → render command buffer →
present → arena cleanup. All **110** expected row counts and generation matches
pass; the iteration-2 first-acquired-after-handler match picks the **same drawable
in 110/110 cases**, so this experiment finds no frame-identification error.

Two valid triple-buffer trials (**44 queries**) and three double-buffer trials
(**66 queries**) are compared. Two additional triple-buffer attempts lost focus
and are excluded. Every retained scroll callback and every observed drawable from
scroll start through the filter phase was active. The actual layer counts were
**3 and 2**, verified per result drawable. Values below are **p50 / p95 / max ms**.

| Handler/result-path measurement | Triple buffer | Double buffer |
|---|---:|---:|
| Rows ready | 0.079 / 0.249 / 0.278 | 0.065 / 0.200 / 0.253 |
| Last-child CPU paint | 2.156 / 4.954 / 6.084 | 1.957 / 4.641 / 5.308 |
| GPU execution only | 1.582 / 1.604 / 2.310 | 1.583 / 1.832 / 2.874 |
| GPU completion → presentation | 23.517 / 24.833 / 32.270 | 23.669 / 25.085 / 28.617 |
| Handler → first tagged result presentation | 28.008 / 28.749 / 36.360 | 28.025 / 28.877 / 36.303 |

Filter drawable acquisition p95 was only **0.073 / 0.054 ms**, and
submission-to-GPU-start p95 **0.856 / 0.619 ms**. Thus the roughly 28–36 ms response
is not 35 ms of filtering or GPU execution. Most delay is **after GPU completion**,
in the presentation path: median **2.82 / 2.84 nominal 120 Hz periods** there,
and **3.36 / 3.36 periods** from handler entry to the result. The historical
**35.650 ms p95** is not remeasured as the new p95; roughly 36 ms tails persist.
The exact cause of each historical outlier remains unproven.

Counting actual intervening presentations refutes a simple explanation of “2–3
older app frames queued ahead”: **42/44 triple-buffer edits and 61/66 double-buffer
edits had zero older frames presented between handler and result**. Remaining
counts were **2 once / 4 once** for triple, and **1 three times / 2 once / 3 once**
for double. Those older frames had already been acquired before the handler.
The result was usually the very next presented app frame, just delayed roughly
three display periods. Metal/compositor scheduling, adaptive scan-out and driver
buffer reuse are not separated by these measurements. We can locate the delay
after GPU completion, not identify its internal WindowServer/display policy.
**Double buffering does not lower the measured presentation p95** (28.749 →
28.877 ms); this is a serial comparison under uncontrolled external load, not a
claim that the 0.129 ms difference is statistically meaningful.

**Gate comparison:** handler entry → first presented drawable containing the
filtered result, **36.303 ms maximum (28.877 ms p95)** for the recommended
double-buffer config. **66/66 edits exceeded 16 ms** (minimum **19.669 ms**);
triple buffering also failed **44/44**. “≤16 ms/keystroke” needs the result visible,
not merely a ready vector or a CPU paint marker. This is already a failing
lower-bound scope: input editing/hardware delivery and optical scan-out are
excluded. Even a p95 interpretation would fail; no gate relaxation is necessary.

**Still unproven:** repeatable zero-hitch 120 fps; the root cause of the residual
hitches and post-GPU presentation delay; hardware-keystroke-to-photon response;
controlled-load/thermal results; production-sized real content and accessibility.
Neither “GPUI cannot do 120” nor “production-ready 120 fps” follows.

Raw iteration-3 CSVs total approximately **1.21 MB** under `docs/spike/`. Rejected
attempts retain compact validation metadata rather than bulk traces. Tagged
presentation CSVs retain the filter phase and older drawables presented after
its first handler; repeat CSVs retain full timelines. Reproduction and probe
scope are in [`spike/README.md`](spike/README.md). No new dependencies were added.

**Validation:** spike-fast optimized build, default-backend
`cargo check --workspace --all-targets --locked`, `cargo fmt --all -- --check`,
Clang `-Wall -Wextra` syntax check, all five repeat summaries/29 hitch joins/110
tagged filters, unchanged historical iteration-2 summary, and `git diff --check`
pass. Upstream `block 0.1.6` still emits Cargo's future-incompatibility warning.

## T-003 GPUI matrix spike, iteration 2 (historical)

**Date:** 2026-10-31. **Decision: NO-GO for accepting the unchanged gate.**

The earlier 30 fps result was not a demonstrated GPUI rendering limit. The
inactive-window limiter reproduces its 33–42 ms cadence, and disabling only that
limiter restores approximately 120 fps. Full-width foreground rendering is now
measured using **Metal drawable presentation timestamps**, independently of
animation callbacks. Three valid runs per implementation still miss sustained
120 fps: **119.140 fps DataTable, 118.945 fps uniform_list**, with missed refreshes.
One extra double-buffer DataTable run achieves **120.001 fps with zero hitches**
for 12 seconds, so this investigation does **not** prove GPUI cannot do it.
Repeatable performance with that intervention remains inconclusive. Filter CPU
paint passes 16 ms; handler-to-Metal-presentation does not.

Keep **`gpui-kit = "=0.8.1"`** as the investigation pin, not an approved production
matrix selection. The optional `skm-app/spike-fast` feature selects **gpui-fast
0.1.12** instead of **gpui-pre 0.3.10**. Components remain **gpui-component 0.8.1**.
The real application is unchanged. All renderer/harness changes are in the
standalone example and `docs/spike/`; the optional feature and custom Cargo
profile exist solely to reproduce the experiment.

### Machine, build and geometry

- Apple **M5 Pro**, 20 GPU cores, macOS **27.0.1 (26A434)**, Rust **1.96.1**.
  This is not the M1 reference machine in spec §14.
- CoreGraphics reports the **built-in main display at 120 Hz**, 1800 × 1169
  logical pixels. Both external LG displays report 144 Hz. The benchmark stays
  at **(80, 80), 1400 × 900**, entirely on the built-in display, with all twelve
  columns visible. These are actual saved CoreGraphics bounds, not just options.
  Updated screenshots: [`spike/t003-matrix.png`](spike/t003-matrix.png) and
  [`spike/t003-fallback.png`](spike/t003-fallback.png).
- Final foreground runs use `cargo build -p skm-app --example matrix_spike
  --profile spike --features spike-fast`: release optimization, **thin LTO**,
  **codegen-units=1**, line-table debug info. No debug logger or per-frame stdout.
  Enabling the requested fast backend required `cargo update -p taffy
  -p unicode-properties`, selecting its exact pins **0.15.0 / 0.1.3**. No unrelated
  dependency upgrade or profiling-tool installation was performed.
- Names are precomputed **SharedString** cells, glyphs use static strings, and
  fallback visible indices use `Rc<Vec<usize>>` instead of a per-frame 5,000-index
  copy. Sample vectors reserve capacity. GPUI element/layout allocation remains;
  this is not an allocation-free renderer. These cheap changes were tested as a
  bundle, not isolated factorial experiments, so individual speedups are unknown.

### Root cause of the earlier numbers

The pinned GPUI implementation runs `on_next_frame` callbacks **before draw**,
despite that method's doc comment. `request_animation_frame` is implemented using
the same callback mechanism; the original loop does not inherently halve the
frame rate. A callback interval measures main-thread scheduling, not GPU presents.
Its reciprocals can exceed 120 because of jitter/bunching.

More importantly, GPUI's default `WindowOptions.inactive_frame_interval` is
**33,333 µs**. In `gpui-pre/src/window.rs`, the frame loop skips pending animation
work when the window is inactive, absent high-rate real input. Four 120 Hz ticks
are approximately the threshold; an early tick can fail the comparison, delaying
work until tick five, **41.667 ms**. Programmatically setting a scroll offset does
not count as high-rate input. The original harness neither recorded nor enforced
window activation. Tiling to 600 × 1129 also made it an incomplete-width workload.

Controlled tests on the same 1400 × 900 visible window, fast backend:

| Control | Active callbacks | Callback aggregate fps | Metal aggregate fps |
|---|---:|---:|---:|
| Resign key window, default inactive limiter | 0 / 311 | 25.901 | 25.899 |
| Resign key window, only inactive limiter disabled | 0 / 1434 | 119.499 | 119.501 |

The limited control has callback p50 **41.092 ms**, p95 **41.959 ms**, max
**43.433 ms**, while CPU paint p95 is only **3.442 ms**. This isolates the cap
rather than blaming text shaping, filtering, allocation, or arena cleanup for
the characteristic 42 ms waits. Source locations are `gpui-pre-0.3.10/src/
platform.rs:2770`, `src/window.rs:1825–1874,1878–1913`; the fast backend has the
same limiter.

Historical CSVs contain **35 intervals near 33 ms and 49 near 41.7 ms** for
DataTable, versus **84 / 117** for the fallback. Slow intervals pull count/time
down to 81 / 30 fps while the median can still be approximately 120. Arithmetic
mean FPS and the median conceal this time cost. **Historical focus was not
logged**, so the old runs' exact focus history cannot be reconstructed. The
limiter is a reproduced causal explanation matching their signature, not proof
that every historical slow interval had this one cause.

### Corrected final scroll measurements

Final numbers pool **three serial, unprofiled, default-buffer runs per renderer**,
labels `fast-datatable-{2,3,4}` and `fast-uniform-{2,3,4}`. Each has five seconds
of warm-up and twelve seconds of scrolling at **2,880 logical pixels/second**,
computed from elapsed time, rather than 24 pixels per callback. **Every measured
callback was active**: 4288/4288 and 4281/4281 respectively. No build ran alongside
these measurements. Ordinary system load and thermal state were not isolated.

FPS here is **Metal `MTLDrawable.presentedTime` cadence**. Avg means aggregate
interval count / elapsed time, not mean of reciprocals. p50/p95/p99 are quantiles
of `1000 / interval_ms`, so high FPS percentiles describe the fastest frames.

| Renderer | Avg fps | p50 fps | p95 fps | p99 fps | Hitches >12.5 ms |
|---|---:|---:|---:|---:|---:|
| DataTable | 119.140 | 120.002 | 120.002 | 120.002 | 31 / 4288 intervals |
| uniform_list | 118.945 | 120.002 | 120.002 | 120.002 | 38 / 4281 intervals |

| Presentation interval metric | DataTable | uniform_list |
|---|---:|---:|
| Measured seconds | 35.991372 | 35.991367 |
| ms p50 / p95 / p99 / max | 8.333206 / 8.333445 / 8.333445 / 16.671181 | 8.333206 / 8.333445 / 8.333445 / 16.671658 |
| Arithmetic mean reciprocal fps | 119.567 | 119.468 |
| Per-run aggregate fps range | 118.751–119.584 | 118.417–119.251 |

These runs have **no zero-presented-time drawables** in the measurement phase and
no gaps >33.333 ms. The 120.002 figure is clock/CSV precision around nominal 120,
not faster-than-panel presentation. A hitch is **>12.5 ms**, 1.5 display periods;
the typical hitch here is a missed refresh, approximately 16.667 ms. A literal
8 ms frame-interval test is inconsistent with nominal 120 Hz's 8.333 ms period.
No looser acceptance tolerance has been substituted for the requested 120 fps.

Callback aggregates are **119.121 / 118.946 fps**, but callback p95 reciprocal FPS
is **142.416 / 142.844**. Presentation quantization at 120 independently confirms
why callback reciprocals alone were the wrong reporting method.

### Filter latency

All **66 queries per backend** are included, with independently asserted expected
row counts. Times are **p50 / p95 / max in milliseconds**.

| Handler entry → marker | DataTable | uniform_list |
|---|---:|---:|
| Filtered rows ready | 0.077 / 0.269 / 0.429 | 0.091 / 0.262 / 0.436 |
| Last-child CPU paint | 2.136 / 5.353 / 5.998 | 1.683 / 3.721 / 4.607 |
| First result drawable's Metal presentation | 27.301 / 35.650 / 45.141 | 27.195 / 28.644 / 36.944 |

The handler starts after programmatic input editing. CPU work passes **16 ms**,
but it is not an end-to-end pass: Metal presentation already exceeds that budget.
These measurements exclude hardware-keyboard delivery and optical scan-out.

### Profiling, extra levers and limits

- Full Xcode is absent: `xcrun xctrace record --template 'Animation Hitches'`
  fails with **exit 72**, “unable to find utility xctrace”. `samply` is absent.
  Built-in **`sample PID 10 1`** captured the original DataTable, standard-backend
  precomputed DataTable, and fast-backend fallback. Raw profiles are retained as
  `t003-i2-*-sample.txt.gz`. Sampling includes startup/warm-up; profiled runs are
  not pooled with final performance measurements.
- The standard DataTable profile has **3732 / 7737 main-thread samples** waiting
  in the event loop, and a **652-sample** branch in `CAMetalLayer.nextDrawable`.
  Layout, painting, allocation and shaping are visible, but this is not evidence
  of 42 ms of first-time shaping or an arena “GC” pause. During final scrolling,
  CPU-marker p95/max is **2.058 / 6.500 ms** for DataTable, **2.724 / 3.982 ms** for
  the fallback. Drawable acquisition can consume a display period while the CPU
  is waiting for a free buffer. Residual missed refreshes are **not individually
  attributed** by this sampling profile; exact scheduler/GPU/compositor blame
  needs a full correlated trace.
- An explicitly labelled **`maximumDrawableCount=2`** intervention produced
  **120.001 fps, zero hitches / 1439 intervals, 11.991573 s** for DataTable.
  The fallback produced **119.418 fps, seven hitches / 1433 intervals**.
  Presented-filter p95 remained **28.869 / 27.362 ms**. This setting is applied
  natively by the experiment, not silently by the passive observer and not yet
  integrated into the production app. Later repeats lost focus or stalled;
  `*-invalid.txt` records preserve those failures. No repeatability claim or
  impossibility proof follows from one clean run.
- Without Metal observation, callback aggregates were **119.621 / 119.918 fps**.
  The gross 30 fps cap therefore is not caused by the presented-handler probe.
  Small probe overhead cannot be bounded from these single no-probe controls.
- AX sizing timed out with the fast backend. Final runs use an explicit one-time
  native AppKit bounds/activation step before warm-up ends. The failed tiled
  trials and an early offscreen inactive control are retained but excluded, with
  reasons, rather than counted as full-width measurements.

### Method and fallback

The standalone Objective-C probe observes `CAMetalLayer.nextDrawable`, registers
`addPresentedHandler` and reads the drawable's **presentedTime**, not callback
receipt time. A fixed preallocated buffer stores samples; SIGUSR1 writes CSVs
after DONE. Phase markers align app timestamps with the probe. This observes
Metal-reported presentation, **not physical photons**. The summary recomputes
percentiles from raw CSVs, excludes explicitly invalid runs, and pools within-run
intervals without including gaps between processes. Full commands, timing scope,
controls and invalid-run explanations are in [`spike/README.md`](spike/README.md).
Machine-readable results are in [`spike/t003-i2-summary.json`](spike/t003-i2-summary.json).

**Fallback remains a core GPUI uniform_list grid**, not a selected production
replacement. It preserves a fixed name cell outside a shared horizontal agent
scroller, but differs in row height and component work and lacks production
sorting/resizing/selection/accessibility coverage. It now runs near 120, not 30,
yet does not rescue the strict gate or presented-filter latency.

**Unproven:** repeatable zero-hitch 120 fps with double buffering, the cause of
each residual missed refresh, hardware-input-to-photon latency, adaptive panel
scan-out, performance under controlled load/thermal conditions, realistic data,
accessibility, idle power and the remainder of spec §14. Neither “GPUI cannot do
120” nor “production-ready 120 fps” is supported. The next useful experiment is a
focused, repeatable double-buffer run with correlated presentation/scheduler
tracing, not another callback-only FPS estimate.

### Validation

- **PASS:** release baseline and thin-LTO fast-backend spike builds; both renderers
  completed three valid foreground GUI runs; full-width screenshots inspected.
- **PASS:** raw-CSV summary, all 66 expected filter row counts per final backend,
  foreground active-state assertions, `cargo fmt --all -- --check` and
  `git diff --check`; default-backend `cargo check --workspace --all-targets --locked`.
- **FAIL:** unchanged sustained-120-fps gate on the repeated default-buffer runs;
  handler-to-presentation filter latency exceeds 16 ms.
- **INCONCLUSIVE:** repeatability of the one zero-hitch double-buffer result;
  interrupted/defocused attempts are explicitly marked invalid.
- **UNAVAILABLE:** Instruments/xctrace and samply; built-in sample was used instead.
- **WARNING:** upstream `block 0.1.6` emits Cargo's future-incompatibility warning.

Raw CSVs, scripts and spike code: branch `t003-iter3` (1d98f7d), kept off the main line.
