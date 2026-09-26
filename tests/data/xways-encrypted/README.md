# X-Ways encrypted EWF1 fixtures

X-Ways Forensics 21.9 Beta 2 created the four `compatible` and `zstd` images from
the same deterministic 1 MiB source. The images cover AES-128 and AES-256, each
with compatible Deflate and with X-Ways Zstandard compression. The imaging reports
are excluded because they contain workstation and license metadata.

## Source identity

All fixtures decode to the same source media, which has these hashes:

- MD5: `C31109DB97E3B19811D76C4AA06C9142`
- SHA-256: `A4A3EC30D6388244DED06C36C1C7F501EE42FD7940C04350C6E367A644220313`

## Native and derived fixtures

The password for the native fixtures is not stored in this repository. The
`known-password` files keep the native compatible EWF1 structure and compressed
media but were re-encrypted with the public test password `xways-test`. These
derived files provide deterministic end-to-end vectors for both counter variants
without publishing the native fixture password. They are reference test vectors,
not byte-for-byte X-Ways output.

The tested X-Ways imaging workflow could not produce native uncompressed or
verifier-less images. The integration tests derive a verifier-less variant in a
temporary file to exercise structural validation.

## Running the tests

The self-contained encryption tests use the public-password vectors. Native
readback requires a password supplied by the operator. See the
[testing instructions](../../../docs/testing.md#x-ways-encrypted-fixtures).
Never commit the native fixture password or the excluded imaging reports.

## Container SHA256

- `aes128-compatible.E01`: `626D864EA4186927B0235CCBD3A701E7A2678AB821019E9FFD23B7FFFBBCE49A`
- `aes128-zstd.E01`: `5D7B2CF4E95E2E63FF5F16FCE632AE885824517E5D7E897C014F12C880A23AAB`
- `aes256-compatible.E01`: `D05B99A1018EF282BFF0F61D7349AC11CF015B86D39631AD97AC7C2CBEF4795B`
- `aes256-zstd.E01`: `A10CF2CCEF4E228706C277ED0E7E42FC14EFE0FEA3F6EF77D358939B912224B3`
- `aes128-known-password.E01`: `67B140B2C033D95898E43416317DA6FBCD5EA411254D29C3CE2BD74CD293270E`
- `aes256-known-password.E01`: `90954331DE204AF4081FAB20C88B481415D3E137EB85AC1570CE44AB70A08FD9`
