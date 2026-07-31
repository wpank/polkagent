# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.1.x   | Yes                |

Pre-1.0 releases receive security patches on the latest minor version only.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for security vulnerabilities.**

Instead, report vulnerabilities privately by emailing:

**security@polkagent.io**

Include the following in your report:

- A description of the vulnerability and its potential impact.
- Steps to reproduce or a proof-of-concept.
- The affected crate(s) and version(s), if known.
- Any suggested fix or mitigation, if available.

## Response SLA

| Severity | Acknowledgment | Patch Target |
|----------|----------------|--------------|
| Critical | Within 48 hours | Within 7 days |
| High     | Within 48 hours | Within 14 days |
| Medium   | Within 5 business days | Next scheduled release |
| Low      | Within 5 business days | Best effort |

We will confirm receipt of your report, provide an initial assessment, and
coordinate disclosure timing with you.

## Disclosure Policy

- We follow coordinated disclosure. We ask that you give us reasonable time to
  address the issue before making it public.
- We will credit reporters in the release notes unless anonymity is requested.
- A CVE will be requested for confirmed vulnerabilities where applicable.

## Scope

The following are in scope:

- All crates in the `polkagent` workspace.
- The CI/CD pipeline and published artifacts.
- Cryptographic operations, signer isolation, and secret handling.
- Grant/policy evaluation and budget enforcement.
- Effect pipeline safety (duplicate prevention, crash recovery).
- SQLite and data store integrity.

## Out of Scope

- Vulnerabilities in third-party dependencies (report those upstream, but let
  us know if they affect Polkagent).
- Issues that require physical access to the machine running the agent.
- Social engineering attacks against project maintainers.
- Denial-of-service attacks that require unrestricted network access to the
  host machine (as opposed to API-level DoS).
- Issues in example or test code that is not shipped in release artifacts.
