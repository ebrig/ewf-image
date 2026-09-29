#!/usr/bin/env python3
"""Portable synthetic raw/E01/AFF4 acquisition timing with hash checks.

This measures the CLI's write, finalization, and verification path from an
ordinary file. It does not measure physical-device throughput or error handling.
Temporary source and outputs are owned by this run and removed on exit.
"""

import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess
import tempfile
import time


def acquire(binary: Path, source: Path, output: Path, expected: str, options: list[str]):
    started = time.monotonic()
    process = subprocess.run(
        [str(binary), "--json", "--quiet", "acquire", str(source), str(output), *options],
        capture_output=True,
        text=True,
        check=False,
    )
    seconds = time.monotonic() - started
    try:
        report = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"{output.name}: invalid CLI result: {process.stderr[-1000:]}") from error
    if process.returncode != 0 or report.get("status") != "complete":
        raise RuntimeError(f"{output.name}: {report.get('error', process.stderr[-1000:])}")
    digest = report.get("sha256") or (report.get("verification") or {}).get("sha256")
    if digest != expected:
        raise RuntimeError(f"{output.name}: acquired SHA256 differs from synthetic source")
    if output.suffix == ".E01":
        stored = sum(path.stat().st_size for path in output.parent.glob(output.stem + ".E*"))
    else:
        stored = output.stat().st_size
    return {
        "seconds": seconds,
        "mib_per_second": source.stat().st_size / (1024 * 1024 * seconds),
        "stored_bytes": stored,
        "timings": report.get("timings"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--baseline-binary", type=Path)
    parser.add_argument("--mib", type=int, default=128)
    parser.add_argument("--directory", type=Path)
    args = parser.parse_args()
    if not 4 <= args.mib <= 4096:
        parser.error("--mib must be 4 through 4096")
    binary = args.binary.resolve(strict=True)
    baseline = args.baseline_binary.resolve(strict=True) if args.baseline_binary else None
    with tempfile.TemporaryDirectory(prefix="ewf-throughput-", dir=args.directory) as name:
        root = Path(name)
        source = root / "source.raw"
        block = random.Random(0x455746).randbytes(1024 * 1024)
        sha256 = hashlib.sha256()
        with source.open("wb") as stream:
            for _ in range(args.mib):
                stream.write(block)
                sha256.update(block)
            stream.flush()
        expected = sha256.hexdigest()
        cases = [
            ("raw", "raw-output.raw", []),
            ("e01_zlib", "ewf-zlib.E01", []),
            ("e01_raw", "ewf-raw.E01", ["--compression", "raw"]),
            ("aff4_zlib", "aff-zlib.aff4", []),
            ("aff4_lz4_256k", "aff-lz4.aff4", ["--compression", "lz4", "--chunk-bytes", "262144"]),
        ]
        results = {}
        baseline_results = {}
        for label, filename, options in cases:
            results[label] = acquire(binary, source, root / filename, expected, options)
            if baseline is not None and label in {"raw", "e01_zlib", "aff4_zlib"}:
                baseline_results[label] = acquire(
                    baseline, source, root / f"baseline-{filename}", expected, []
                )
        print(json.dumps({
            "schema_version": 1,
            "source_bytes": source.stat().st_size,
            "source_sha256": expected,
            "notes": "Single pass per mode, ordinary buffered file, same-host cache effects; includes destination verification. Baseline runs after candidate for each common format. Not a physical-device benchmark.",
            "results": results,
            "baseline_results": baseline_results,
        }, indent=2))


if __name__ == "__main__":
    main()
