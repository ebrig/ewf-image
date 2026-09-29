# Unified evidence CLI

`ewf-cli` combines EWF, AFF4, and raw-image operations in one executable. The
name is provisional. Building it requires Rust 1.96 or later:

```sh
cargo build -p ewf-cli --release --locked
```

The executable is `target/release/ewf-cli` (`ewf-cli.exe` on Windows). To install
it instead, run `cargo install --path crates/ewf-cli --locked` from this checkout.

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

Run `<command> --help` to list a command's options. By default, each command
prints a short human-readable summary. `--json` prints a machine-readable result
on stdout instead. Progress is written to stderr, and `--quiet` suppresses
progress without suppressing the result.

Supported encrypted X-Ways EWF1 images can be inspected, verified, exported, or
converted using `--password-file PATH`. Use `-` to read from stdin. The input is
1 to 32 bytes, with one trailing LF or CRLF removed. The password is not put in
command arguments or JSON results. This option applies only to EWF image reads;
encrypted output and encrypted EWF2 remain unsupported.

The output filename selects the format: `.E01`, `.Ex01`, `.aff4`, `.raw` (or
`.dd`, `.img`, `.bin`), or `.Lx01` for a logical collection. Write EWF output
extensions in the case shown. EWF and AFF4 input containers are detected by
signature. Raw input requires a recognized raw extension because raw images have
no identifying header. A renamed container is still decoded as a container.

## Acquisition and conversion

Each acquisition writes one destination. Windows examples:

```powershell
ewf-cli acquire '\\.\PhysicalDrive2' case.aff4 --case-number CASE-123
ewf-cli acquire '\\.\PhysicalDrive2' case.Ex01
```

Linux example:

```sh
sudo ewf-cli acquire /dev/sdb case.E01
```

For a healthy disk where throughput matters more than fine read-error
localization, `ewf-cli ewf acquire` accepts `--bulk-read-bytes 262144`. The
resumable E01 default remains one 32 KiB image chunk per bulk attempt. A failed
larger attempt is discarded and re-read sector by sector; cancellation cannot
accept a partially completed attempt. Use `--compression raw` to store E01
chunks without zlib when the source data compresses poorly.
`--compression zlib-fast` uses the faster zlib level for E01 and Ex01. It can
reduce encoder time while producing larger images; the selected mode is retained
for E01 resume. Compare both elapsed time and stored size on representative data.

AFF4 acquisition accepts `--compression stored|zlib|snappy|lz4` and
`--chunk-bytes BYTES`; its defaults remain zlib and 32 KiB. Physical chunk size
must be sector aligned and at most 16 MiB. The CLI adjusts chunks per bevy so
the uncompressed bevy stays within 128 MiB. Raw output has no compression or
chunk settings. These choices change container encoding, not source bytes, and
the CLI still verifies the destination before reporting success.
JSON results include `timings` for streaming and finalization or verification
phases. These wall times help compare settings on the same host, but device and
filesystem caches still affect them.

Device acquisition uses a read-only Windows and Linux device adapter with
geometry, identity, and destination-overlap checks.
Administrator or root access may be required. Other platforms support
regular-file sources only. Acquisition does not freeze a live disk, so use a
stable source or a snapshot. The automated test suite does not exercise AFF4 or
raw acquisition from a physical device.

| Source | Supported destinations |
| --- | --- |
| Physical device or regular source file (`acquire`) | E01, Ex01, AFF4 physical, raw |
| Physical EWF, one AFF4 physical volume, raw (`convert`) | E01, Ex01, AFF4 physical, raw |
| Logical EWF or AFF4 logical collection (`convert`) | Lx01, AFF4 logical |
| Directory (`collect`) | Lx01, AFF4 logical |

`--sector-size BYTES` supplies the geometry for raw files and for AFF4 images
that do not record it. The default in those cases is 512 bytes. When an image
records its geometry, any override must agree with it. Supported physical sector
sizes are 512, 1024, 2048, and 4096 bytes. Physical input must be nonempty and
sector-aligned. AFF4 output records the sector size independently of its
compression-chunk size.

An AFF4 container with multiple physical disks requires `convert --resource ID`.
Run `info` to list the resource IDs. Conversion accepts one AFF4 volume. Use
`verify-set` to verify a multi-volume set. Conversion always copies decoded disk
contents and never substitutes the encoded ZIP or EWF container bytes.

Conversion checks the available source integrity references, hashes the
transferred bytes, checks source consistency, and verifies the destination. Raw
and AFF4 outputs are verified before publication. EWF output is published
transactionally and then reopened for verification. If that verification fails,
check the `published` field. A raw input without a separately recorded digest can
establish that the copy matches the input, but not that the source is authentic.

Common case fields are mapped where possible. Logical conversions keep file
bytes, hierarchy, empty folders, and supported timestamps. Lx01 stores timestamps
in whole seconds. Resource identifiers, integrity graphs, and format-specific
metadata are regenerated or omitted. The `metadata_not_preserved` field lists
these omissions, and exit code 4 signals them to scripts. Conversion refuses
logical substreams and unsupported entry or hierarchy types rather than silently
dropping their bytes. Raw output cannot store case metadata or acquisition-error
ranges. Keep the conversion report with the evidence when it records either.

```text
ewf-cli convert case.E01 converted.aff4 --json
ewf-cli convert converted.aff4 exported.raw --json
ewf-cli collect snapshot-directory files.Lx01
ewf-cli convert files.Lx01 files.aff4 --json
```

Every command requires a new output path and never overwrites an existing
destination. E01 acquisition supports checkpoints, resume, and acquisition
history. Ex01, AFF4, and raw acquisition and all conversions run in a single pass
and cannot be resumed.

E01, Ex01, Lx01, and AFF4 conversion stream payloads with scratch bounded by
segment or bevy size. E01 conversion uses `SequentialWriter`, while the general
`EwfWriter` still spools full images for positioned writes. Metadata and catalog
memory grows with the input size. Final segment writing and publication have no
cancellation callback; a pending cancellation takes effect during the following
verification step.

The CLI has no `--memory-limit` or resource-budget options. The CLI uses internal
streaming buffers and sets AFF4 metadata and verification limits from platform
capacity. Library defaults and format validation are unchanged. The CLI does not
preallocate all available RAM or automatically use every CPU core.

## Inspecting and checking

`verify IMAGE` checks the references stored in the container. A raw file has no
stored references, so it requires an independently recorded `--sha256 HASH`.
Without that option, `verify` reports the computed digest of a raw file and exits
with code 4. `--sha256` also compares decoded EWF media or the only physical disk
in an AFF4 container. Select an AFF4 resource explicitly when the container holds
more than one. The report states the scope of a selected-resource check
separately from whole-container verification.

For whole-image EWF verification, `--workers 1..64` enables bounded parallel
chunk decoding; the default is one worker. Compressed EWF2 images may benefit,
but storage and codec determine throughput. Selected-file, AFF4, and raw checks
do not accept this option.

```text
ewf-cli verify case.E01 --sha256 HASH
ewf-cli verify case.Ex01 --workers 4
ewf-cli verify case.aff4 --metadata-sha256 HASH
ewf-cli verify-set primary.aff4 companion.aff4 --image RESOURCE-ID --full
ewf-cli metadata case.aff4
ewf-cli files files.Lx01 --offset 0 --limit 100
ewf-cli verify files.Lx01 2
ewf-cli extract files.Lx01 2 selected.bin
ewf-cli extract files.Lx01 2 selected.bin --restore-times
```

EWF file selectors are the preorder catalog indices shown by `files`. AFF4
selectors are resource IDs. Extraction checks the stored file references before
publishing the output. Missing or unsupported references are reported. Catalog
names never become extraction paths on the host. The user supplies the
destination path.
`--restore-times` sets the recorded access and modification times on the new
file before publication and reports which fields were applied. EWF times have
whole-second precision; AFF4 can retain nanosecond precision. This does not
restore ACLs, xattrs, alternate streams, or directory metadata.

The EWF diagnostic commands `analyze`, `recover`, `resume`, `checkpoint inspect`,
`checkpoint validate`, `recover-publication`, and `report` are also available at
the top level. The top-level commands use safe defaults. For advanced
acquisition and recovery options, use `ewf-cli ewf <command>`. The
[EWF command guide](cli.md) describes both.

## Results

| Exit | Meaning |
| --- | --- |
| 0 | Requested operation completed with its stated verification scope |
| 1 | Operational failure |
| 2 | Invalid command syntax |
| 3 | Verification mismatch or EWF analysis errors |
| 4 | Incomplete references, substitutions, metadata omissions, or other findings |
| 130 | Cancelled |

JSON results include `schema_version`, `tool`, `tool_version`, `status`,
`exit_code`, `elapsed_seconds`, and operation-specific fields. `published` is
false before publication and true after confirmed publication. `published` is
null when an EWF publication outcome must be resolved. A nonzero exit code does
not by itself mean that no output was created. Matching internal hashes do not
authenticate the source.

`ewf-cli` is the only command-line executable. It prints human-readable output
by default, so scripts should add `--json`.
