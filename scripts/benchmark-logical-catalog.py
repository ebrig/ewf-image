#!/usr/bin/env python3
"""Measure a CLI's synthetic large-catalog collection and verification on Linux.

The optional AFF4 mode records bounded-reader rejection as a failed verification,
not successful acceptance. Uses a new temporary tree and never real evidence.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--files", type=int, default=50000)
    parser.add_argument("--aff4", action="store_true")
    parser.add_argument("--limit-metadata-bytes", type=int)
    parser.add_argument("--limit-triples", type=int)
    args = parser.parse_args()
    if not 1 <= args.files <= 99999:
        parser.error("file count must be between 1 and 99999")
    binary = args.binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="logical-catalog-scale-") as name:
        root = Path(name)
        source = root / "source"
        source.mkdir()
        block = bytes(n % 251 for n in range(1024))
        expected = hashlib.sha256()
        for index in range(args.files):
            (source / f"file-{index:06}").write_bytes(block)
            expected.update(block)
        destination = root / ("case.aff4" if args.aff4 else "case.Lx01")
        command = [str(binary)] + ([] if args.aff4 else ["--quiet"])
        command += ["collect", str(source), str(destination)]
        for option in ["metadata_bytes", "triples"]:
            value = getattr(args, "limit_" + option)
            if value is not None:
                if not args.aff4:
                    parser.error("limit overrides require --aff4")
                command += ["--limit-" + option.replace("_", "-"), str(value)]
        start = time.monotonic()
        with (root / "stdout.json").open("w") as output, (root / "stderr.log").open("w") as error:
            child = subprocess.Popen(command, stdout=output, stderr=error)
            try:
                _, status, usage = os.wait4(child.pid, 0)
                child.returncode = os.waitstatus_to_exitcode(status)
            except BaseException:
                child.kill()
                child.wait()
                raise
        elapsed = time.monotonic() - start
        report = json.loads((root / "stdout.json").read_text())
        assert destination.exists() == bool(report.get("published")), "publication report disagrees with filesystem"
        if child.returncode == 0 and not args.aff4:
            assert report["verified_files"] == args.files
            assert report["verification"]["sha256"] == expected.hexdigest()
        if child.returncode == 0 and args.aff4:
            assert len(report["output"]["streams"]) == args.files
            assert all(item["sha256"] == hashlib.sha256(block).hexdigest() for item in report["output"]["streams"])
        print(json.dumps({"profile": "aff4" if args.aff4 else "ewf2", "files": args.files,
            "source_bytes": args.files * 1024, "elapsed_seconds": elapsed,
            "peak_rss_bytes": usage.ru_maxrss * 1024, "exit_code": child.returncode,
            "published": report.get("published"), "verification_passed": child.returncode == 0,
            "limits": report.get("limits"),
            "verification_error": report.get("verification_error", report.get("error"))}, indent=2))
        if child.returncode != 0:
            raise SystemExit(child.returncode)


if __name__ == "__main__":
    main()
