# Security policy

Boring Forum runs public, user-facing communities. A vulnerability here exposes real people's
accounts, email addresses and private messages, so security reports get priority over features.

## Reporting a vulnerability

**Do not open a public issue.** Report privately, whichever is easier:

- GitHub **Security advisories** → "Report a vulnerability" (preferred: it keeps the report,
  the fix and the CVE in one place).
- Email **kontakt@riko.dev**.

Please include:

- what an attacker can do, and the rough prerequisites (anonymous, any member, staff, …);
- the smallest set of steps or a short script that shows it;
- the version or commit you tested, and the deployment shape if it matters (behind a proxy,
  self-terminated TLS, plugins installed).

A proof of concept against your own test instance is welcome. Please do not test against someone
else's forum.

## What to expect

| Stage | Target |
| --- | --- |
| Acknowledgement that a human has read it | **72 hours** |
| First assessment (severity, affected versions, plan) | 7 days |
| Fix released — critical (unauthenticated takeover, data loss) | 14 days |
| Fix released — high | 30 days |
| Fix released — medium and low | next scheduled release |
| Public advisory | with the release, or sooner if already exploited |

If a report goes quiet past these targets, send a reminder; if it stays quiet for 90 days, you
are free to disclose publicly. We will not ask for an extension without explaining why.

## Scope

In scope: this repository — the server, the admin SPA, the islands, the default theme, the plugin
host, the importers, the Docker and deployment files, and the CI workflows.

Out of scope, unless you can show real impact:

- findings that need an account you already control to attack only itself;
- missing headers or cookie flags with no exploit path (tell us anyway, in an issue);
- rate limits on endpoints that are rate-limited by design;
- volumetric denial of service — the deployment guide puts a CDN/WAF in front;
- vulnerabilities in third-party plugins or themes that are not in this repository;
- reports produced only by a scanner, with no analysis of whether the finding is reachable.

Self-hosted deployments are configured by their operators. A finding that depends on a
configuration the documentation warns against is a documentation bug, and still worth reporting.

## Supported versions

Security fixes are released for the **current and the previous minor version**. Older
versions get nothing, including a backport on request — upgrade.

| Version | Security fixes |
| --- | --- |
| current minor | yes |
| previous minor | yes |
| anything older | no |

Releases that contain a security fix say so at the top of the release notes, with the severity
and the advisory link. Administrators see an in-app notice when their instance is behind a
release that carries a security fix.

## Recognition

Reporters are credited in the advisory and the release notes, unless they ask not to be. There is
no bug bounty while the project is pre-1.0; one is planned for when it goes public, and reports
sent before then will be considered for it retroactively.

## Hardening your deployment

[docs/configuration.md](docs/configuration.md) describes the security settings an operator is expected to apply: encryption at rest, database roles, TLS and log retention.
