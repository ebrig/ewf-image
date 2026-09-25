# ewf-cli

One command line for the independent `ewf-image` and `aff4-image` libraries.
The name is provisional. Build from this workspace with Rust 1.96 or later:

```sh
cargo build -p ewf-cli --release --locked
```

```text
ewf-cli acquire disk.raw case.E01
ewf-cli acquire disk.raw case.Ex01
ewf-cli acquire disk.raw case.aff4
ewf-cli convert case.E01 case.aff4
ewf-cli convert case.aff4 disk.raw
ewf-cli collect snapshot-directory files.Lx01
ewf-cli convert files.Lx01 files.aff4
ewf-cli info case.aff4
ewf-cli verify case.aff4
ewf-cli files files.aff4
ewf-cli extract files.aff4 RESOURCE-ID selected.bin
```

Each acquisition writes **one output**. Its extension selects the format.
Results are concise text; use `--json` for scripts and `--quiet` to hide progress.
See the [unified CLI guide](../../docs/ewf-cli.md) for device paths, supported
conversions, verification scope, metadata omissions, and exit codes.

The legacy executables remain available. This unpublished workspace package
shares the existing EWF command runtime and native device adapter in process;
it does not launch either legacy executable. Format crates retain their
independent library APIs and dependency graphs.
