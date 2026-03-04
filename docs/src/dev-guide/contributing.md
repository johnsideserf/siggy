# Contributing

## Getting started

1. Fork the repository and clone your fork
2. Install prerequisites: **Rust 1.70+** and
   [signal-cli](https://github.com/AsamK/signal-cli)
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

Use prefixed names:

| Prefix | Use case |
|---|---|
| `feature/` | New functionality |
| `fix/` | Bug fixes |
| `refactor/` | Code restructuring |
| `docs/` | Documentation changes |

Examples: `feature/dark-mode`, `fix/unread-count`, `docs/update-readme`

## Code style

- Follow existing patterns in the codebase
- Run `cargo clippy` with warnings-as-errors -- CI enforces this
- Keep commits focused: one logical change per commit
- Write descriptive commit messages
- Reference issue numbers in commits and PRs (e.g., `closes #29`)

## Pull requests

- Create a PR targeting `master`
- Include a clear description of what changed and why
- Reference the issue being addressed if applicable
- Make sure CI passes (clippy + tests)
- Trivial docs-only changes may be committed directly to `master`; all code
  changes must go through a PR

## Reporting bugs

Use the
[bug report template](https://github.com/johnsideserf/siggy/issues/new?template=bug_report.yml).
Include:

- Your OS and terminal emulator
- siggy version (`siggy --version` or the release tag)
- Steps to reproduce the issue

## Suggesting features

Use the
[feature request template](https://github.com/johnsideserf/siggy/issues/new?template=feature_request.yml).
Describe the problem you're trying to solve before proposing a solution.

## License

By contributing, you agree that your contributions will be licensed under
[GPL-3.0](https://github.com/johnsideserf/siggy/blob/master/LICENSE).
