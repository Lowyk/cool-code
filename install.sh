#!/bin/sh
# Installs Cool Code (the `coolcode` command) from a GitHub release on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/Lowyk/cool-code/master/install.sh | sh
#
# Settings, all optional:
#   COOLCODE_VERSION      a release tag such as v0.1.0 (default: the latest release)
#   COOLCODE_INSTALL_DIR  where to put the program (default: $HOME/.local/bin)
#   COOLCODE_REPO         owner/name on GitHub (default: Lowyk/cool-code)
#   COOLCODE_BASE_URL     a folder or URL that already holds the archive and its .sha256
#                         (used for testing)
#   COOLCODE_TARGET       override the detected platform, for example x86_64-unknown-linux-gnu

set -eu

REPO="${COOLCODE_REPO:-Lowyk/cool-code}"
VERSION="${COOLCODE_VERSION:-latest}"
DEST="${COOLCODE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}
need() { command -v "$1" > /dev/null 2>&1 || fail "$1 is needed to install Cool Code"; }

need curl
need tar
need uname
need install

detect_target() {
  if [ -n "${COOLCODE_TARGET:-}" ]; then
    printf '%s' "$COOLCODE_TARGET"
    return
  fi
  os=$(uname -s)
  arch=$(uname -m)
  case "$os-$arch" in
    Linux-x86_64 | Linux-amd64) printf 'x86_64-unknown-linux-gnu' ;;
    Darwin-arm64 | Darwin-aarch64) printf 'aarch64-apple-darwin' ;;
    Darwin-x86_64) printf 'x86_64-apple-darwin' ;;
    *) fail "there is no Cool Code build for $os on $arch yet (see https://github.com/$REPO/releases)" ;;
  esac
}

latest_tag() {
  # The "latest" page redirects to the newest release's tag; this needs no API token.
  url=$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") \
    || fail "could not look up the latest release of $REPO"
  tag=${url##*/}
  case "$tag" in
    v[0-9]*) printf '%s' "$tag" ;;
    *) fail "$REPO has no published release yet" ;;
  esac
}

sha256_of() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum > /dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    fail "sha256sum or shasum is needed to check the download"
  fi
}

target=$(detect_target)
if [ "$VERSION" = "latest" ]; then
  tag=$(latest_tag)
else
  tag="$VERSION"
fi
name="coolcode-$tag-$target"
base="${COOLCODE_BASE_URL:-https://github.com/$REPO/releases/download/$tag}"

work=$(mktemp -d 2> /dev/null || mktemp -d -t coolcode)
trap 'rm -rf "$work"' EXIT INT TERM

say "Installing Cool Code $tag for $target"
curl -fsSL -o "$work/$name.tar.gz" "$base/$name.tar.gz" \
  || fail "could not download $base/$name.tar.gz (is $tag a release with a build for $target?)"
curl -fsSL -o "$work/$name.sha256" "$base/$name.sha256" \
  || fail "could not download the checksum file $base/$name.sha256"

expected=$(awk '{print $1; exit}' "$work/$name.sha256")
actual=$(sha256_of "$work/$name.tar.gz")
[ -n "$expected" ] && [ "$expected" = "$actual" ] \
  || fail "the download does not match its checksum, so it was not installed"

tar -xzf "$work/$name.tar.gz" -C "$work"
[ -f "$work/$name/coolcode" ] || fail "the archive did not contain coolcode"

mkdir -p "$DEST"
install -m 755 "$work/$name/coolcode" "$DEST/coolcode"
say "Installed $DEST/coolcode"

case ":$PATH:" in
  *":$DEST:"*) say "Run: coolcode" ;;
  *)
    say ""
    say "$DEST is not on your PATH yet. Add this line to your shell profile, then open a new terminal:"
    say "  export PATH=\"$DEST:\$PATH\""
    say "Or run it right away: $DEST/coolcode"
    ;;
esac
