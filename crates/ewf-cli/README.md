# ewf-cli

A single command-line tool for the `ewf-image` and `aff4-image` libraries. The
name is provisional. Build it from this workspace with Rust 1.96 or later:

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

Each acquisition writes one output, and the output extension selects the format.
Results are printed as concise text. Add `--json` for scripts and `--quiet` to
hide progress. The [unified CLI guide](../../docs/ewf-cli.md) covers device
paths, supported conversions, verification scope, metadata omissions, and exit codes.

`ewf-cli` is the only command-line executable. Use `ewf-cli ewf <command>` for
advanced EWF acquisition and recovery controls. The EWF command runtime and
native device adapter run in this process. Each format crate keeps its own
library API and dependency graph.
