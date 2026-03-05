# Installation

## From crates.io

Requires **Rust 1.70+**.

```sh
cargo install siggy
```

## Pre-built binaries

Download the latest release for your platform from the
[Releases page](https://github.com/johnsideserf/siggy/releases).

### Linux / macOS (one-liner)

```sh
curl -fsSL https://raw.githubusercontent.com/johnsideserf/siggy/master/install.sh | bash
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/johnsideserf/siggy/master/install.ps1 | iex
```

Both install scripts download the latest release binary and check for signal-cli.

## Build from source

Requires **Rust 1.70+**.

Install directly from the repository:

```sh
cargo install --git https://github.com/johnsideserf/siggy.git
```

Or clone and build locally:

```sh
git clone https://github.com/johnsideserf/siggy.git
cd siggy
cargo build --release
# Binary is at target/release/siggy
```

## signal-cli setup

siggy requires [signal-cli](https://github.com/AsamK/signal-cli) as its messaging backend.

1. **Install signal-cli** -- follow the
   [signal-cli installation guide](https://github.com/AsamK/signal-cli/wiki/Installation).
   The install scripts above will check for it automatically.

2. **Make it accessible** -- signal-cli must be on your `PATH`, or you can set the
   full path in the [config file](configuration.md):

   ```toml
   signal_cli_path = "/usr/local/bin/signal-cli"
   ```

   On Windows, point to `signal-cli.bat` if it isn't in your `PATH`.

3. **Java runtime** -- signal-cli 0.14+ requires Java 25+. Make sure `java` is
   available in your shell. On Linux, the install script uses the native signal-cli
   build which does not require Java.

## Supported platforms

| Platform | Binary | Notes |
|---|---|---|
| Linux x86_64 | `siggy-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | |
| macOS x86_64 | `siggy-vX.Y.Z-x86_64-apple-darwin.tar.gz` | Intel Macs |
| macOS arm64 | `siggy-vX.Y.Z-aarch64-apple-darwin.tar.gz` | Apple Silicon |
| Windows x86_64 | `siggy-vX.Y.Z-x86_64-pc-windows-msvc.zip` | |
