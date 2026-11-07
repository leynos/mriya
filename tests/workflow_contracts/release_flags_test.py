"""Contract: the release build leaves the development build standard explicitly.

``.cargo/config.toml`` names the parallel ``rustc`` frontend and, on Linux,
``mold``. Cargo reads one ``rustflags`` source, so an assigned ``RUSTFLAGS``
displaces the whole file, and the shared ``rust-build-release`` action exports
its ``rustflags`` input as ``RUSTFLAGS`` before its toolchain setup. Left empty,
that input falls back to the nested ``setup-rust`` default, which happens to be
``-D warnings``; the release would then be excluded by a side effect nobody
wrote down. The step therefore states the value, and these tests hold it: the
value is non-empty, denies warnings, and names neither standard flag.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
import yaml

WORKFLOWS = Path(__file__).resolve().parents[2] / ".github" / "workflows"

#: The shared action whose steps build the shipped artefact.
RELEASE_ACTION = "rust-build-release@"

#: Words that mark a flag the development standard owns.
STANDARD_FLAG_MARKERS = ("-Zthreads", "mold", "-fuse-ld")


def release_problems(workflow: dict[str, Any], file: str) -> tuple[list[str], int]:
    """Return the complaints about a workflow's release-build steps and their count.

    Parameters
    ----------
    workflow:
        The parsed workflow.
    file:
        The file name, for complaints.

    Returns
    -------
    tuple[list[str], int]
        The complaints, and how many release-build steps were judged.
    """
    problems: list[str] = []
    judged = 0
    for job_id, job in (workflow.get("jobs") or {}).items():
        for index, step in enumerate(job.get("steps") or []):
            if RELEASE_ACTION not in str(step.get("uses", "")):
                continue
            judged += 1
            where = f"{file}:{job_id}:step {index}"
            value = str((step.get("with") or {}).get("rustflags", "")).strip()
            if not value:
                problems.append(f"{where}: passes no rustflags, so the exclusion is implicit")
                continue
            if "-D warnings" not in value and "-Dwarnings" not in value:
                problems.append(f"{where}: rustflags {value!r} does not deny warnings")
            problems.extend(
                f"{where}: rustflags {value!r} names the development flag {marker}"
                for marker in STANDARD_FLAG_MARKERS
                if marker in value
            )
    return problems, judged


def _step(rustflags: str | None) -> dict[str, Any]:
    """Return a one-job workflow whose release step passes ``rustflags``."""
    with_block: dict[str, str] = {"target": "x86_64-unknown-linux-gnu"}
    if rustflags is not None:
        with_block["rustflags"] = rustflags
    return {
        "jobs": {
            "build": {"steps": [{"uses": f"org/a/.github/actions/{RELEASE_ACTION}abc", "with": with_block}]}
        }
    }


@pytest.mark.parametrize(
    ("rustflags", "fragment"),
    [
        ("-D warnings", ""),
        (None, "implicit"),
        ("", "implicit"),
        ("-D warnings -Zthreads=8", "-Zthreads"),
        ("-D warnings -Clink-arg=-fuse-ld=mold", "mold"),
        ("-C opt-level=3", "does not deny warnings"),
    ],
    ids=["explicit", "absent", "empty", "frontend_leak", "linker_leak", "no_warning_policy"],
)
def test_release_step_cases(rustflags: str | None, fragment: str) -> None:
    """Each way of weakening the exclusion is a complaint; the explicit form is not."""
    problems, judged = release_problems(_step(rustflags), "w.yml")
    assert judged == 1
    if fragment:
        assert any(fragment in problem for problem in problems), problems
    else:
        assert not problems, problems


def test_the_repository_release_builds_are_explicitly_excluded() -> None:
    """Every real release-build step states its exclusion, and one is found."""
    problems: list[str] = []
    judged = 0
    for path in sorted(WORKFLOWS.glob("*.yml")):
        workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
        found, count = release_problems(workflow, path.name)
        problems.extend(found)
        judged += count
    assert judged > 0, "no release-build step found, so the contract proves nothing"
    assert not problems, problems
