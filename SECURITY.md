# Security

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private vulnerability
reporting on this repository (Security tab, "Report a vulnerability"). Include what you found, how
to reproduce it, and what it lets someone do. You will get an answer as soon as it can be looked
at, and a fix before any public write-up.

## What counts

Cool Code runs commands and edits files on your computer on a model's behalf, so these matter most:

- A way for a repository, file or model reply to trust a folder, widen a permission mode or approve
  an action by itself.
- A way to read or write outside the trusted workspace.
- An API key, sign-in token or other credential written to a file, a log or a request to the wrong
  host.
- A prompt or file sent to a provider that the privacy settings said would not be.

## What is not covered

The ChatGPT Plus/Pro sign-in is unofficial: it can stop working, and its use may conflict with
OpenAI's terms. That is documented in the README and is a risk you accept by using it, not a
vulnerability.
