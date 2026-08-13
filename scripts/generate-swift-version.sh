#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUTPUT_XCCONFIG" >&2
  exit 2
fi

OUTPUT=$1
VERSION=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)
BUILD=${THREADRELAY_BUILD_NUMBER:-}

if [ -z "$VERSION" ] || [ -z "$BUILD" ]; then
  echo "Cargo version and THREADRELAY_BUILD_NUMBER are required" >&2
  exit 1
fi

case "$BUILD" in
  *[!0-9]*|'')
    echo "THREADRELAY_BUILD_NUMBER must be a positive integer" >&2
    exit 2
    ;;
esac

mkdir -p "$(dirname "$OUTPUT")"
{
  echo "// Generated from Cargo.toml. Do not edit."
  echo "MARKETING_VERSION = ${VERSION%%[-+]*}"
  echo "CURRENT_PROJECT_VERSION = $BUILD"
} >"$OUTPUT"
