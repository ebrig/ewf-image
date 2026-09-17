#!/usr/bin/env python3
"""Linux acceptance tests using only newly allocated loop/DM devices and images.

Run as root on a disposable host. No existing disk is formatted or written.
Requires losetup, dmsetup, mount, umount, mkfs.ext4, setpriv, and pinned libewf.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid


def command(*args, input=None):
    result = subprocess.run([str(arg) for arg in args], input=input, text=True,
                            capture_output=True, timeout=120, check=False)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout.strip()


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        while block := stream.read(1024 * 1024):
            value.update(block)
    return value.hexdigest()


class Devices:
    def __init__(self):
        self.loops = {}
        self.mappers = {}
        self.mounts = []

    def attach(self, image, sector=512, readonly=True, device=None):
        args = ["losetup", "--sector-size", str(sector)]
        if readonly:
            args.append("--read-only")
        if device is None:
            device = command(*args, "--find", "--show", image)
        else:
            command(*args, device, image)
        self.loops[device] = str(image.resolve())
        return device

    def detach(self, device):
        expected = self.loops[device]
        actual = json.loads(command("losetup", "--json", "--list", "--output", "NAME,BACK-FILE", device))
        entries = actual["loopdevices"]
        if len(entries) != 1 or entries[0]["name"] != device or entries[0]["back-file"] != expected:
            raise RuntimeError(f"refusing to detach changed loop ownership: {device}")
        command("losetup", "--detach", device)
        del self.loops[device]

    def mapper(self, table):
        name = "ewf-accept-" + uuid.uuid4().hex
        identifier = "EWF-ACCEPT-" + uuid.uuid4().hex
        command("dmsetup", "create", name, "--readonly", "--uuid", identifier, input=table)
        self.mappers[name] = identifier
        return "/dev/mapper/" + name

    def mount(self, source, target, *options):
        command("mount", *options, source, target)
        self.mounts.append((str(source), str(target)))

    def mapper_action(self, mapped, action):
        name = Path(mapped).name
        identifier = self.mappers[name]
        actual = command("dmsetup", "info", "--columns", "--noheadings", "--options", "uuid", name)
        if actual != identifier:
            raise RuntimeError("mapper ownership changed; refusing suspend/resume")
        command("dmsetup", action, "--noflush", name)

    def close(self):
        # Never remove backing images if detachment fails. The caller retains
        # the workspace and reports its exact location for operator recovery.
        for source, target in reversed(self.mounts):
            actual = json.loads(command("findmnt", "--json", "--mountpoint", target, "--output", "SOURCE,TARGET"))["filesystems"]
            if len(actual) != 1 or actual[0]["source"] != source or actual[0]["target"] != target:
                raise RuntimeError("mount ownership changed; refusing cleanup")
            command("umount", target)
        self.mounts.clear()
        for name, identifier in list(self.mappers.items()):
            actual = command("dmsetup", "info", "--columns", "--noheadings", "--options", "uuid", name)
            if actual != identifier:
                raise RuntimeError("mapper ownership changed; refusing cleanup")
            command("dmsetup", "remove", name)
            del self.mappers[name]
        for device in list(self.loops):
            self.detach(device)


def cli(binary, root, args, code=0, prefix=()):
    result = subprocess.run([*prefix, str(binary), "--quiet", *map(str, args)],
                            cwd=root, capture_output=True, text=True, timeout=180, check=False)
    report = json.loads(result.stdout)
    if result.returncode != code or report["exit_code"] != code:
        raise AssertionError(f"{args}: expected {code}, got {result.returncode}: {report}")
    return report


def oracle(args, output, expected):
    exported = subprocess.run([str(args.ewfexport), "-q", "-u", "-f", "raw", "-t", "-", str(output)],
                              capture_output=True, timeout=120, check=True)
    if hashlib.sha256(exported.stdout).hexdigest() != expected:
        raise AssertionError("libewf export differs from independent source digest")
    command(args.ewfverify, "-q", output)


def history_report(args, root, output, expected):
    history = cli(args.binary, root, ["report", output])["history"]
    latest = history["latest_run"]["result"]
    assert latest["verification"]["sha256"] == expected, history
    assert latest["status"] in ("complete", "complete_with_substitutions"), history
    saved = output.parent / ("." + output.name + ".ewf-report.json")
    assert json.loads(saved.read_text()) == history
    return history


def stalled_device(args, root, devices, mapped, expected, cancel):
    """Suspend only our mapper after a real checkpoint, holding kernel reads."""
    label = "cancel" if cancel else "timeout"
    output = root / f"stalled-{label}.E01"
    checkpoint = root / f".stalled-{label}.E01.ewf-acquisition" / "checkpoint-00001"
    stdout = root / f"stalled-{label}.json"
    stderr = root / f"stalled-{label}.stderr"
    arguments = [str(args.binary), "--quiet", "acquire", mapped, str(output),
                 "--compression", "raw", "--sectors-per-chunk", "1",
                 "--chunks-per-segment", "128", "--zero-fill", "--retries", "100"]
    if not cancel:
        arguments += ["--read-timeout-ms", "1000"]
    suspended = False
    report = None
    with stdout.open("w") as out, stderr.open("w") as err:
        process = subprocess.Popen(arguments, cwd=root, stdout=out, stderr=err)
        try:
            deadline = time.monotonic() + 30
            while not checkpoint.exists():
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError("acquisition did not reach a live checkpoint")
                time.sleep(0.005)
            suspended = True
            devices.mapper_action(mapped, "suspend")
            # Confirm the dedicated reader is actually blocked in the kernel,
            # rather than signalling between operations or during a seal.
            deadline = time.monotonic() + 5
            while True:
                blocked = False
                for task in Path(f"/proc/{process.pid}/task").glob("*"):
                    try:
                        blocked |= (task.joinpath("comm").read_text().strip() == "ewf-source-read"
                                    and "State:\tD" in task.joinpath("status").read_text())
                    except FileNotFoundError:
                        pass
                if blocked:
                    break
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError("source worker did not enter a blocked kernel read")
                time.sleep(0.005)
            if cancel:
                process.send_signal(signal.SIGINT)
            # Linux teardown can itself wait for uninterruptible I/O. Require
            # the completed CLI report while stalled, then release the mapper.
            deadline = time.monotonic() + 10
            while report is None:
                try:
                    report = json.loads(stdout.read_text())
                except json.JSONDecodeError:
                    if time.monotonic() > deadline:
                        raise AssertionError("CLI did not report while its source read was stalled") from None
                    time.sleep(0.01)
        finally:
            try:
                if suspended:
                    devices.mapper_action(mapped, "resume")
            finally:
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
                    raise
    code = 130 if cancel else 1
    assert process.returncode == code and report["exit_code"] == code, report
    assert report["source_read_stop"] == ("cancelled" if cancel else "timeout"), report
    assert report["substituted_sectors"] == 0 and not report["published"], report
    assert 0 < report["checkpoint_bytes"] <= report["accepted_bytes"] < report["source"]["size"], report
    cli(args.binary, root, ["checkpoint", "validate", output])
    done = cli(args.binary, root, ["resume", output, "--read-timeout-ms", "10000"])
    assert done["verification"]["sha256"] == expected, done
    oracle(args, output, expected)
    history = history_report(args, root, output, expected)
    assert history["runs"][0]["result"]["source_read_stop"] == ("cancelled" if cancel else "timeout"), history


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--ewfexport", type=Path, required=True)
    parser.add_argument("--ewfverify", type=Path, required=True)
    parser.add_argument("--directory", type=Path, default=Path("/var/tmp"))
    args = parser.parse_args()
    if not __debug__:
        parser.error("assertions must remain enabled; do not run with -O or PYTHONOPTIMIZE")
    if os.geteuid() != 0:
        parser.error("root is required to allocate disposable loop devices and mounts")
    for tool in ["losetup", "dmsetup", "mount", "umount", "mkfs.ext4", "setpriv", "findmnt"]:
        if not shutil.which(tool):
            parser.error(f"missing tool: {tool}")
    args.binary = args.binary.resolve(strict=True)
    args.ewfexport = args.ewfexport.resolve(strict=True)
    args.ewfverify = args.ewfverify.resolve(strict=True)
    for tool in [args.ewfexport, args.ewfverify]:
        version = subprocess.run([str(tool), "-V"], capture_output=True, text=True, check=True)
        if "20260924" not in version.stdout + version.stderr:
            parser.error("this gate requires libewf 20260924")
    root = Path(tempfile.mkdtemp(prefix="ewf-device-accept-", dir=args.directory)).resolve()
    root.chmod(0o755)
    devices = Devices()
    completed = []
    try:
        source = root / "source.raw"
        block = bytes((n * 31 + n // 256) % 251 for n in range(65536))
        with source.open("wb") as stream:
            for _ in range(256):
                stream.write(block)
            stream.flush()
            os.fsync(stream.fileno())
        source.chmod(0o600)
        expected = digest(source)
        for sector in [512, 4096]:
            device = devices.attach(source, sector)
            output = root / f"sector-{sector}.E01"
            wrong = cli(args.binary, root, ["acquire", device, root / f"wrong-{sector}.E01", "--sector-size", 4096 if sector == 512 else 512], 1)
            assert "geometry" in wrong["error"], wrong
            paused = cli(args.binary, root, ["acquire", device, output, "--stop-after", 4 * 1024 * 1024, "--chunks-per-segment", 32], 130)
            assert paused["checkpoint_bytes"] == 4 * 1024 * 1024
            devices.detach(device)
            cli(args.binary, root, ["resume", output], 1)
            cli(args.binary, root, ["checkpoint", "validate", output])
            replacement = root / f"replacement-{sector}.raw"
            shutil.copyfile(source, replacement)
            devices.attach(replacement, sector, device=device)
            changed = cli(args.binary, root, ["resume", output], 1)
            assert "identity" in changed["error"]
            devices.detach(device)
            devices.attach(source, sector, device=device)
            done = cli(args.binary, root, ["resume", output])
            assert done["status"] == "complete"
            assert done["verification"]["sha256"] == expected
            assert done["source"]["sector_size"] == sector
            oracle(args, output, expected)
            history_report(args, root, output, expected)
            assert digest(source) == expected
            completed.append(f"{sector}-byte device: geometry, pause, removal, replacement rejection, resume, source preservation, libewf")
            # A real permission failure must not be substituted or create state.
            denied = cli(args.binary, root, ["acquire", device, root / f"denied-{sector}.E01", "--zero-fill"], 1,
                         prefix=("setpriv", "--reuid", "65534", "--regid", "65534", "--clear-groups"))
            assert denied["accepted_bytes"] == 0
            assert "permission" in denied["error"].lower(), denied
            devices.detach(device)

        # A DM error target provides genuine kernel read errors over owned data.
        loop = devices.attach(source)
        sectors = source.stat().st_size // 512
        mapped = devices.mapper(f"0 1 linear {loop} 0\n1 1 error\n2 {sectors - 2} linear {loop} 2\n")
        output = root / "bad-sector.E01"
        stopped = cli(args.binary, root, ["acquire", mapped, output, "--retries", 0], 1)
        assert stopped["substituted_sectors"] == 0
        done = cli(args.binary, root, ["resume", output, "--zero-fill", "--retries", 0], 4)
        assert done["acquisition_errors"] == [{"first_sector": 1, "sector_count": 1}], done
        actual = bytearray(source.read_bytes())
        actual[512:1024] = bytes(512)
        oracle(args, output, hashlib.sha256(actual).hexdigest())
        history_report(args, root, output, hashlib.sha256(actual).hexdigest())
        assert digest(source) == expected
        completed.append("kernel bad-sector I/O: default stop, explicit substitution, native range, libewf")

        stalled = devices.mapper(f"0 {sectors} linear {loop} 0\n")
        for cancel in [False, True]:
            stalled_device(args, root, devices, stalled, expected, cancel)
        assert digest(source) == expected
        completed.append("suspended kernel reads: timeout and SIGINT, no substitution, checkpoint validation, verified resume, libewf")

        # Mount only a newly created filesystem image to test overlap rejection.
        filesystem = root / "filesystem.raw"
        with filesystem.open("wb") as stream:
            stream.truncate(64 * 1024 * 1024)
        command("mkfs.ext4", "-q", "-F", filesystem)
        volume = devices.attach(filesystem, readonly=False)
        mountpoint = root / "mounted"
        mountpoint.mkdir()
        devices.mount(volume, mountpoint)
        collision = cli(args.binary, root, ["acquire", volume, mountpoint / "case.E01"], 1)
        assert "destination" in collision["error"]
        assert not list(mountpoint.glob(".case*"))
        completed.append("destination on source volume rejected before output creation")

        # Actual ENOSPC on a private tmpfs, followed by capacity growth and resume.
        full = root / "full"
        full.mkdir()
        devices.mount("tmpfs", full, "-t", "tmpfs", "-o", "size=4m")
        export_image = root / "export-source.E01"
        cli(args.binary, root, ["acquire", source, export_image])
        export_target = full / "export.raw"
        failed_export = cli(args.binary, root, ["export", export_image, export_target], 1)
        assert failed_export["phase"] == "export", failed_export
        assert not failed_export["published"]
        assert failed_export["verification"] is None
        assert not export_target.exists()
        assert not list(full.glob(".ewf-export-*"))
        cli(args.binary, root, ["verify", export_image])
        completed.append("strict raw export: real filesystem ENOSPC, no published partial, temporary cleanup, preserved source")
        output = full / "full.E01"
        failed = cli(args.binary, root, ["acquire", source, output, "--compression", "raw", "--chunks-per-segment", 16], 1)
        assert 0 < failed["checkpoint_bytes"] < source.stat().st_size
        command("mount", "-o", "remount,size=64m", full)
        exported = cli(args.binary, root, ["export", export_image, export_target])
        assert exported["verification"]["sha256"] == expected
        assert digest(export_target) == expected
        done = cli(args.binary, root, ["resume", output])
        assert done["verification"]["sha256"] == expected
        oracle(args, output, expected)
        history = history_report(args, root, output, expected)
        assert len(history["runs"]) == 2, history
        assert history["runs"][0]["status"] in ("failed", "interrupted"), history
        completed.append("persistent history: stops, policy changes, disk-full interruption, consolidated verified report")
        completed.append("real filesystem ENOSPC, retained checkpoint, capacity expansion, verified resume")
    finally:
        try:
            devices.close()
        except Exception:
            print(f"Cleanup incomplete; retained owned workspace: {root}", file=sys.stderr)
            raise
        else:
            shutil.rmtree(root)
    print(json.dumps({"schema_version": 1, "status": "passed", "checks": completed}, indent=2))


if __name__ == "__main__":
    main()
