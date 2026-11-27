# Security policy

## Supported versions

| Version | Supported |
|---|---|
| 1.x | Yes |
| < 1.0 (the Python bridge in `server/`) | No — reference code only, do not deploy |

## Reporting a vulnerability

Please do not open a public issue for a security problem.

- Preferred: GitHub's private vulnerability reporting for this
  repository (the "Report a vulnerability" button under the
  Security tab).
- If that is unavailable, open a regular issue asking for a
  private contact route — without any details of the problem.

Expect an acknowledgement within a few days. Fixes are developed
privately and released as a patch version; credit is given in
the release notes unless you prefer otherwise.

## Scope notes

winmuse operates a PC — mouse, keyboard, clipboard — by design,
for whoever can reach its endpoint within the active permission
profile. That capability itself is the product, not a
vulnerability. The reports that matter most:

- Bypassing a permission profile (`observe` / `guided`) or the
  `block_win_keys` guard
- Bypassing or leaking the bearer token, or the ESC cancel / F9
  local-arm consent being ignored
- The server binding somewhere other than the configured address
- The audit log missing calls it should have recorded

Hardening context lives in the README's Security section.
