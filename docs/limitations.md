# Limitations

Use this guide to choose supported workflows. [Compatibility](compatibility.md)
lists implemented profiles and independent-tool coverage.
AFF4 profiles and limits are described in the
[AFF4 guide](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image).

## Formats and encryption

| Capability | Boundary |
| --- | --- |
| Encrypted EWF2 | Detected and rejected; no decryption or encrypted writing |
| X-Ways encrypted EWF1 | Library and CLI read AES-128/AES-256 CTR with a supplied password; no encrypted output |
| X-Ways Zstandard | Read-only; copying that encoding into a writer is rejected |
| Delta/shadow overlays | No base-plus-overlay reading or writing |
| EWF2 BZip2 | Local reader/writer support; limited independent-tool coverage |

Secondary or shadow **mirroring** produces a byte-identical segment set that can
be read independently. Mirroring does not implement mutable overlays. Resuming
incomplete EWF1 output with `EwfWriter::resume` is a separate operation that
rewrites the image.

Native X-Ways fixtures cover compressed, single-segment images that include a
password verifier. Native uncompressed, verifier-less, and split encrypted
fixtures are unavailable. Derived tests exercise structural validation of
verifier-less images. AES-CTR does not authenticate media, and password
validation does not replace integrity checks.

Logical catalogs that contain entry names with unpaired UTF-16 surrogates can be
read, and `SingleFileEntry::name_utf16` keeps the original units. Writers reject
these names before publication, so such catalogs cannot be copied into new
logical output unchanged.

## Writer resources and recovery

`EwfWriter` keeps full raw and encoded spools. `SequentialWriter` limits payload
scratch space to one segment, but catalogs and output paths grow with their
counts. Staged output occupies the size of the final image. Replacing existing
output can require space for the new set and for backups, in addition to the spools.

`AcquisitionWriter` supports known-size, sector-aligned physical E01 output with
raw or zlib compression, one destination, and a stable identity supplied by the
caller. `AcquisitionWriter` requires hard links and file locking. Resume
revalidates and rehashes the sealed prefix. Resume does not continue images from
other producers, store hash-library internal state, or check that the live
source bytes are unchanged. EWF1 bad-sector addresses are limited to 32 bits.

Sequential EWF2 output does not support seeking, checkpoint resume, or segment
limits based on encoded size. Segment sizing uses raw chunk capacity. Library
sequential output can mirror and replace existing output. The one-shot CLI
commands create new output only and have no mirror option. See
[writer selection](acquisition.md#choose-a-writer).

File-backed finishes in the general and sequential writers use a recoverable
publication journal. Replacing existing output requires
`overwrite_existing = true`, which `EwfWriter::resume` enables. Destinations
owned by the caller through `Write` do not use the transaction. After an
interruption, call `EwfWriter::recover_output(first, secondary)` with the actual
paths. `recover_output` refuses to run while a writer is active or when the
secondary paths do not match. Recovery resolves the transaction but does not
check media integrity.

A finish error can occur after the transaction commits. Opening an image by path
rejects a pending journal, but other applications do not recognize these locks
or journals. Close readers before replacing the files they use. Persistent empty
`.lock` files coordinate writers. Do not remove them while the output is in use.
An interrupted cleanup can leave inert `.ewf-cleanup-*` directories.

## Durability and cancellation

Staged and backup files are synchronized, and directory entries are also
synchronized on Unix. Directory synchronization is not implemented on Windows.
Process-exit and I/O-failure tests do not certify behavior under power loss, on
network filesystems, or on storage that ignores flushes.

CLI source reads support cancellation and optional deadlines. On Windows, native
cancellation is attempted on a best-effort basis. Discovery, identity queries,
destination I/O, sealing, and verification are not covered by the deadline.
Drivers may keep reads pending or delay process teardown. The stopped reader owns
its buffers and handle and cannot write acquired output. Library cancellation is
cooperative and occurs between operations.

## Collection and extraction

The `ewf-cli ewf collect` command records regular files and directories, Unicode
names, and available basic timestamps. `collect` rejects links, reparse points,
special or inaccessible entries, and names that contain catalog delimiters.
Collection is limited to 100,000 entries and 127 directory levels. Neither
`collect` nor the AFF4 portable collector creates a snapshot that is consistent
across files. Collect from a stable snapshot when consistency is required.

EWF collection does not capture ACLs, ADS, xattrs, or sparse allocation.
Selective extraction can restore recorded access and modification times when
requested. It does not restore ACLs, xattrs, ADS, Unix mode, directory metadata,
or special objects. General filesystem mounting, service runtimes, and
native FFI wrappers are not implemented.

## Analysis and recovery

Analysis can continue after individual chunk errors. A structural failure while
opening the image produces a single finding with unavailable coverage. Analysis
does not resynchronize a broken descriptor chain or enumerate the defects that
the break makes inaccessible.

Recovery supports physical, unencrypted raw or zlib EWF1 images with reliable
geometry and separate sectors sections. Recovery excludes logical, SMART, EWF2,
encrypted, Zstandard, and table-resident images. Missing descriptors, missing
table headers, and ambiguous missing middle segments are not reconstructed.
Substituted and checksum-suspect bytes are marked explicitly in the provenance
map and are not verified source data. See
[recovery semantics](reader-analysis.md#damaged-image-recovery).

## Device and consumer acceptance

Device acquisition requires discoverable geometry, stable identifiers, and
resolvable destination storage. Windows overlap checks cannot detect controller,
SAN, or virtual-storage relationships that are not reported in volume disk
extents. Device acquisition does not lock or dismount a live filesystem or
provide a snapshot.

Physical hot-unplug, actual bad media, hardware write blockers, and power loss
require separate acceptance testing. EnCase consumer import/export has not been
tested. The EWF2 logical writer requires chunks of at least 8 KiB: smaller
split output crashed the pinned libewf 20260924 exporter. Split output at 8 KiB
and the default 32 KiB passed that exporter. An older libewf 20251220 exporter
still omitted a file from a one-file split image even at 32 KiB; its behavior
needs separate investigation. See
[device acceptance](device-acceptance.md).
