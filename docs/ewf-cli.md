# Unified evidence CLI

`ewf-cli` combines EWF, AFF4, and raw-image operations. The name is provisional.
Build with `cargo build -p ewf-cli --release --locked`; the executable is
`target/release/ewf-cli` (`ewf-cli.exe` on Windows). From this checkout,
`cargo install --path crates/ewf-cli --locked` also works. Rust 1.96 is required.

## Everyday commands

```text
ewf-cli acquire SOURCE OUTPUT
ewf-cli convert INPUT OUTPUT
ewf-cli collect DIRECTORY OUTPUT
ewf-cli info IMAGE
ewf-cli verify IMAGE
ewf-cli files IMAGE
ewf-cli extract IMAGE ENTRY OUTPUT
```

Use `<command> --help` for its options. `--json` selects a machine-readable
result on stdout. The default is a short human-readable summary. Progress goes
to stderr; `--quiet` suppresses progress without suppressing the result.

Output filenames select the format: `.E01`, `.Ex01`, `.aff4`, `.raw` (also
`.dd`, `.img`, `.bin`), or `.Lx01` for a logical collection. Use the canonical
case shown for EWF output extensions. Input EWF/AFF4 containers are detected
by signature. Raw input requires a recognized raw extension because it has
no identifying header. A renamed container is still decoded as a container.

## Acquisition and conversion

Every acquisition writes one destination. Examples for Windows and Linux:

```powershell
ewf-cli acquire '\\.\PhysicalDrive2' case.aff4 --case-number CASE-123
ewf-cli acquire '\\.\PhysicalDrive2' case.Ex01
```

```sh
sudo ewf-cli acquire /dev/sdb case.E01
```

Device acquisition uses the existing EWF CLI's Windows/Linux read-only adapter,
including geometry, identity, and destination-overlap checks. Administrator/root
access may be required. Other platforms support regular-file sources. Acquisition
does not freeze a live disk; use a stable source or snapshot. No device was used
to validate the new AFF4/raw dispatch in the local automated test suite.

| Source | Supported destinations |
| --- | --- |
| Physical device or regular source file (`acquire`) | E01, Ex01, AFF4 physical, raw |
| Physical EWF, one AFF4 physical volume, raw (`convert`) | E01, Ex01, AFF4 physical, raw |
| Logical EWF or AFF4 logical collection (`convert`) | Lx01, AFF4 logical |
| Directory (`collect`) | Lx01, AFF4 logical |

`--sector-size BYTES` supplies raw-file geometry or missing AFF4 geometry.
The default in those cases is 512 bytes; recorded geometry must agree with any
override. Supported physical sector sizes are 512, 1024, 2048, and 4096 bytes.
Physical input must be nonempty and sector-aligned. AFF4 output records the
sector size independently of its compression-chunk size.

An AFF4 container with multiple physical disks requires `convert --resource ID`;
`info` lists IDs. Conversion currently accepts one AFF4 volume; `verify-set`
remains available for multi-volume verification. Conversion never substitutes
the encoded ZIP/EWF container bytes for decoded disk contents.

Conversion checks available source integrity references, hashes transferred
bytes, checks source consistency, and verifies the destination. Raw and AFF4
outputs are verified while staged. EWF output is published transactionally and
then reopened for verification; consult `published` if verification fails.
Raw input without a separately recorded digest can establish copy equality,
not independent source authenticity.

Common case fields are mapped where possible. Logical conversions retain file
bytes, hierarchy, empty folders, and supported timestamps. Lx01 stores timestamps
in whole seconds. Resource identifiers, integrity graphs, and format-specific
metadata are regenerated or omitted; `metadata_not_preserved` reports this and
exit code 4 makes it visible to scripts. Conversion refuses logical substreams
and unsupported entry/hierarchy types rather than silently losing their bytes.
Raw has no case-metadata or acquisition-error container. Keep the conversion
report alongside evidence when it records metadata or acquisition-error ranges.

```text
ewf-cli convert case.E01 converted.aff4 --json
ewf-cli convert converted.aff4 exported.raw --json
ewf-cli collect snapshot-directory files.Lx01
ewf-cli convert files.Lx01 files.aff4 --json
```

New output paths are required; existing destinations are never overwritten.
E01 acquisition retains the established checkpoint/resume/history workflow.
Ex01/AFF4/raw acquisition and conversion are one-shot operations. E01 conversion
uses the transactional general writer, with full disk-backed raw and encoded
scratch spools; it is not checkpoint-resumable. Ex01/Lx01 and AFF4 stream payloads
with bounded segment/bevy buffers. Metadata/catalog memory grows with input size.
Finalization in the general E01 writer is not cancellable midway; a pending
cancellation is observed by the subsequent verification step.

There are no `--memory-limit` or resource-budget switches. The CLI uses internal
streaming buffers and platform capacity for AFF4 metadata/verification limits.
Library defaults and format/structural validation remain unchanged. This does
not mean the CLI preallocates all RAM or automatically uses every CPU core.

## Inspecting and checking

`verify IMAGE` checks available container references. A raw file needs an
independently recorded `--sha256 HASH` to make a comparison. Without one it
reports its computed digest and exits 4. The same option compares decoded
EWF media or the sole AFF4 physical disk; select an AFF4 resource explicitly
when ambiguous. A selected resource's scope is reported separately from
whole-container verification.

```text
ewf-cli verify case.E01 --sha256 HASH
ewf-cli verify case.aff4 --metadata-sha256 HASH
ewf-cli verify-set primary.aff4 companion.aff4 --image RESOURCE-ID --full
ewf-cli metadata case.aff4
ewf-cli files files.Lx01 --offset 0 --limit 100
ewf-cli verify files.Lx01 2
ewf-cli extract files.Lx01 2 selected.bin
```

EWF file selectors are preorder catalog indices from `files`; AFF4 selectors
are resource IDs. Extraction checks stored file references before publishing.
Missing or unsupported references remain visible. Catalog names never become
host extraction paths; the user supplies the destination path.

EWF diagnostics remain available as `analyze`, `recover`, `resume`,
`checkpoint inspect`, `checkpoint validate`, `recover-publication`, and `report`.
See the [EWF CLI guide](cli.md) for their underlying semantics. Advanced legacy
acquisition tuning is still available through `ewf-image`; `ewf-cli` exposes
the normal workflow and uses the established safe defaults.

## Results

| Exit | Meaning |
| --- | --- |
| 0 | Requested operation completed with its stated verification scope |
| 1 | Operational failure |
| 2 | Invalid command syntax |
| 3 | Verification mismatch or EWF analysis errors |
| 4 | Incomplete references, substitutions, metadata omissions, or other findings |
| 130 | Cancelled |

JSON includes `schema_version`, `tool`, `tool_version`, `status`, `exit_code`,
`elapsed_seconds`, and operation-specific results. `published` is false before
publication, true after known publication, or null when an EWF publication
outcome needs resolution. A nonzero exit does not by itself mean no output was
created. Internal matching hashes do not authenticate the source.

The independently usable `ewf-image` and `aff4-image` executables remain
available. Their default output is also human-readable; scripts should add
`--json`. Neither legacy CLI is launched as a subprocess by `ewf-cli`.
