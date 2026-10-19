# Contributing to OmniCore

Thank you for helping improve OmniCore. Please open an issue for a reproducible bug or a focused feature proposal before large changes. Include the Rust version, operating system, setup steps, expected behavior, and redacted logs. Never post a Discord token, provider key, private message, voice transcript, user identifier, or database dump in an issue.

For a pull request:

1. Fork the repository and make a focused branch. Keep the bot in Rust and preserve existing data and permission boundaries.
2. Add or update tests for behavior changes. Tests should use fixtures or mocks and run without production Discord, MongoDB, or paid provider credentials.
3. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked`, `cargo test --locked`, and `cargo build --release --locked`.
4. Explain what changed, how you tested it, and any configuration, privacy, or migration impact. Do not include generated models, logs, secrets, or private runtime files.
5. Accept review and revise the change before merge. Maintainers may request a security or data-retention review for AI, moderation, or voice changes.

By contributing, you agree to provide your contribution under this repository's GPL-3.0 license and affirm that you have the right to submit it. Preserve existing authorship and third-party notices.
