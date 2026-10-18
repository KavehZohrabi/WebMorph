# Security policy

WebMorph is an early-stage project. Please report suspected vulnerabilities
privately rather than posting exploit details in a public issue.

Use GitHub's **Report a vulnerability** action for this repository when it is
available. If private vulnerability reporting is unavailable, contact the
maintainers through the repository's GitHub contact route and ask for a private
security channel before sending technical details. Do not include real
credentials, cookies, or private browsing data in a report.

Please include the affected version or commit, operating system, reproduction
steps, expected impact, and a minimal proof of concept. We will acknowledge
reports when practical and coordinate disclosure after a fix is available.

## Security scope

The lightweight engine is intended for public HTTP/HTTPS content. It applies
URL, destination, redirect, timeout, response-size, and content-type checks and
sanitizes terminal control characters. It does not execute downloaded scripts.
The optional Chromium backend is a separate, explicitly selected feature with
its own dependency and runtime surface. Neither engine should be treated as a
replacement for a hardened browser or as safe for arbitrary untrusted local
networks.

No security response-time or support-time guarantee is currently offered.
