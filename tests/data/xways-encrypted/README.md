# X-Ways encrypted EWF1 fixtures

The four `compatible` and `zstd` images were created by X-Ways Forensics 21.9
Beta 2 from the same deterministic 1 MiB source. They cross AES-128/AES-256
with compatible Deflate and X-Ways Zstandard compression. Imaging reports are
intentionally excluded because they contain workstation and license metadata.

The source media hashes are:

- MD5: `C31109DB97E3B19811D76C4AA06C9142`
- SHA-256: `A4A3EC30D6388244DED06C36C1C7F501EE42FD7940C04350C6E367A644220313`

The authentic fixture password is not stored in this repository. The
`known-password` files retain the authentic compatible EWF1 structure and
compressed media but were independently re-encrypted with the public test
password `xways-test`. They provide deterministic end-to-end vectors for both
counter variants without publishing the authentic fixture password. They are
reference test vectors, not claims of byte-for-byte X-Ways output.

Native uncompressed and verifier-less images were unavailable in the tested
X-Ways imaging workflow. The integration tests derive a verifier-less variant
in a temporary file to exercise structural validation.

## SHA-256

- `aes128-compatible.E01`: `626D864EA4186927B0235CCBD3A701E7A2678AB821019E9FFD23B7FFFBBCE49A`
- `aes128-zstd.E01`: `5D7B2CF4E95E2E63FF5F16FCE632AE885824517E5D7E897C014F12C880A23AAB`
- `aes256-compatible.E01`: `D05B99A1018EF282BFF0F61D7349AC11CF015B86D39631AD97AC7C2CBEF4795B`
- `aes256-zstd.E01`: `A10CF2CCEF4E228706C277ED0E7E42FC14EFE0FEA3F6EF77D358939B912224B3`
- `aes128-known-password.E01`: `67B140B2C033D95898E43416317DA6FBCD5EA411254D29C3CE2BD74CD293270E`
- `aes256-known-password.E01`: `90954331DE204AF4081FAB20C88B481415D3E137EB85AC1570CE44AB70A08FD9`
