"""Independently summarize the saved CSVs without starting the GUI."""

import csv
import math
import pathlib


def stats(values):
    ordered = sorted(values)
    percentile = lambda q: ordered[math.ceil((len(ordered) - 1) * q)]
    return {
        "avg": sum(values) / len(values),
        "p50": percentile(0.50),
        "p95": percentile(0.95),
        "p99": percentile(0.99),
        "max": max(values),
    }


for mode in ("datatable", "uniform-list"):
    root = pathlib.Path("docs/spike")
    with (root / f"t003-{mode}-frames.csv").open() as f:
        intervals = [float(row["interval_ms"]) for row in csv.DictReader(f)]
    with (root / f"t003-{mode}-filter.csv").open() as f:
        filters = list(csv.DictReader(f))
    assert len(filters) == 22
    assert [int(row["rows"]) for row in filters] == [
        3000,
        1000,
        1000,
        1000,
        1000,
        1000,
        1000,
        1000,
        1000,
        1000,
        1000,
        200,
        20,
        2,
        5000,
        1000,
        1000,
        1000,
        5000,
        0,
        0,
        5000,
    ]
    print(mode, "frames", len(intervals), "seconds", sum(intervals) / 1000)
    print("aggregate fps", len(intervals) * 1000 / sum(intervals))
    print("instantaneous callback fps", stats([1000 / ms for ms in intervals]))
    print("interval ms", stats(intervals), "over 8 ms", sum(ms > 8 for ms in intervals))
    for key in ("ready_ms", "cpu_paint_ms", "next_frame_callback_ms"):
        print(key, stats([float(row[key]) for row in filters]))
