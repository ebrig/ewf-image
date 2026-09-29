# Acquisition performance measurements

These are local measurements for choosing a format and compression setting, not
device throughput guarantees. The benchmark reads a 64 MiB regular file, writes
an image, publishes it, and verifies the decoded output. Each value is the median
of three or four release-build trials, as noted below. Paired comparisons
alternate binary order and use the same seeded random and repeating-text inputs.
Source SHA256 is checked
against each acquisition report.

Run the benchmark with:

```sh
python scripts/benchmark-physical-throughput.py --binary target/release/ewf-cli --mib 64 --trials 4
```

Use `target/release/ewf-cli.exe` on Windows. The script emits JSON containing
each trial and phase timing. `--baseline-binary PATH` adds a paired comparison
for the default raw, E01 zlib, and AFF4 zlib cases.

## Buffered-file results (2026-09-29)

The baseline is `c5e2dc2` from the main checkout. The candidate was this
performance branch before `07a16cf`. These measurements include the OS file
cache and the local destination, so small differences may be noise.

| Input | Format | Baseline | Candidate | Candidate speedup |
| --- | --- | ---: | ---: | ---: |
| Random | Raw | 0.438 s | 0.295 s | 1.48x |
| Random | E01 zlib | 2.359 s | 2.216 s | 1.06x |
| Random | AFF4 zlib | 3.032 s | 2.457 s | 1.23x |
| Repeating text | Raw | 0.397 s | 0.198 s | 2.01x |
| Repeating text | E01 zlib | 0.753 s | 0.803 s | 0.94x |
| Repeating text | AFF4 zlib | 1.919 s | 1.456 s | 1.32x |

This first comparison used three trials. A later four-trial paired comparison
isolated the AFF4 identity-map digest reuse: random AFF4 zlib fell from 2.397 s
to 2.098 s, and repeating-text AFF4 zlib from 1.506 s to 1.155 s. The two
comparisons were run separately and should not be combined into a single
baseline-to-current speedup.

The optional E01 `zlib-fast` setting was also measured over four trials:

| Input | E01 zlib | E01 zlib-fast | E01 raw | Zlib-fast stored size |
| --- | ---: | ---: | ---: | ---: |
| Random | 2.232 s | 1.895 s | 1.190 s | 64.03 MiB |
| Repeating text | 0.761 s | 0.760 s | 1.206 s | 0.81 MiB |

For repeating text, default zlib stored 0.51 MiB. Fast zlib gave no clear time
benefit on that input and produced a larger image. It remains opt-in.

## Remaining acceptance work

Physical-disk acquisition has not been benchmarked. Measure a stable, readable
source with a separate output disk and a bounded test source before extrapolating
these file results. Read throughput, write throughput, publication, and final
verification should be recorded separately. Large images, sustained writes,
read errors, and independent E01/AFF4 readers also need representative tests.
