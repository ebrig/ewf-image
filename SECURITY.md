# Security policy

`ewf-image` and the experimental `aff4-image` parse untrusted forensic images.
Report any malformed input that causes incorrect output, panics, resource
exhaustion, or unsafe behavior in downstream applications.

## Supported versions

| Version | Support |
| --- | --- |
| Current development branch | Fixes land here; report the exact commit |
| Published EWF packages | Report the package version; no separate backport branch is designated |
| Experimental AFF4 packages | Reports accepted; API may change between 0.x releases |

A fix on the development branch is not necessarily present in a published
package. Check the [changelog](CHANGELOG.md) against the version you use.

## Report privately

Use the repository's [private vulnerability reporting
form](https://github.com/ebrig/ewf-image/security/advisories/new) when it is
available. Do not open a public issue that contains exploit details or private
evidence. If private reporting is unavailable, open a minimal issue that requests
a private contact channel. Omit technical details until a maintainer responds.

Include the affected version or commit, the API or command involved, the observed
impact, and the smallest input you can safely share. Remove unrelated case data,
passwords, recovered content, and personal information. Keep the original
evidence privately if it is needed to establish provenance.

## Response process

Security reports take priority over feature work. Each confirmed issue receives
a fix, regression coverage, and a changelog entry. Disclosure is timed to give
users a reasonable window to update. The maintainer aims to acknowledge reports
within 14 calendar days.

Internal hash matches do not authenticate evidence. Keep independent references,
source provenance, and verification scope with acquisition records. Resource
budgets and bounded fuzz runs reduce risk, but they do not establish that every
input or storage environment is safe.

AFF4 operations in `ewf-cli` rely on operating-system memory and disk limits
instead of library resource quotas. Structural checks still apply, but memory exhaustion can
terminate the process. Applications that require resource isolation should use
the explicit library limits together with appropriate operating-system controls.
