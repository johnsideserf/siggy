#!/usr/bin/env bash
set -euo pipefail

REPO="johnsideserf/signal-tui"
INSTALL_DIR="$HOME/.local/bin"
SIGNAL_CLI_REPO="AsamK/signal-cli"

# Cleanup on exit
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

info()  { printf '\033[1;34m::\033[0m %s\n' "$*"; }
error() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# --- Detect platform ---
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-unknown-linux-gnu" ;;
      *)       error "Unsupported Linux architecture: $ARCH" ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64)  TARGET="x86_64-apple-darwin" ;;
      arm64)   TARGET="aarch64-apple-darwin" ;;
      *)       error "Unsupported macOS architecture: $ARCH" ;;
    esac
    ;;
  *)
    error "Unsupported OS: $OS (use install.ps1 for Windows)"
    ;;
esac

info "Detected platform: $TARGET"

# --- Get latest release tag ---
info "Fetching latest release..."
LATEST_URL="https://api.github.com/repos/$REPO/releases/latest"
RELEASE_JSON="$(curl -fsSL "$LATEST_URL")" || error "Failed to fetch release info. Check your internet connection."
TAG="$(printf '%s' "$RELEASE_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"

if [ -z "$TAG" ]; then
  error "Could not determine latest release tag"
fi

info "Latest release: $TAG"

# --- Download and install signal-tui ---
ARCHIVE="signal-tui-${TAG}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/$REPO/releases/download/$TAG/$ARCHIVE"

info "Downloading $ARCHIVE..."
curl -fsSL -o "$TMPDIR/$ARCHIVE" "$DOWNLOAD_URL" || error "Download failed: $DOWNLOAD_URL"

mkdir -p "$INSTALL_DIR"
tar xzf "$TMPDIR/$ARCHIVE" -C "$INSTALL_DIR"
chmod +x "$INSTALL_DIR/signal-tui"

info "Installed signal-tui to $INSTALL_DIR/signal-tui"

# --- Check for signal-cli ---
if command -v signal-cli >/dev/null 2>&1; then
  info "signal-cli found: $(command -v signal-cli)"
else
  info "signal-cli not found"

  if [ "$OS" = "Linux" ]; then
    info "Installing signal-cli native build (no Java required)..."

    # Get latest signal-cli release tag
    SCLI_JSON="$(curl -fsSL "https://api.github.com/repos/$SIGNAL_CLI_REPO/releases/latest")" || error "Failed to fetch signal-cli release info"
    SCLI_TAG="$(printf '%s' "$SCLI_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"

    if [ -z "$SCLI_TAG" ]; then
      error "Could not determine latest signal-cli release tag"
    fi

    SCLI_VERSION="${SCLI_TAG#v}"
    SCLI_ARCHIVE="signal-cli-native-${SCLI_VERSION}-Linux-x86-64.tar.gz"
    SCLI_URL="https://github.com/$SIGNAL_CLI_REPO/releases/download/$SCLI_TAG/$SCLI_ARCHIVE"

    info "Downloading signal-cli $SCLI_TAG native build..."
    curl -fsSL -o "$TMPDIR/$SCLI_ARCHIVE" "$SCLI_URL" || error "signal-cli download failed: $SCLI_URL"

    tar xzf "$TMPDIR/$SCLI_ARCHIVE" -C "$TMPDIR"
    cp "$TMPDIR/signal-cli-native-${SCLI_VERSION}/bin/signal-cli" "$INSTALL_DIR/signal-cli"
    chmod +x "$INSTALL_DIR/signal-cli"

    info "Installed signal-cli to $INSTALL_DIR/signal-cli"

  elif [ "$OS" = "Darwin" ]; then
    echo ""
    info "signal-cli is required. Install it with one of:"
    echo ""
    if command -v brew >/dev/null 2>&1; then
      echo "  brew install signal-cli"
    else
      echo "  # Install Homebrew first: https://brew.sh"
      echo "  brew install signal-cli"
    fi
    echo ""
    echo "  Or download manually from:"
    echo "  https://github.com/AsamK/signal-cli/releases"
    echo "  (Requires Java 21+)"
    echo ""
  fi
fi

# --- Check PATH ---
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    info "$INSTALL_DIR is not in your PATH. Add it with:"
    echo ""
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "  Add that line to your ~/.bashrc or ~/.zshrc to make it permanent."
    echo ""
    ;;
esac

# --- Done ---
echo ""
info "Done! Run 'signal-tui' to get started."
