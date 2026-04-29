"""Regression tests for `main.py` output handling."""

import atexit
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MAIN = ROOT / "main.py"
MAIN_TIMEOUT_SECONDS = 120
ZIG_CACHE_ROOT = Path(tempfile.mkdtemp(prefix="qir-qis-main-test-"))


def cleanup_zig_cache_root() -> None:
    """Best-effort cleanup for the shared Zig caches created during this run."""
    shutil.rmtree(ZIG_CACHE_ROOT, ignore_errors=True)


atexit.register(cleanup_zig_cache_root)


def run_main(*args: str) -> str:
    """Run `main.py` with shared Zig caches and return stdout."""
    env = os.environ.copy()
    env["ZIG_GLOBAL_CACHE_DIR"] = str(ZIG_CACHE_ROOT / "zig-global-cache")
    env["ZIG_LOCAL_CACHE_DIR"] = str(ZIG_CACHE_ROOT / "zig-local-cache")
    try:
        proc = subprocess.run(  # noqa: S603
            [sys.executable, str(MAIN), *args],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            env=env,
            timeout=MAIN_TIMEOUT_SECONDS,
        )
    except subprocess.CalledProcessError as exc:
        message = f"stdout:\n{exc.stdout}\n\nstderr:\n{exc.stderr}"
        raise AssertionError(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = (
            f"timed out after {MAIN_TIMEOUT_SECONDS}s\n\n"
            f"stdout:\n{exc.stdout}\n\nstderr:\n{exc.stderr}"
        )
        raise AssertionError(message) from exc
    return proc.stdout


def run_main_spec(fixture: str) -> str:
    """Run `main.py --spec` on a fixture and return stdout."""
    return run_main("--spec", fixture)


def test_base_spec_output() -> None:
    """`base.ll` should emit baseline scalar results in spec format."""
    output = run_main_spec("tests/data/base.ll")
    assert "HEADER  schema_id       labeled" in output, output  # noqa: S101
    assert "OUTPUT  TUPLE   2       t0" in output, output  # noqa: S101
    assert "OUTPUT  RESULT  " in output, output  # noqa: S101
    assert "OUTPUT  RESULT_ARRAY" not in output, output  # noqa: S101


def test_base_plain_output() -> None:
    """`base.ll` should emit labeled non-spec output through `main.py`."""
    output = run_main("tests/data/base.ll")
    assert "'output_labeling_schema': 'labeled'" in output, output  # noqa: S101
    assert "USER:QIRTUPLE:t0" in output, output  # noqa: S101
    assert "USER:RESULT:r1" in output, output  # noqa: S101
    assert "USER:RESULT:r2" in output, output  # noqa: S101


def test_dynamic_result_array_spec_output() -> None:
    """`dynamic_result_array.ll` should emit spec-formatted result arrays."""
    output = run_main_spec("tests/data/dynamic_result_array.ll")
    assert "OUTPUT  RESULT_ARRAY    00      a0" in output, output  # noqa: S101
    assert "USER:RESULT_ARRAY" not in output, output  # noqa: S101


def test_dynamic_result_mixed_array_spec_output() -> None:
    """`dynamic_result_mixed_array_output.ll` should preserve the array label."""
    output = run_main_spec("tests/data/dynamic_result_mixed_array_output.ll")
    assert "OUTPUT  RESULT_ARRAY    00      mix0" in output, output  # noqa: S101
    assert "USER:RESULT_ARRAY" not in output, output  # noqa: S101


if __name__ == "__main__":
    test_base_spec_output()
    test_base_plain_output()
    test_dynamic_result_array_spec_output()
    test_dynamic_result_mixed_array_spec_output()
