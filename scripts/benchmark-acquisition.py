#!/usr/bin/env python3
"""Measure synthetic CLI acquisition/resume on Linux, including verified output.

Uses Linux wait4 resource accounting for per-process peak RSS. All generated
files live in one temporary directory and are removed on exit. No devices open.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import signal
import subprocess
import sys
import tempfile
import time


def measure(binary, directory, name, arguments, expected_code):
    report_path = directory / f"{name}.json"
    log_path = directory / f"{name}.log"
    peak_scratch = 0
    start = time.monotonic()
    with report_path.open("wb") as report, log_path.open("wb") as log:
        process = subprocess.Popen(
            [str(binary), "--quiet", *arguments], cwd=directory, stdout=report, stderr=log,
            start_new_session=True)
        try:
            while True:
                pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                if pid:
                    process.returncode = os.waitstatus_to_exitcode(status)
                    peak_rss_kib = usage.ru_maxrss
                    break
                size = 0
                scratch = directory / ".case.E01.ewf-acquisition" / "scratch"
                try:
                    for path in scratch.iterdir():
                        try:
                            size += path.stat().st_size
                        except FileNotFoundError:
                            pass
                except FileNotFoundError:
                    pass
                peak_scratch = max(peak_scratch, size)
                time.sleep(0.02)
        except BaseException:
            # Stop the child before removing its workspace.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
            raise
    elapsed = time.monotonic() - start
    if process.returncode != expected_code:
        raise RuntimeError(f"{name} exited {process.returncode}: "
                           f"{report_path.read_text()} {log_path.read_text()}")
    result = json.loads(report_path.read_text())
    return {
        "elapsed_seconds": elapsed,
        "peak_rss_bytes": peak_rss_kib * 1024,
        "sampled_peak_scratch_bytes": peak_scratch,
        "result": result,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--mib", type=int, default=512)
    parser.add_argument("--compression", choices=["raw", "zlib"], default="zlib")
    parser.add_argument("--directory", type=Path, help="parent directory for temporary test files")
    args = parser.parse_args()
    if not sys.platform.startswith("linux"):
        parser.error("run on Linux for wait4 high-water RSS accounting")
    if args.mib < 4 or args.mib % 4:
        parser.error("--mib must be a positive multiple of four, at least four")
    binary = args.binary.resolve(strict=True)
    size = args.mib * 1024 * 1024
    with tempfile.TemporaryDirectory(prefix="ewf-acquisition-benchmark-", dir=args.directory) as name:
        directory = Path(name)
        block = random.Random(0x455746).randbytes(1024 * 1024)
        expected = hashlib.sha256()
        with (directory / "source.raw").open("wb") as source:
            for _ in range(args.mib):
                source.write(block)
                expected.update(block)
            source.flush()
            os.fsync(source.fileno())
        acquisition = measure(binary, directory, "acquire", [
            "acquire", "source.raw", "case.E01", "--compression", args.compression,
            "--stop-after", str(size // 4)], 130)
        resumed = measure(binary, directory, "resume", ["resume", "case.E01"], 0)
        result = resumed["result"]
        if result["status"] != "complete" or result["verification"]["sha256"] != expected.hexdigest():
            raise RuntimeError("resumed output failed independent source SHA256 comparison")
        total_seconds = acquisition["elapsed_seconds"] + resumed["elapsed_seconds"]
        print(json.dumps({
            "schema_version": 1,
            "source_bytes": size,
            "compression": args.compression,
            "stored_image_bytes": sum(path.stat().st_size for path in directory.glob("case.E*")),
            "end_to_end_mib_per_second": args.mib / total_seconds,
            "measurement_notes": "Includes checkpoint validation, resume rehash, publication, and verification. Scratch peaks are sampled lower bounds; RSS is Linux wait4 process high-water RSS. Synthetic source and output may benefit from OS caches.",
            "acquire": acquisition,
            "resume": resumed,
        }, indent=2))


if __name__ == "__main__":
    main()
