#!/usr/bin/env python3
"""Linux synthetic EWF2/AFF4 payload and large-catalog scale measurements.

Build release examples sequential/catalog_scale, AFF4 acquire, and both CLIs.
Reports wait4 process RSS (not filesystem cache) and elapsed wall time. Outputs
are decoded/verified locally; this does not measure independent compatibility.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import tempfile
import time


def measure(root, name, command):
    start = time.monotonic()
    with (root / f"{name}.out").open("w") as output, (root / f"{name}.err").open("w") as error:
        process = subprocess.Popen([str(value) for value in command], stdout=output, stderr=error)
        try:
            _, status, usage = os.wait4(process.pid, 0)
            process.returncode = os.waitstatus_to_exitcode(status)
        except BaseException:
            process.kill()
            process.wait()
            raise
    if process.returncode:
        raise RuntimeError(f"{name}: {(root / f'{name}.err').read_text()[-2000:]}")
    return {"elapsed_seconds": time.monotonic() - start, "peak_rss_bytes": usage.ru_maxrss * 1024}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--mib", type=int, default=1024)
    parser.add_argument("--files", type=int, default=10000)
    parser.add_argument("--directory", type=Path)
    args = parser.parse_args()
    if not 1 <= args.mib <= 65536 or not 1 <= args.files <= 100000:
        parser.error("mib must be 1..65536 and files 1..100000")
    release = args.release.resolve(strict=True)
    results = {"source_mib": args.mib, "catalog_files": args.files, "measurements": {}}
    with tempfile.TemporaryDirectory(prefix="ewf-aff4-scale-", dir=args.directory) as name:
        root = Path(name)
        if shutil.disk_usage(root).free < args.mib * 3 * 1024 * 1024 + 256 * 1024 * 1024:
            raise RuntimeError("insufficient free space for bounded synthetic measurement")
        source = root / "source.raw"
        block = random.Random(0xEAF4).randbytes(1024 * 1024)
        digest = hashlib.sha256()
        with source.open("wb") as output:
            for _ in range(args.mib):
                output.write(block)
                digest.update(block)
            output.flush()
            os.fsync(output.fileno())
        results["source_sha256"] = digest.hexdigest()
        metrics = results["measurements"]
        for codec in ["raw", "zlib"]:
            output_dir = root / f"ewf-{codec}"
            output_dir.mkdir()
            path = output_dir / "case.Ex01"
            metrics[f"ewf2_{codec}_write_and_verify"] = measure(root, f"ewf-{codec}", [release / "examples/sequential", source, path, codec])
            measure(root, "ewf-verify", [release / "ewf-image", "--quiet", "verify", path])
            report = json.loads((root / "ewf-verify.out").read_text())
            assert report["verification"]["sha256"] == digest.hexdigest(), "EWF source SHA256 mismatch"
            metrics[f"ewf2_{codec}_write_and_verify"]["stored_bytes"] = sum(p.stat().st_size for p in output_dir.glob("*.Ex*"))
            shutil.rmtree(output_dir)
        path = root / "physical.aff4"
        metrics["aff4_physical_zlib_write"] = measure(root, "aff4-write", [release / "examples/acquire", source, path])
        metrics["aff4_physical_verify_all"] = measure(root, "aff4-verify", [release / "aff4-image", "verify", path])
        report = json.loads((root / "aff4-verify.out").read_text())
        assert digest.hexdigest() in [stream["verification"]["sha256"] for stream in report["resources"] if stream.get("verification")], "AFF4 source SHA256 mismatch"
        metrics["aff4_physical_zlib_write"]["stored_bytes"] = path.stat().st_size
        path.unlink()
        source.unlink()
        metrics["ewf2_catalog_write_and_compare"] = measure(root, "ewf-catalog", [release / "examples/catalog_scale", root / "catalog.Lx01", args.files])
        catalog = root / "files"
        catalog.mkdir()
        for index in range(args.files):
            (catalog / f"file-{index:06}").write_bytes(block[:1024])
        metrics["aff4_catalog_collect_and_verify"] = measure(root, "aff4-catalog", [release / "aff4-image", "collect", catalog, root / "catalog.aff4"])
        report = json.loads((root / "aff4-catalog.out").read_text())
        assert report["published"] and len(report["output"]["streams"]) == args.files
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
