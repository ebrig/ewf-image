#!/usr/bin/env python3
"""Linux AFF4 ENOSPC acceptance on a new private tmpfs (run as root).

Only mounts a fresh temporary directory; never opens or formats a disk device.
Requires the release example acquire and ewf-cli. Existing outputs are
never supplied to the writer. This tests capacity failure, not power loss.
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--acquire", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("root is required for a private tmpfs mount")
    acquire = args.acquire.resolve(strict=True)
    binary = args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="aff4-storage-"))
    mount = root / "volume"
    mount.mkdir()
    mounted = False
    try:
        subprocess.run(["mount", "-t", "tmpfs", "-o", "size=8m,nosuid,nodev,noexec", "tmpfs", str(mount)], check=True)
        mounted = True
        source = root / "source.raw"
        block = random.Random(0xAFF4).randbytes(1024 * 1024)
        source.write_bytes(block * 16)
        for profile in ["physical", "logical"]:
            output = mount / f"{profile}.aff4"
            command = [str(acquire), str(source), str(output)]
            if profile == "logical":
                command.append("logical")
            failed = subprocess.run(command, capture_output=True, text=True, timeout=120)
            assert failed.returncode != 0, "full filesystem unexpectedly succeeded"
            assert "No space left on device" in failed.stderr, failed.stderr
            assert not output.exists(), "failed acquisition published a final path"
            assert not list(mount.iterdir()), "handled failure left writer scratch"
            # Free capacity by reducing this synthetic source, then retry fresh.
            source.write_bytes(block)
            completed = subprocess.run(command, capture_output=True, text=True, timeout=120, check=True)
            expected = hashlib.sha256(block).hexdigest()
            assert expected in completed.stdout, "writer source hash mismatch"
            verified = subprocess.run([str(binary), "--json", "verify", str(output)], capture_output=True, text=True, timeout=120, check=True)
            assert json.loads(verified.stdout)["status"] == "verified", "reopened content verification failed"
            output.unlink()
            source.write_bytes(block * 16)
        print(json.dumps({"physical_enospc": "passed", "logical_enospc": "passed", "retry_and_verify": "passed", "filesystem": "private 8 MiB tmpfs"}))
    finally:
        if mounted:
            # If unmount fails, preserve the directory for operator cleanup.
            subprocess.run(["umount", str(mount)], check=True, timeout=30)
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
