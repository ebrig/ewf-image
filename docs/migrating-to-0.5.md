# Migrating to 0.5

This guide covers changes since 0.4.0. The repository source is version 0.5.0,
which has not been published. See [the changelog](../CHANGELOG.md#unreleased)
for the full list of changes.

## Update digest handling

`VerifyResult` adds `computed_sha256` and `sha256_match`, and `WriteResult` adds
`computed_sha256`. Update any code that constructs these structs directly.
Verification now compares recognized stored SHA256 references in addition to MD5
and SHA1. Malformed or conflicting supported references are rejected, including
when an image is opened leniently. Unknown digest identifiers remain available as
metadata and are not verified.

Complete EWF1 output embeds the computed SHA256 unless an explicit reference was
provided. EWF2 output returns the SHA256 but does not embed a SHA256 section, so
record the digest externally. Explicit valid reference hashes can intentionally
differ from the computed media hash. See
[digest semantics](compatibility.md#writer-digest-semantics).

## Handle logical entry names

`SingleFileEntry` adds the public `name_utf16` field. Update struct literals that
do not use `..Default::default()`. The field is set only when an entry name
contains unpaired UTF-16 surrogates. In that case, `name` is a display string with
replacement characters, and two different names can display alike. Use
`name_utf16` or the catalog index to tell such entries apart. Writers reject
entries whose `name_utf16` is set instead of re-encoding the name.

## Choose publication and resume behavior

File-backed `EwfWriter` output now refuses existing segments by default. Set
`WriteOptions::overwrite_existing = true` to replace existing output
intentionally. `EwfWriter::resume` enables replacement and still rewrites an
incomplete EWF1 image.

Resolve an interrupted publication from the general or sequential writer with
`EwfWriter::recover_output(first, secondary)`, using the actual first-segment
paths. A finish error can occur after publication, so resolve the journal before
using the output. Destinations owned by the caller through `Write` remain the
caller's responsibility.

Use `AcquisitionWriter` for append-only physical E01 acquisition.
`AcquisitionWriter` checkpoints sealed segments and resumes without rewriting
them. It requires a known, sector-aligned size, a stable source identity, raw or
zlib compression, and hard-link support. Resume validates and rehashes the entire
sealed prefix.

For known-length E01 or EWF2 output, use `SequentialWriter` or
`LogicalWriter::create_sequential`. These writers bound payload scratch space but
cannot resume. Split Lx01 catalogs are written in the final segment, so open the
complete set. See [acquisition](acquisition.md) for resource use and recovery
requirements.

## Adopt the CLI

For combined EWF and AFF4 operations, build the provisional `ewf-cli` with
`cargo build -p ewf-cli --release --locked`. `ewf-cli` selects the acquisition
output format from the filename and supports physical and logical conversion.
See the [unified CLI guide](ewf-cli.md). The standalone `ewf-image` and
`aff4-image` executables have been removed.

Use `ewf-cli ewf <command>` for advanced acquisition, checkpoint inspection,
verification, export, logical-file access, analysis, and recovery. The
[EWF command guide](cli.md) covers these controls. `ewf-cli` prints text by
default, so add `--json` in scripts, including when redirecting stdout. EWF JSON
schema version 1 allows fields to be added.

Use `ewf-cli verify IMAGE ENTRY` and `ewf-cli extract IMAGE ENTRY OUTPUT` for
logical files. The `ewf-cli ewf verify-file` and `ewf-cli ewf extract-file`
commands remain accepted for compatibility. The one-shot EWF2 commands report `published` as `false`, `true`, or `null`, and a
`null` value means the transaction must be recovered. EWF acquisition and
collection verify output after publication, so a failed verification can leave a
published image.

For encrypted EWF1 input, provide `--password-file PATH` or
`--password-file -` to read a password from stdin. Extraction can apply recorded
file access and modification times with `--restore-times`.

Windows device sessions created by the earlier PowerShell identity adapter must
be completed with the original binary, because the native adapter uses different
checkpoint identity tokens. File-source and Linux identities are unchanged.

## AFF4 callers

AFF4 support is an experimental sibling crate with its own version. Its API may
change between 0.x releases. Update struct literals for the new resource-limit
fields and exhaustive matches for the new error variants. `add_directory_tree`
applies the default `CollectionLimits`.
Use `add_directory_tree_with_limits` to override them, and pass the same reader
limits to `finish_verified`.

`finish_verified` checks the finalized staging file before publication, and
`finish` does not verify. `Error::VerificationFailed { report }` preserves the
available diagnostics. `Error::PublishedButUnsynced` means the output exists but
synchronizing the parent directory failed. Preserve the reported path and
digests, and inspect the result before use.

The library ZIP and metadata budgets can reject inputs that earlier versions
accepted. AFF4 operations in `ewf-cli` set limits from platform capacity instead
of application quotas, and the `--limit-*` options of the former `aff4-image`
executable have no replacement. `ewf-cli` does not impose a percentage-of-RAM
limit. The default library limits are unchanged.
See the [AFF4 guide](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image).
