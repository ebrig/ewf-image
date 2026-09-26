#!/usr/bin/env python3
"""Test one-shot EWF2 CLI ENOSPC/retry on a new private Linux tmpfs.

Run as root on a disposable Linux host. No disk device is opened or formatted.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import signal
import subprocess
import tempfile


def invoke(binary, arguments, expected):
    result = subprocess.run([str(binary), "--json", "--quiet", "ewf", *map(str, arguments)], capture_output=True, text=True, timeout=120)
    assert result.returncode == expected, result.stdout + result.stderr
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("root is required for the private tmpfs mount")
    binary = args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="ewf-sequential-storage-"))
    volume = root / "volume"
    volume.mkdir()
    inputs = root / "source"
    inputs.mkdir()
    mounted = False
    try:
        subprocess.run(["mount", "-t", "tmpfs", "-o", "size=8m,nosuid,nodev,noexec", "tmpfs", str(volume)], check=True)
        mounted = True
        block = random.Random(0xE2F2).randbytes(1024 * 1024)
        source = inputs / "data.raw"
        for command, extension, input_path in [("acquire-sequential", "Ex01", source), ("collect", "Lx01", inputs)]:
            output = volume / f"case.{extension}"
            source.write_bytes(block * 16)
            arguments = [command, input_path, output, "--chunks-per-segment", "32", "--compression", "raw"]
            failed = invoke(binary, arguments, 1)
            assert "No space left on device" in failed["error"], failed
            assert not output.exists(), "failed operation exposed a final image"
            invoke(binary, ["recover-publication", output], 0)
            assert all(path.name.endswith(".lock") for path in volume.iterdir()), "unexpected staging after handled failure"
            source.write_bytes(block)
            completed = invoke(binary, arguments, 0)
            assert completed["published"] is True
            expected = hashlib.sha256(block).hexdigest()
            assert completed["verification"]["sha256"] == expected
            if command == "collect":
                assert completed["verified_files"] == 1
            for path in completed["segments"]:
                candidate = Path(path).resolve(strict=True)
                assert candidate.parent == volume.resolve(strict=True)
                candidate.unlink()
        # Use an owned sparse regular file so cancellation is exercised through
        # the actual CLI signal handler without requiring a device or FIFO.
        with source.open("wb") as stream:
            stream.truncate(512 * 1024 * 1024)
        output = volume / "cancel.Ex01"
        child = subprocess.Popen([str(binary), "ewf", "acquire-sequential", str(source), str(output)],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            first = child.stderr.readline()
            assert first.startswith("acquisition:"), first
            child.send_signal(signal.SIGINT)
            stdout, stderr = child.communicate(timeout=30)
            assert child.returncode == 130, stdout + stderr
            report = json.loads(stdout)
            assert report["published"] is False and report["status"] == "cancelled"
            assert not output.exists()
        finally:
            if child.poll() is None:
                child.kill()
                child.communicate()
        print(json.dumps({"physical_enospc": "passed", "logical_enospc": "passed", "retry_and_verify": "passed", "sigint": "passed", "filesystem": "private 8 MiB tmpfs"}))
    finally:
        if mounted:
            subprocess.run(["umount", str(volume)], check=True, timeout=30)
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
