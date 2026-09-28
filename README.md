# ewf-image

Read, verify, and write Expert Witness Format (EWF) forensic images in pure Rust.

[![Crates.io](https://img.shields.io/crates/v/ewf-image.svg)](https://crates.io/crates/ewf-image)
[![Documentation](https://docs.rs/ewf-image/badge.svg)](https://docs.rs/ewf-image)
[![CI](https://github.com/ebrig/ewf-image/actions/workflows/ci.yml/badge.svg)](https://github.com/ebrig/ewf-image/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

![ewf-image project banner](https://raw.githubusercontent.com/ebrig/ewf-image/main/docs/assets/ewf-image-banner.png)

`ewf-image` opens EnCase-style `.E01`, `.L01`, `.S01`, `.Ex01`, and `.Lx01`
images and exposes their decoded media as ordinary Rust readers. The library
forbids unsafe code and runs without libewf or any other external tool.

- **Read** physical, logical, and SMART images, including split segment sets.
- **Verify** decoded media against stored and independently recorded MD5, SHA1,
  and SHA256 digests.
- **Browse** logical file catalogs, then verify and extract individual files.
- **Write** EWF1 and EWF2 images, with resumable E01 acquisition and bounded
  streaming writers.
- **Analyze** damaged images and recover readable data with a provenance map.

The examples below target version 0.5.0. See the
[changelog](CHANGELOG.md#050) for its changes and the
[migration guide](docs/migrating-to-0.5.md) for upgrades from 0.4.

## Installation

```toml
[dependencies]
ewf-image = "0.5"
```

The crate requires Rust 1.96 or later. Its runtime features are:

| Feature | Effect |
| --- | --- |
| `verify` | Media verification, per-file verification, and integrity analysis. Enabled by default. |
| `parallel` | Verification across multiple worker threads. Enables `verify`. |
| `serde` | Serialization of reports and metadata types. |

Stored-hash parsing, section integrity checks, and writer hashing remain available
with `default-features = false`.

## Read an image

Open the first segment. The remaining segments are found automatically.

```rust,no_run
use std::io::Read;

fn main() -> ewf_image::Result<()> {
    let image = ewf_image::Image::open("case.E01")?;
    let info = image.info();
    println!("{:?}, {} bytes in {} segments", info.format, info.logical_size, info.segment_count);

    // Read sequentially through a Read + Seek cursor.
    let mut first_sector = [0u8; 512];
    image.cursor().read_exact(&mut first_sector)?;

    // Or read at an absolute offset without a cursor.
    let mut buffer = [0u8; 4096];
    image.read_at(&mut buffer, 1024 * 1024)?;
    Ok(())
}
```

`Image` is a cheap, shareable handle. Clones and cursors share bounded caches,
so one image can serve many readers. `OpenOptions` adjusts cache sizes, handle
limits, and strictness. Segment files must remain unchanged while an image is open.

## Verify an image

`verify` decodes the complete media and compares it with the digests stored in
the image. Verification bypasses caches, so corrupt data cannot pass as valid.

```rust,no_run
use ewf_image::{Image, VerifyOptions};

fn main() -> ewf_image::Result<()> {
    let image = Image::open("case.E01")?;

    let result = image.verify()?;
    println!("MD5 match: {:?}", result.md5_match);
    println!("SHA256 match: {:?}", result.sha256_match);

    // Compare against a digest recorded outside the image.
    let acquisition_sha256 = [0u8; 32]; // Replace with the recorded value.
    let options = VerifyOptions::default().with_expected_sha256(acquisition_sha256);
    let report = image.verify_with_options(&options)?;
    println!("references match: {:?}", report.references_match());
    Ok(())
}
```

A match value of `None` means the image stores no digest of that type. A match
shows that the decoded media equals what was hashed at acquisition. It cannot
show whether unreadable source sectors were replaced with zeros at that time. See
[verification, analysis, and recovery](docs/reader-analysis.md).

## Work with logical files

Logical images (`.L01` and `.Lx01`) contain a catalog of files and folders.

```rust,no_run
use ewf_image::{Image, SingleFileEntryType};
use std::{fs::File, io};

fn main() -> ewf_image::Result<()> {
    let image = Image::open("files.L01")?;
    let Some(root) = image.root_file_entry() else {
        println!("not a logical image");
        return Ok(());
    };

    for entry in &root.children {
        println!("{} ({} bytes)", entry.name().unwrap_or("?"), entry.size().unwrap_or(0));
    }

    let first_file = root
        .children
        .iter()
        .find(|entry| entry.entry_type() == Some(SingleFileEntryType::File));
    if let Some(entry) = first_file {
        let check = image.verify_single_file(entry)?;
        println!("stored file hashes match: {:?}", check.references_match());

        let mut output = File::create_new("extracted.bin")?;
        io::copy(&mut image.single_file_cursor(entry), &mut output)?;
    }
    Ok(())
}
```

Extraction copies file content only. Timestamps and other recorded metadata
remain available on each catalog entry but are not applied to extracted files.
The `ewf-cli extract --restore-times` option applies recorded file access and
modification times to a selected new output file.

## Write an image

`EwfWriter` creates EWF1 or EWF2 images from any readable source.

```rust,no_run
use ewf_image::{EwfWriter, WriteCompression, WriteFormat, WriteOptions};
use std::{fs::File, io};

fn main() -> ewf_image::Result<()> {
    let mut options = WriteOptions {
        format: WriteFormat::Ewf2Physical,
        compression: WriteCompression::Zlib,
        ..WriteOptions::default()
    };
    options.metadata.set_header_value("case_number", "CASE-001");

    let mut writer = EwfWriter::create("case.Ex01", options)?;
    io::copy(&mut File::open("disk.raw")?, &mut writer)?;
    let result = writer.finish()?;
    println!("wrote {} segments", result.segment_paths.len());
    Ok(())
}
```

`EwfWriter` supports every output format and positioned writes, but it spools
the complete source to temporary storage. Specialized writers cover large or
long-running jobs:

- `AcquisitionWriter` acquires physical E01 images with checkpoints and can
  resume after an interruption.
- `SequentialWriter` streams known-length E01, Ex01, and Lx01 output and stages
  one segment at a time. The CLI uses it for E01 conversion.
- `LogicalWriter` builds L01 and Lx01 file catalogs from files you supply.

Writers return computed MD5, SHA1, and SHA256 digests but do not reread their
output. [Acquisition and writing](docs/acquisition.md) explains resume,
publication, and recovery for each writer.

## Supported formats

| Format | Read | Write |
| --- | --- | --- |
| EWF1 physical `.E01` | Raw and zlib; X-Ways Zstandard; X-Ways AES-128/AES-256 encryption | Raw and zlib |
| EWF1 logical `.L01` | Media and file catalog | Media and file catalog |
| EWF1 SMART `.S01` | Media | Media |
| EWF2 physical `.Ex01` | Raw, zlib, BZip2, and pattern-fill | Raw, zlib, BZip2, and pattern-fill |
| EWF2 logical `.Lx01` | Media and file catalog | Media and file catalog |

Encrypted X-Ways images open with `Image::open_with_password`. Encrypted EWF2
images, encrypted output, and delta (overlay) images are not supported.
[Compatibility](docs/compatibility.md) describes tested producers and consumers,
and [limitations](docs/limitations.md) lists unsupported workflows.

AFF4 containers are handled by the separate, experimental
[`aff4-image`](https://github.com/ebrig/ewf-image/tree/main/crates/aff4-image)
crate. `ewf-image` has no AFF4 dependencies.

## Command-line tool

The workspace includes `ewf-cli`, an unpublished command-line tool for EWF, AFF4,
and raw images. Build it from this repository:

```sh
cargo build -p ewf-cli --release --locked
```

```text
ewf-cli info case.E01
ewf-cli verify case.E01
ewf-cli acquire /dev/sdb case.E01
ewf-cli convert case.E01 case.aff4
ewf-cli collect evidence-folder files.Lx01
ewf-cli extract files.Lx01 2 recovered.bin
```

The output file extension selects the format. Commands never overwrite existing
files. Acquisition requires a stable source, and directory collection does not
create a filesystem snapshot. See the [CLI guide](docs/ewf-cli.md) for device
acquisition, conversion, JSON output, and exit codes, and the [EWF command
guide](docs/cli.md) for advanced acquisition and recovery options.

## Documentation

- [API reference](https://docs.rs/ewf-image) and [runnable examples](examples)
- [Verification, analysis, and recovery](docs/reader-analysis.md)
- [Acquisition and writing](docs/acquisition.md)
- [CLI guide](docs/ewf-cli.md) and [EWF command guide](docs/cli.md)
- [Compatibility](docs/compatibility.md) and [limitations](docs/limitations.md)
- [Architecture](docs/architecture.md)
- [Testing](docs/testing.md)
- [Release process](RELEASING.md)
- Migration guides: [0.5](docs/migrating-to-0.5.md), [0.3](docs/migrating-to-0.3.md), [0.2](docs/migrating-to-0.2.md)

## Contributing and security

Read [Contributing](CONTRIBUTING.md) before opening a pull request. Report
vulnerabilities privately as described in the [security policy](SECURITY.md).

## License

Licensed under the [Apache License 2.0](LICENSE).
