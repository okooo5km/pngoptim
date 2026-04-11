#!/bin/bash
# Copyright (c) 2026 okooo5km(十里). All rights reserved.
# Licensed under the MIT License.
#
# Build XCFramework from Rust FFI crate (workspace layout).
# Usage:
#   bash scripts/build-xcframework.sh              # Full build (5 architectures)
#   bash scripts/build-xcframework.sh --local-only  # Native target only (development)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FRAMEWORK_NAME="PNGOptimCore"
LIB_NAME="libpngoptim_ffi.a"
MERGED_LIB_NAME="libpngoptim_merged.a"

export MACOSX_DEPLOYMENT_TARGET=10.15
export IPHONEOS_DEPLOYMENT_TARGET=13.0

# lcms2 static linking is now enforced via Cargo.toml features = ["static"],
# so LCMS2_NO_PKG_CONFIG=1 is no longer needed here.

LOCAL_ONLY=false
if [[ "${1:-}" == "--local-only" ]]; then
  LOCAL_ONLY=true
fi

echo "==> Building pngoptim-ffi (local_only=$LOCAL_ONLY)"

# ── Helper: find and merge native C libraries into Rust static lib ──
# Rust staticlib doesn't include C library dependencies (e.g., lcms2).
# We find them in the build directory and merge with libtool.
merge_native_libs() {
  local target_dir="$1"
  local ffi_lib="$2"
  local output_lib="$3"

  # Find all native static libraries built by build scripts (lcms2, etc.)
  local native_libs=()
  while IFS= read -r -d '' lib; do
    native_libs+=("$lib")
  done < <(find "$target_dir/build" -name "*.a" -print0 2>/dev/null)

  if [[ ${#native_libs[@]} -gt 0 ]]; then
    echo "    Merging ${#native_libs[@]} native lib(s)..."
    libtool -static -o "$output_lib" "$ffi_lib" "${native_libs[@]}" 2>/dev/null
  else
    cp "$ffi_lib" "$output_lib"
  fi
}

# ── Step 1: Build Rust static libraries ──

if $LOCAL_ONLY; then
  echo "==> Building native target only..."
  cargo build --release --manifest-path "$PROJECT_DIR/ffi/Cargo.toml"

  # Determine native target triple
  NATIVE_TARGET=$(rustc -vV | grep host | cut -d' ' -f2)
  echo "    Native target: $NATIVE_TARGET"

  # Merge native libs (workspace shared target directory)
  TARGET_DIR="$PROJECT_DIR/target/$NATIVE_TARGET/release"
  if [[ ! -d "$TARGET_DIR" ]]; then
    TARGET_DIR="$PROJECT_DIR/target/release"
  fi
  merge_native_libs "$TARGET_DIR" "$TARGET_DIR/$LIB_NAME" "$TARGET_DIR/$MERGED_LIB_NAME"
else
  TARGETS=(
    aarch64-apple-darwin
    x86_64-apple-darwin
    aarch64-apple-ios
    aarch64-apple-ios-sim
    x86_64-apple-ios
  )

  echo "==> Installing Rust targets..."
  for t in "${TARGETS[@]}"; do
    rustup target add "$t" 2>/dev/null || true
  done

  echo "==> Cross-compiling for ${#TARGETS[@]} targets..."
  for t in "${TARGETS[@]}"; do
    echo "    Building $t..."

    # Set SDK sysroot for iOS targets
    case "$t" in
      aarch64-apple-ios)
        SDK_PATH=$(xcrun --sdk iphoneos --show-sdk-path)
        export CFLAGS="-isysroot $SDK_PATH"
        ;;
      aarch64-apple-ios-sim|x86_64-apple-ios)
        SDK_PATH=$(xcrun --sdk iphonesimulator --show-sdk-path)
        export CFLAGS="-isysroot $SDK_PATH"
        ;;
      *)
        unset CFLAGS 2>/dev/null || true
        ;;
    esac

    cargo build --release --target "$t" --manifest-path "$PROJECT_DIR/ffi/Cargo.toml"

    # Merge native libs for this target (workspace shared target directory)
    TARGET_DIR="$PROJECT_DIR/target/$t/release"
    merge_native_libs "$TARGET_DIR" "$TARGET_DIR/$LIB_NAME" "$TARGET_DIR/$MERGED_LIB_NAME"
  done
  unset CFLAGS 2>/dev/null || true
fi

# ── Step 2: Create XCFramework-specific headers ──
# XCFramework needs module name "PNGOptimCore" (matching binaryTarget name)
# to avoid conflict with SPM's "CPNGOptim" C bridge target.

echo "==> Preparing headers..."
XCFW_HEADERS="$PROJECT_DIR/target/xcframework-headers"
mkdir -p "$XCFW_HEADERS"
cp "$PROJECT_DIR/ffi/generated/pngoptim.h" "$XCFW_HEADERS/pngoptim.h"
cat > "$XCFW_HEADERS/module.modulemap" <<'MODULEMAP'
module PNGOptimCore {
  header "pngoptim.h"
  link "pngoptim_ffi"
  export *
}
MODULEMAP

if $LOCAL_ONLY; then
  # ── Local-only: create single-platform XCFramework ──

  LIB_PATH="$PROJECT_DIR/target/$NATIVE_TARGET/release/$MERGED_LIB_NAME"
  if [[ ! -f "$LIB_PATH" ]]; then
    LIB_PATH="$PROJECT_DIR/target/release/$MERGED_LIB_NAME"
  fi

  echo "==> Creating local XCFramework..."
  rm -rf "$PROJECT_DIR/$FRAMEWORK_NAME.xcframework"

  xcodebuild -create-xcframework \
    -library "$LIB_PATH" \
    -headers "$XCFW_HEADERS" \
    -output "$PROJECT_DIR/$FRAMEWORK_NAME.xcframework"
else
  # ── Full build: create universal binaries + multi-platform XCFramework ──

  echo "==> Creating universal binaries..."
  mkdir -p "$PROJECT_DIR/target/universal-macos" "$PROJECT_DIR/target/universal-ios-sim"

  lipo -create \
    "$PROJECT_DIR/target/aarch64-apple-darwin/release/$MERGED_LIB_NAME" \
    "$PROJECT_DIR/target/x86_64-apple-darwin/release/$MERGED_LIB_NAME" \
    -output "$PROJECT_DIR/target/universal-macos/$LIB_NAME"

  lipo -create \
    "$PROJECT_DIR/target/aarch64-apple-ios-sim/release/$MERGED_LIB_NAME" \
    "$PROJECT_DIR/target/x86_64-apple-ios/release/$MERGED_LIB_NAME" \
    -output "$PROJECT_DIR/target/universal-ios-sim/$LIB_NAME"

  echo "==> Creating XCFramework..."
  rm -rf "$PROJECT_DIR/$FRAMEWORK_NAME.xcframework"

  xcodebuild -create-xcframework \
    -library "$PROJECT_DIR/target/universal-macos/$LIB_NAME" \
    -headers "$XCFW_HEADERS" \
    -library "$PROJECT_DIR/target/aarch64-apple-ios/release/$MERGED_LIB_NAME" \
    -headers "$XCFW_HEADERS" \
    -library "$PROJECT_DIR/target/universal-ios-sim/$LIB_NAME" \
    -headers "$XCFW_HEADERS" \
    -output "$PROJECT_DIR/$FRAMEWORK_NAME.xcframework"

  # ── Step 3: Package for distribution ──

  echo "==> Packaging XCFramework..."
  cd "$PROJECT_DIR"
  rm -f "$FRAMEWORK_NAME.xcframework.zip"
  zip -r -y "$FRAMEWORK_NAME.xcframework.zip" "$FRAMEWORK_NAME.xcframework"

  echo "==> Computing checksum..."
  swift package compute-checksum "$FRAMEWORK_NAME.xcframework.zip"
fi

echo "==> Done! XCFramework at: $PROJECT_DIR/$FRAMEWORK_NAME.xcframework"
