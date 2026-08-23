#!/usr/bin/env python3
"""Build the component-aware macOS update manifest for a GitHub release."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import Any


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("value must be a positive integer")
    return parsed


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def asset(path: Path, name: str, kind: str, base_url: str, signed: bool, notarized: bool) -> dict[str, Any]:
    return {
        "type": kind,
        "url": f"{base_url}/{name}",
        "sha256": sha256(path),
        "size": path.stat().st_size,
        "signed": signed,
        "notarized": notarized,
    }


def read_previous(path: Path | None) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    if path is None or not path.is_file():
        return None, None
    payload = json.loads(path.read_text(encoding="utf-8"))
    if payload.get("schemaVersion") == 2:
        return payload.get("ui"), payload.get("daemon")
    if "version" in payload:
        legacy_ui = {
            key: payload[key]
            for key in ("version", "build", "releaseUrl", "notes", "assets")
            if key in payload
        }
        return legacy_ui, None
    raise ValueError("previous manifest is neither schema 1 nor schema 2")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--previous", type=Path)
    parser.add_argument("--component", choices=("all", "ui", "daemon"), required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--signed", action="store_true")
    parser.add_argument("--ui-version", required=True)
    parser.add_argument("--ui-build", type=positive_int, required=True)
    parser.add_argument("--ui-dmg", type=Path, required=True)
    parser.add_argument("--ui-dmg-name", required=True)
    parser.add_argument("--ui-app-zip", type=Path, required=True)
    parser.add_argument("--ui-app-zip-name", required=True)
    parser.add_argument("--daemon-version", required=True)
    parser.add_argument("--daemon-build", type=positive_int, required=True)
    parser.add_argument("--daemon-binary", type=Path, required=True)
    parser.add_argument("--daemon-binary-name", required=True)
    parser.add_argument("--daemon-api-major", type=positive_int, default=1)
    parser.add_argument("--minimum-ui-version", required=True)
    parser.add_argument("--minimum-ui-build", type=positive_int, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if args.component == "daemon" and not args.signed:
        raise ValueError("daemon-only releases require a signed macOS build")
    if args.component == "all" and not args.signed:
        print(
            "warning: unsigned macOS builds publish UI metadata only; daemon metadata is omitted",
            file=sys.stderr,
        )
    previous_ui, previous_daemon = read_previous(args.previous)
    release_url = f"https://github.com/{args.repository}/releases/tag/{args.release_tag}"
    base_url = f"https://github.com/{args.repository}/releases/download/{args.release_tag}"

    ui = previous_ui
    if args.component in ("all", "ui"):
        ui = {
            "version": args.ui_version,
            "build": args.ui_build,
            "releaseUrl": release_url,
            "notes": "",
            "assets": {
                "macos-universal": asset(
                    args.ui_dmg,
                    args.ui_dmg_name,
                    "dmg",
                    base_url,
                    args.signed,
                    args.signed,
                ),
                "macos-sparkle-universal": asset(
                    args.ui_app_zip,
                    args.ui_app_zip_name,
                    "app-zip",
                    base_url,
                    args.signed,
                    args.signed,
                ),
            },
        }

    daemon = previous_daemon
    if args.component in ("all", "daemon") and args.signed:
        daemon = {
            "version": args.daemon_version,
            "build": args.daemon_build,
            "apiMajor": args.daemon_api_major,
            "minimumUIVersion": args.minimum_ui_version,
            "minimumUIBuild": args.minimum_ui_build,
            "releaseUrl": release_url,
            "notes": "",
            "assets": {
                "macos-daemon-universal": asset(
                    args.daemon_binary,
                    args.daemon_binary_name,
                    "executable",
                    base_url,
                    True,
                    True,
                )
            },
        }

    if ui is None:
        raise ValueError("a daemon-only release requires a previous UI manifest")

    manifest: dict[str, Any] = {"schemaVersion": 2, "ui": ui}
    if daemon is not None:
        manifest["daemon"] = daemon
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
