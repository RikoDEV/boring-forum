# Boring Forum documentation

Boring Forum is a community platform written in Rust. These pages cover running and extending it.
To try it locally, follow the quick start in the
[repository README](https://github.com/RikoDEV/boring-forum#quick-start).

## Running a forum

- [Installing](installation.md): Docker Compose or from source, a reverse proxy and TLS, email, and
  upgrading.
- [Administering a forum](administration.md): forums, groups and permissions, and the admin panel
  section by section.
- [Moderating a forum](moderation.md): the review queue, sanctions, spam protection and the EU
  Digital Services Act process.
- [Configuration](configuration.md): environment variables, encryption at rest, database roles,
  TLS and log retention.
- [Operations](operations.md): monitoring, backups and restores, and importing from IPS, phpBB or
  Discourse.
- [Features and settings](features.md): uploads, federation, AI features, themes, badges and the
  other things an administrator can switch on.
- [Troubleshooting](troubleshooting.md): what to check when something is wrong.

## Building on it

- [The JSON API](api.md): API keys, how requests behave, and webhooks.
- [Writing a plugin](plugins/getting-started.md): a worked example, the manifest and how to ship.
- [Plugin reference](plugins/abi.md): the calling convention, hooks, capabilities and limits.

## Maintaining it

- [Releasing](releasing.md): how versions are chosen and published, and how to roll back.

## Getting help

- Questions and bugs: [GitHub issues](https://github.com/RikoDEV/boring-forum/issues).
- Security reports: see the [security policy](https://github.com/RikoDEV/boring-forum/blob/main/SECURITY.md).
- Contributing: see [CONTRIBUTING.md](https://github.com/RikoDEV/boring-forum/blob/main/CONTRIBUTING.md).
