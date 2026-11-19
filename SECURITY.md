# Security Policy

## Reporting a vulnerability

Please do not report security problems in public issues, discussions or pull
requests.

Report them privately through GitHub: open the **Security** tab of this
repository and choose **Report a vulnerability**. If that is not possible,
email **security@airo-bot.com**.

Include what you found, how to reproduce it, and what an attacker could do
with it. We will confirm receipt within 3 working days, keep you updated, and
credit you in the release notes unless you prefer otherwise. Please give us
reasonable time to release a fix before disclosing it publicly.

## Supported versions

Airobot is before 1.0. Security fixes go into the latest release only; please
upgrade to it before reporting.

## Scope

In scope: this repository's server, web app, desktop app, CLI and the Docker
image built from it. Airobot Cloud (airo-bot.com) runs the same code; report
problems you find there the same way, and do not test against other people's
data or degrade the service.

How the server protects data is described in
[docs/security-model.md](docs/security-model.md).
