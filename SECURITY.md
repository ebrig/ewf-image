# Security Policy

`ewf` parses attacker-controlled forensic image files. Reports involving
malformed input are treated seriously, even when the immediate impact appears
limited to local tooling.

## Supported Versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |

## Reporting A Vulnerability

Do not open a public issue with exploit details. Use GitHub private
vulnerability reporting for this repository when available. If private
reporting is unavailable, open a minimal public issue asking for a private
contact path and omit technical details until a maintainer responds.

Please include:

- Affected version or commit.
- The smallest reproducing input you can share.
- The API or example command that triggers the issue.
- Whether the issue causes panic, incorrect output, resource exhaustion, or
  unsafe behavior in downstream applications.

## Handling Expectations

Security reports are triaged before ordinary feature work. Confirmed issues
receive a fix, regression coverage, and a changelog entry. Public disclosure
timing should give users a reasonable update window.
