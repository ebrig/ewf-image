# Security Policy

`ewf-image` parses attacker-controlled forensic image files. Reports involving
malformed input are treated seriously, even when the immediate impact appears
limited to local tooling.

## Supported Versions

| Version | Supported |
| --- | --- |
| 0.4.x | Yes |
| 0.3.x and earlier | No |

## Reporting a Vulnerability

Do not open a public issue with exploit details. Use this repository's
[private vulnerability reporting form](https://github.com/ebrig/ewf-image/security/advisories/new).

Please include:

- Affected version or commit.
- The smallest reproducing input you can share.
- The API or example command that triggers the issue.
- Whether the issue causes panic, incorrect output, resource exhaustion, or
  unsafe behavior in downstream applications.

Do not attach confidential case evidence or other sensitive forensic material.
Describe the material first so the maintainer can arrange an appropriate
transfer method if it is needed.

## Handling Expectations

The maintainer aims to acknowledge reports within 14 calendar days. Security
reports are triaged before ordinary feature work. Confirmed issues receive a
fix, regression coverage, and a changelog entry. Disclosure timing is
coordinated with the reporter so users have a reasonable update window.
