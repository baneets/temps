#!/usr/bin/env bash
# Build and push all sandbox images to Docker Hub (gotempsh/).
# Requires: docker buildx, logged in to Docker Hub as gotempsh.
# Usage: ./scripts/build-sandbox-images.sh [runtime...]
# If no runtimes specified, builds all: node bun python rust go full

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ $# -eq 0 ]; then
    RUNTIMES=(node bun python rust go full)
else
    RUNTIMES=("$@")
fi

PRINT_DOCKERFILE="$REPO_ROOT/target/debug/examples/print_dockerfile"

# Always rebuild the helper so Dockerfile changes in temps-agents land in
# the pushed images. Cargo short-circuits if nothing changed, so the cost
# is just a stat-check on incremental builds.
echo "Building print_dockerfile helper..."
cargo build --example print_dockerfile -p temps-agents --manifest-path "$REPO_ROOT/Cargo.toml"

if [ ! -x "$PRINT_DOCKERFILE" ]; then
    echo "error: print_dockerfile binary not found at $PRINT_DOCKERFILE after build" >&2
    exit 1
fi

# Ensure buildx builder with multi-arch support
BUILDER_NAME="temps-multiarch"
if ! docker buildx inspect "$BUILDER_NAME" >/dev/null 2>&1; then
    echo "Creating buildx builder $BUILDER_NAME..."
    docker buildx create --name "$BUILDER_NAME" --driver docker-container --use
else
    docker buildx use "$BUILDER_NAME"
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

for runtime in "${RUNTIMES[@]}"; do
    IMAGE="gotempsh/temps-sandbox-${runtime}"
    echo ""
    echo "=========================================="
    echo "Building and pushing: $IMAGE"
    echo "=========================================="

    BUILD_DIR="$TMPDIR/$runtime"
    mkdir -p "$BUILD_DIR"

    "$PRINT_DOCKERFILE" "$runtime" > "$BUILD_DIR/Dockerfile"

    docker buildx build \
        --platform linux/amd64,linux/arm64 \
        --tag "$IMAGE:latest" \
        --push \
        "$BUILD_DIR"

    echo "✓ $IMAGE:latest pushed"
done

echo ""
echo "All images built and pushed successfully."
