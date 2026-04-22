"""Install pinned hugrenv packages from the local lockfile."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import platform
import shutil
import sys
import tarfile
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path
from typing import NoReturn

MIN_ARCHIVE_PARTS = 2


def _fail(message: str) -> NoReturn:
    raise SystemExit(message)


def _detect_platform() -> tuple[str, str]:
    system = sys.platform
    machine = platform.machine().lower()

    arch_map = {
        "x86_64": "x86_64",
        "amd64": "amd64" if system.startswith("win") else "x86_64",
        "arm64": "aarch64",
        "aarch64": "aarch64",
    }
    try:
        arch = arch_map[machine]
    except KeyError as exc:
        message = f"unsupported architecture: {machine}"
        raise SystemExit(message) from exc

    if system.startswith("linux"):
        return ("manylinux_2_28", arch)
    if system == "darwin":
        return ("macosx_11_0", arch)
    if system.startswith("win"):
        return ("win", arch)
    message = f"unsupported platform: {system}"
    _fail(message)


def _verify_archive(archive: Path, expected_hash: str) -> None:
    digest = hashlib.sha256(archive.read_bytes()).digest()
    actual_hash = f"sha256-{base64.b64encode(digest).decode()}"
    if actual_hash != expected_hash:
        message = (
            f"hash mismatch for {archive.name}: expected {expected_hash}, "
            f"got {actual_hash}"
        )
        _fail(message)


def _safe_extract(archive: Path, destination: Path) -> None:
    with tarfile.open(archive, "r:gz") as tar:
        for member in tar.getmembers():
            parts = Path(member.name).parts
            if len(parts) < MIN_ARCHIVE_PARTS:
                continue
            relative_name = Path(*parts[1:])
            member_path = destination / relative_name
            if not member_path.resolve().is_relative_to(destination.resolve()):
                message = f"unsafe archive member: {member.name}"
                _fail(message)
        for member in tar.getmembers():
            parts = Path(member.name).parts
            if len(parts) < MIN_ARCHIVE_PARTS:
                continue
            member.name = str(Path(*parts[1:]))
            tar.extract(member, destination)


def _download_archive(url: str, destination: Path) -> None:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https":
        message = f"unsupported URL scheme for hugrenv download: {parsed.scheme}"
        _fail(message)
    with urllib.request.urlopen(url) as response, destination.open("wb") as file_obj:  # noqa: S310
        shutil.copyfileobj(response, file_obj)


def _main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root",
        default=".",
        help="repository root containing hugrenv.lock",
    )
    parser.add_argument("--dest", required=True, help="destination directory")
    parser.add_argument(
        "--package",
        action="append",
        dest="packages",
        required=True,
        help="package to install; may be repeated",
    )
    args = parser.parse_args()

    repo_root = Path(args.root).resolve()
    lockfile = repo_root / "hugrenv.lock"
    lock_data = json.loads(lockfile.read_text())

    platform_tag, arch = _detect_platform()
    version = lock_data["version"]
    try:
        hashes = lock_data["hashes"][platform_tag][arch]
    except KeyError as exc:
        message = f"missing lockfile entry for {platform_tag}_{arch}"
        raise SystemExit(message) from exc

    destination = Path(args.dest).resolve()
    shutil.rmtree(destination, ignore_errors=True)
    destination.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)
        for package in args.packages:
            if package not in hashes:
                message = f"missing lockfile entry for package {package}"
                _fail(message)
            asset_tag = f"{platform_tag}_{arch}"
            url = (
                "https://github.com/Quantinuum/hugrverse-env/releases/download/"
                f"v{version}/hugrenv-{package}-{asset_tag}.tar.gz"
            )
            archive_path = temp_path / f"{package}.tar.gz"
            sys.stderr.write(f"downloading {url}\n")
            _download_archive(url, archive_path)
            _verify_archive(archive_path, hashes[package])
            _safe_extract(archive_path, destination)


if __name__ == "__main__":
    _main()
