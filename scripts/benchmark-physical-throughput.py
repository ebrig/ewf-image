#!/usr/bin/env python3
"""Compare release CLI acquisition throughput from ordinary files.

Runs paired baseline/candidate trials in alternating order and checks the
reported media SHA256. This is a buffered-file benchmark, not a physical-device
or read-error benchmark. Results include publication and destination verification.
"""

import argparse
import hashlib
import json
from pathlib import Path
import random
import statistics
import subprocess
import tempfile
import time


MIB = 1024 * 1024
CASES = {
    "raw": ("image.raw", []),
    "e01_zlib": ("image.E01", []),
    "e01_raw": ("image.E01", ["--compression", "raw"]),
    "aff4_zlib": ("image.aff4", []),
    "aff4_lz4_256k": (
        "image.aff4",
        ["--compression", "lz4", "--chunk-bytes", "262144"],
    ),
}
BASELINE_CASES = {"raw", "e01_zlib", "aff4_zlib"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(MIB), b""):
            digest.update(block)
    return digest.hexdigest()


def source_file(path: Path, mib: int, kind: str) -> str:
    digest = hashlib.sha256()
    generator = random.Random(0x455746)
    phrase = b"ewf-image throughput measurement\n"
    pattern = (phrase * (MIB // len(phrase) + 1))[:MIB]
    with path.open("wb") as output:
        for _ in range(mib):
            block = generator.randbytes(MIB) if kind == "random" else pattern
            output.write(block)
            digest.update(block)
    return digest.hexdigest()


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
        raise RuntimeError(
            f"{output.name}: invalid CLI result: {process.stderr[-1000:]}"
        ) from error
    if process.returncode != 0 or report.get("status") != "complete":
        raise RuntimeError(f"{output.name}: {report.get('error', process.stderr[-1000:])}")
    digest = report.get("sha256") or (report.get("verification") or {}).get("sha256")
    if digest != expected:
        raise RuntimeError(f"{output.name}: acquired SHA256 differs from synthetic source")
    if output.suffix == ".E01":
        stored = sum(path.stat().st_size for path in output.parent.glob("image.E*"))
    else:
        stored = output.stat().st_size
    return {
        "seconds": seconds,
        "mib_per_second": source.stat().st_size / (MIB * seconds),
        "stored_bytes": stored,
        "timings": report.get("timings"),
    }


def summary(runs: list[dict]) -> dict:
    timings = {}
    for key in sorted({key for run in runs for key in (run["timings"] or {})}):
        values = [
            run["timings"][key]
            for run in runs
            if isinstance((run["timings"] or {}).get(key), (int, float))
        ]
        if values:
            timings[key] = statistics.median(values)
    return {
        "median_seconds": statistics.median(run["seconds"] for run in runs),
        "median_mib_per_second": statistics.median(
            run["mib_per_second"] for run in runs
        ),
        "median_timings": timings,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--baseline-binary", type=Path)
    parser.add_argument("--mib", type=int, default=128)
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    parser.add_argument(
        "--datasets", nargs="+", choices=["random", "compressible"],
        default=["random", "compressible"],
    )
    args = parser.parse_args()
    if not 4 <= args.mib <= 4096:
        parser.error("--mib must be 4 through 4096")
    if not 1 <= args.trials <= 10:
        parser.error("--trials must be 1 through 10")
    binary = args.binary.resolve(strict=True)
    baseline = args.baseline_binary.resolve(strict=True) if args.baseline_binary else None
    identities = {
        "candidate": {"path": str(binary), "sha256": sha256_file(binary)},
        "baseline": ({"path": str(baseline), "sha256": sha256_file(baseline)}
                     if baseline else None),
    }
    results = {}
    with tempfile.TemporaryDirectory(prefix="ewf-throughput-", dir=args.directory) as name:
        root = Path(name)
        for kind in args.datasets:
            source = root / f"{kind}.raw"
            expected = source_file(source, args.mib, kind)
            cases = {}
            for label in args.cases:
                filename, options = CASES[label]
                runs = {"candidate": [], "baseline": []}
                paired = baseline is not None and label in BASELINE_CASES
                for trial in range(args.trials):
                    order = ["candidate", "baseline"] if trial % 2 == 0 else ["baseline", "candidate"]
                    for role in order if paired else ["candidate"]:
                        selected = binary if role == "candidate" else baseline
                        with tempfile.TemporaryDirectory(prefix="trial-", dir=root) as trial_dir:
                            run = acquire(
                                selected, source, Path(trial_dir) / filename, expected, options
                            )
                        runs[role].append(run)
                result = {role: {"summary": summary(values), "runs": values}
                          for role, values in runs.items() if values}
                if paired:
                    result["candidate_speedup"] = (
                        result["baseline"]["summary"]["median_seconds"]
                        / result["candidate"]["summary"]["median_seconds"]
                    )
                cases[label] = result
            results[kind] = {"source_sha256": expected, "cases": cases}
    print(json.dumps({
        "schema_version": 2,
        "source_mib": args.mib,
        "trials": args.trials,
        "binaries": identities,
        "notes": "Buffered file source; same-host cache, thermal, CPU, and destination effects remain. Paired runs alternate order. Physical-device throughput is not measured.",
        "results": results,
    }, indent=2))


if __name__ == "__main__":
    main()
