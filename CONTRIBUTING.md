# Contributing to siggy

Thanks for your interest in contributing! Here's how to get started.

## Getting started

1. Fork the repo and clone your fork
2. Install prerequisites: Rust 1.70+ and [signal-cli](https://github.com/AsamK/signal-cli)
3. Build and run tests:

```sh
cargo build
cargo test
```

Use `--demo` mode to test the UI without a Signal account:

```sh
cargo run -- --demo
```

## Making changes

1. Create a feature branch from `master`:

```sh
git checkout -b feature/my-change
```

2. Make your changes. Run checks before committing:

```sh
cargo clippy --tests -- -D warnings
cargo test
```

3. Push your branch and open a pull request against `master`.

## Branch naming

Use prefixed names: `feature/`, `fix/`, `refactor/`, `docs/`

Examples: `feature/dark-mode`, `fix/unread-count`, `docs/update-readme`

## Code style

- Follow existing patterns in the codebase
- Run `cargo clippy` with warnings as errors -- CI enforces this
- Keep commits focused: one logical change per commit
- Write descriptive commit messages

## Reporting bugs

Use the [bug report template](https://github.com/johnsideserf/siggy/issues/new?template=bug_report.yml). Include your OS, terminal emulator, and siggy version.

## Suggesting features

Use the [feature request template](https://github.com/johnsideserf/siggy/issues/new?template=feature_request.yml). Describe the problem you're trying to solve.

## Translating

If you want to contribute a README translation, see [TRANSLATING.md](TRANSLATING.md). Translations do not require Rust knowledge and are a great way to help if you are a native speaker of a language siggy does not yet support.

## License

By contributing, you agree that your contributions will be licensed under [GPL-3.0](LICENSE).
