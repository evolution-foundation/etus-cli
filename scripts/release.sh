#!/usr/bin/env bash
set -euo pipefail

REPO="evolution-foundation/etus-cli"
MAC_TARGETS=("aarch64-apple-darwin:etus-aarch64-macos" "x86_64-apple-darwin:etus-x86_64-macos")
LINUX_PLATFORMS=("linux/amd64:etus-x86_64-linux" "linux/arm64:etus-aarch64-linux")
CHECK_IMAGE="alpine:3.22"

usage() {
  echo "usage: scripts/release.sh <version> [--publish]" >&2
  exit 2
}

fail() {
  echo "error: $*" >&2
  exit 1
}

[[ $# -ge 1 ]] || usage
VERSION="${1#v}"
PUBLISH=0
shift
for arg in "$@"; do
  case "$arg" in
    --publish) PUBLISH=1 ;;
    *) usage ;;
  esac
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CRATE_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
[[ "$VERSION" == "$CRATE_VERSION" ]] || fail "version $VERSION does not match Cargo.toml ($CRATE_VERSION)"
grep -q "^## $VERSION " CHANGELOG.md || fail "CHANGELOG.md has no section for $VERSION"
[[ "$(uname -s)" == "Darwin" ]] || fail "run on macOS: the macOS binaries are built natively"
command -v rustup >/dev/null || fail "rustup is required"
command -v docker >/dev/null || fail "docker is required"
docker buildx version >/dev/null || fail "docker buildx is required"

COMMIT="$(git rev-parse HEAD)"
if [[ $PUBLISH -eq 1 ]]; then
  command -v gh >/dev/null || fail "gh is required to publish"
  [[ -z "$(git status --porcelain --untracked-files=no)" ]] || fail "commit or stash local changes before publishing"
  git fetch -q origin main
  [[ "$COMMIT" == "$(git rev-parse origin/main)" ]] || fail "publish from the merged origin/main (HEAD is $COMMIT)"
  ! gh release view "v$VERSION" --repo "$REPO" >/dev/null 2>&1 || fail "release v$VERSION already exists"
fi

DIST="$ROOT/dist/v$VERSION"
rm -rf "$DIST"
mkdir -p "$DIST"

for entry in "${MAC_TARGETS[@]}"; do
  target="${entry%%:*}"
  asset="${entry##*:}"
  echo "==> $asset ($target)"
  rustup target add --toolchain stable "$target" >/dev/null
  RUSTC="$(rustup which --toolchain stable rustc)" \
    "$(rustup which --toolchain stable cargo)" build --release --locked --target "$target"
  cp "target/$target/release/etus" "$DIST/$asset"
  codesign --force --sign - "$DIST/$asset"
done

for entry in "${LINUX_PLATFORMS[@]}"; do
  platform="${entry%%:*}"
  asset="${entry##*:}"
  out="$(mktemp -d)"
  echo "==> $asset ($platform, musl static)"
  docker buildx build \
    --platform "$platform" \
    --file scripts/release.Dockerfile \
    --output "type=local,dest=$out" \
    .
  cp "$out/etus" "$DIST/$asset"
  chmod 755 "$DIST/$asset"
  rm -rf "$out"
done

ASSETS=(etus-aarch64-macos etus-x86_64-macos etus-x86_64-linux etus-aarch64-linux)

(
  cd "$DIST"
  printf '%s\n' "$VERSION" > VERSION
  shasum -a 256 "${ASSETS[@]}" > SHA256SUMS
  for asset in "${ASSETS[@]}"; do
    grep "  $asset\$" SHA256SUMS > "$asset.sha256"
  done
  shasum -a 256 -c SHA256SUMS
)

check() {
  local asset="$1"
  shift
  local got
  got="$("$@")" || fail "$asset does not run"
  [[ "$got" == "etus $VERSION" ]] || fail "$asset printed '$got', expected 'etus $VERSION'"
  printf '%-22s %9s bytes  %s\n' "$asset" "$(stat -f %z "$DIST/$asset")" "$got"
}

echo "==> checks"
check etus-aarch64-macos "$DIST/etus-aarch64-macos" --version
if arch -x86_64 /usr/bin/true 2>/dev/null; then
  check etus-x86_64-macos arch -x86_64 "$DIST/etus-x86_64-macos" --version
else
  echo "etus-x86_64-macos: Rosetta not available, skipped run check"
fi
check etus-x86_64-linux docker run --rm --platform linux/amd64 -v "$DIST:/dist:ro" "$CHECK_IMAGE" /dist/etus-x86_64-linux --version
check etus-aarch64-linux docker run --rm --platform linux/arm64 -v "$DIST:/dist:ro" "$CHECK_IMAGE" /dist/etus-aarch64-linux --version
for asset in etus-aarch64-macos etus-x86_64-macos; do
  codesign --verify "$DIST/$asset" || fail "$asset signature is invalid"
done
for asset in etus-x86_64-linux etus-aarch64-linux; do
  file "$DIST/$asset" | grep -Eq "statically linked|static-pie linked" || fail "$asset is not statically linked"
done

if [[ $PUBLISH -eq 0 ]]; then
  echo "==> built in $DIST (not published; pass --publish to create the release)"
  exit 0
fi

TAG="v$VERSION"
NOTES="$(awk -v v="$VERSION" '$0 ~ "^## " v " " {on=1; next} /^## / {on=0} on' CHANGELOG.md)"
FILES=(SHA256SUMS VERSION)
for asset in "${ASSETS[@]}"; do
  FILES+=("$asset" "$asset.sha256")
done
FILES=("${FILES[@]/#/$DIST/}")
gh release create "$TAG" \
  --repo "$REPO" \
  --target "$COMMIT" \
  --title "$TAG" \
  --notes "$NOTES" \
  "${FILES[@]}"
