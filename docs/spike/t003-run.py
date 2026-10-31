"""Run one release benchmark; leave its window open after DONE for capture."""

import pathlib
import subprocess
import sys

mode = "uniform-list" if "--fallback" in sys.argv else "datatable"
args = ["target/release/examples/matrix_spike"]
if mode == "uniform-list":
    args.append("--fallback")
process = subprocess.Popen(
    args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True
)
print(f"spike pid={process.pid}", flush=True)
with pathlib.Path(f"docs/spike/t003-{mode}-stdout.txt").open("w") as log:
    for line in process.stdout:
        log.write(line)
        log.flush()
        print(line, end="", flush=True)
        if line.startswith("DONE:"):
            break
    else:
        raise SystemExit(f"spike exited before DONE: {process.wait()}")
