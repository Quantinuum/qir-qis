"""Build a wheel and repair external shared-library dependencies."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import NoReturn


def _fail(message: str) -> NoReturn:
    raise SystemExit(message)


def _run(command: list[str], *, env: dict[str, str] | None = None) -> None:
    subprocess.run(command, check=True, env=env)  # noqa: S603


def _find_wheel(directory: Path) -> Path:
    wheels = sorted(directory.glob("*.whl"))
    if len(wheels) != 1:
        message = f"expected exactly one wheel in {directory}, found {len(wheels)}"
        _fail(message)
    return wheels[0]


def _repair_env() -> dict[str, str]:
    env = os.environ.copy()
    llvm_prefix = env.get("LLVM_SYS_211_PREFIX")
    if llvm_prefix is None:
        return env

    lib_paths = [str(Path(llvm_prefix) / "lib"), str(Path(llvm_prefix) / "lib64")]
    if sys.platform == "darwin":
        current = env.get("DYLD_FALLBACK_LIBRARY_PATH", "")
        env["DYLD_FALLBACK_LIBRARY_PATH"] = ":".join(
            [*lib_paths, *([current] if current else [])]
        )
    elif sys.platform.startswith("linux"):
        current = env.get("LD_LIBRARY_PATH", "")
        env["LD_LIBRARY_PATH"] = ":".join([*lib_paths, *([current] if current else [])])
    return env


def _repair_wheel(raw_wheel: Path, out_dir: Path) -> None:
    env = _repair_env()
    if sys.platform.startswith("linux"):
        _run(
            [
                "uvx",
                "--from",
                "auditwheel",
                "auditwheel",
                "repair",
                "-w",
                str(out_dir),
                str(raw_wheel),
            ],
            env=env,
        )
        return

    if sys.platform == "darwin":
        _run(
            [
                "uvx",
                "--from",
                "delocate",
                "delocate-wheel",
                "-w",
                str(out_dir),
                str(raw_wheel),
            ],
            env=env,
        )
        return

    if sys.platform.startswith("win"):
        _run(
            [
                "uvx",
                "--from",
                "delvewheel",
                "delvewheel",
                "repair",
                "-w",
                str(out_dir),
                str(raw_wheel),
            ],
            env=env,
        )
        return

    message = f"unsupported platform for wheel repair: {sys.platform}"
    _fail(message)


def _main() -> None:
    repo_root = Path(__file__).resolve().parents[2]
    dist_dir = repo_root / "dist"
    raw_dir = dist_dir / ".raw"

    shutil.rmtree(raw_dir, ignore_errors=True)
    raw_dir.mkdir(parents=True, exist_ok=True)

    _run(["uv", "build", "--wheel", "--out-dir", str(raw_dir), str(repo_root)])
    raw_wheel = _find_wheel(raw_dir)
    _repair_wheel(raw_wheel, dist_dir)


if __name__ == "__main__":
    _main()
