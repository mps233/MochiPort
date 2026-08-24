#!/usr/bin/env python3
"""Collect Cargo dependency notices into a deterministic release document."""

from __future__ import annotations

import json
import argparse
import subprocess
from collections import OrderedDict
from pathlib import Path


NOTICE_NAMES = ("LICENSE*", "COPYING*", "NOTICE*", "COPYRIGHT*")


def package_notice_files(package: dict[str, object]) -> list[Path]:
    manifest = Path(str(package["manifest_path"]))
    package_dir = manifest.parent
    candidates: set[Path] = set()

    license_file = package.get("license_file")
    if license_file:
        path = Path(str(license_file))
        candidates.add(path if path.is_absolute() else package_dir / path)

    for pattern in NOTICE_NAMES:
        candidates.update(path for path in package_dir.glob(pattern) if path.is_file())

    return sorted(
        (path.resolve() for path in candidates if path.is_file()),
        key=lambda path: path.name.casefold(),
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Collect Cargo dependency notices into a deterministic release document."
    )
    parser.add_argument("output", type=Path)
    parser.add_argument(
        "--manifest-path",
        action="append",
        default=[],
        help="Cargo manifest to include; may be repeated (defaults to the repository root).",
    )
    args = parser.parse_args()

    repository_root = Path(__file__).resolve().parent.parent
    output_path = args.output
    if not output_path.is_absolute():
        output_path = repository_root / output_path

    manifest_paths = [
        (repository_root / path).resolve() if not Path(path).is_absolute() else Path(path).resolve()
        for path in (args.manifest_path or ["Cargo.toml"])
    ]
    packages_by_id: dict[str, dict[str, object]] = {}
    for manifest_path in manifest_paths:
        metadata = json.loads(
            subprocess.check_output(
                [
                    "cargo",
                    "metadata",
                    "--manifest-path",
                    str(manifest_path),
                    "--locked",
                    "--format-version",
                    "1",
                ],
                cwd=repository_root,
                text=True,
            )
        )
        root_package_id = metadata.get("resolve", {}).get("root")
        for package in metadata["packages"]:
            if package["id"] != root_package_id:
                packages_by_id.setdefault(package["id"], package)
    packages = sorted(
        packages_by_id.values(),
        key=lambda package: (package["name"].casefold(), package["version"], package["id"]),
    )

    sections = [(repository_root / "packaging" / "THIRD_PARTY_LICENSES.txt").read_text()]
    sections.append("\n\nCargo dependency inventory\n==========================\n")
    notice_owners: OrderedDict[str, dict[str, set[str]]] = OrderedDict()
    missing_notices: list[str] = []
    for package in packages:
        package_label = f"{package['name']} {package['version']}"
        license_name = package.get("license") or "not specified"
        sections.append(f"\n- {package_label} | {license_name}")
        found_notice = False
        for path in package_notice_files(package):
            content = path.read_text(encoding="utf-8", errors="replace").strip()
            if not content:
                continue
            found_notice = True
            notice = notice_owners.setdefault(content, {"packages": set(), "filenames": set()})
            notice["packages"].add(package_label)
            notice["filenames"].add(path.name)

        if not found_notice:
            missing_notices.append(package_label)

    if missing_notices:
        sections.append("\n\nPackages without standalone notice files\n----------------------------------------\n")
        sections.extend(f"\n- {package}" for package in missing_notices)

    sections.append("\n\nCargo dependency license texts\n==============================\n")
    for index, (content, owners) in enumerate(notice_owners.items(), start=1):
        filenames = ", ".join(sorted(owners["filenames"], key=str.casefold))
        packages_text = ", ".join(sorted(owners["packages"], key=str.casefold))
        sections.append(
            f"\n\nLicense text {index}\n"
            f"----------------\n"
            f"Files: {filenames}\n"
            f"Used by: {packages_text}\n\n"
            f"{content}\n"
        )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("".join(sections).rstrip() + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
