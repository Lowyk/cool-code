# Contributing

Thanks for helping. Cool Code is a Rust program; see [docs/architecture.md](docs/architecture.md)
for how the pieces fit together.

## Before you send a change

Run all three, as CI does on Linux, Windows and macOS:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

On Linux you need `libdbus-1-dev` and `pkg-config` to build.

## What a good change looks like

- **Test first.** A new behaviour or a bug fix comes with a test that fails without it. Tests never
  touch your real settings, credential store or network: they use a temporary config, an in-memory
  credential store and the local mock server in `src/testutil.rs`.
- **Small and focused.** One change per pull request, with a message that says what and why.
- **No secrets.** Never put an API key, token or personal data in code, tests, logs or issues.
- **Be honest about providers.** Do not add support that works by disguising the client or
  reusing a subscription login a provider forbids for third-party tools. Say so in the docs when a
  feature is unofficial.
- **Keep the harness in charge.** Permissions, trust and limits are enforced by the program, never
  by what the model says.

## Reporting a problem

Use the issue templates. For anything security-related, follow [SECURITY.md](SECURITY.md) instead
of opening a public issue.
