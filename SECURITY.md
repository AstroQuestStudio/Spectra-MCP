# Security Policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories: open the **Security** tab of this repository and choose **Report a vulnerability**. Do not open a public issue for a security problem.

We will acknowledge your report, investigate, and keep you informed of the fix.

## Scope notes

Spectra only speaks MCP over stdio. It never opens a network port and never exposes an HTTP or SSE transport, so remote access to the server is not part of its design. Reports about this boundary (for example, any way to make Spectra listen on a socket) are especially welcome.
