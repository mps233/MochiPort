#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 OUTPUT_XCCONFIG" >&2
  exit 2
fi

OUTPUT=$1
VERSION=${MOCHIPORT_UI_VERSION:-}
BUILD=${MOCHIPORT_UI_BUILD_NUMBER:-}

if [ -z "$VERSION" ] && [ -f "$OUTPUT" ]; then
  VERSION=$(sed -n 's/^MARKETING_VERSION *= *\([^ ]*\).*/\1/p' "$OUTPUT" | head -n 1)
fi
if [ -z "$BUILD" ] && [ -f "$OUTPUT" ]; then
  BUILD=$(sed -n 's/^CURRENT_PROJECT_VERSION *= *\([^ ]*\).*/\1/p' "$OUTPUT" | head -n 1)
fi

if [ -z "$VERSION" ] || [ -z "$BUILD" ]; then
  echo "MOCHIPORT_UI_VERSION and MOCHIPORT_UI_BUILD_NUMBER are required" >&2
  exit 1
fi

case "$BUILD" in
  *[!0-9]*|'')
    echo "MOCHIPORT_UI_BUILD_NUMBER must be a positive integer" >&2
    exit 2
    ;;
esac

mkdir -p "$(dirname "$OUTPUT")"
{
  echo "// MochiPort UI version. Independent from the embedded daemon version."
  echo "MARKETING_VERSION = ${VERSION%%[-+]*}"
  echo "CURRENT_PROJECT_VERSION = $BUILD"
} >"$OUTPUT"
