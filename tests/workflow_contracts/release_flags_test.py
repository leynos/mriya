"""Contract: the release build leaves the development build standard explicitly.

``.cargo/config.toml`` names the parallel ``rustc`` frontend and, on Linux,
``mold``. Cargo reads one ``rustflags`` source, so an assigned ``RUSTFLAGS``
displaces the whole file, and the shared ``rust-build-release`` action exports
its ``rustflags`` input as ``RUSTFLAGS`` before its toolchain setup. Left empty,
that input falls back to the nested ``setup-rust`` default, which happens to be
``-D warnings``; the release would then be excluded by a side effect nobody
wrote down. The step therefore states the value, and these tests hold it: the
value is non-empty, denies warnings with no lint cap undoing it, and names
neither standard flag.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

import shlex
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
import yaml

WORKFLOWS = Path(__file__).resolve().parents[2] / ".github" / "workflows"

#: The shared action whose steps build the shipped artefact.
RELEASE_ACTION = "rust-build-release@"

#: The linker's name. The dictionary reads it as a misspelling of ``mould``, so
#: it is exempted from spelling on this declaration line alone.
LINKER_NAME = "mold"

#: Words that mark a flag the development standard owns.
STANDARD_FLAG_MARKERS = ("-Zthreads", LINKER_NAME, "-fuse-ld")

#: Lint-cap levels that leave ``-D warnings`` without effect.
CAPPING_LEVELS = frozenset({"allow", "warn"})


def _release_steps(
    workflow: dict[str, Any],
) -> Iterator[tuple[str, int, dict[str, Any]]]:
    """Yield ``(job_id, index, step)`` for every release-build step."""
    for job_id, job in (workflow.get("jobs") or {}).items():
        for index, step in enumerate(job.get("steps") or []):
            if RELEASE_ACTION in str(step.get("uses", "")):
                yield job_id, index, step


def _cap_level(tokens: list[str]) -> str | None:
    """Return the level a ``--cap-lints`` option names, if one is present."""
    for index, token in enumerate(tokens):
        if token == "--cap-lints" and index + 1 < len(tokens):
            return tokens[index + 1]
        if token.startswith("--cap-lints="):
            return token.partition("=")[2]
    return None


def _denies_warnings(tokens: list[str]) -> bool:
    """Return whether the tokens deny warnings, with no lower lint cap on top.

    ``--cap-lints`` takes precedence over ``-D warnings``, so a cap of ``allow``
    or ``warn`` leaves the release without the warning policy it appears to
    state. ``deny`` and ``forbid`` cap at or above the denial and keep it.
    """
    if _cap_level(tokens) in CAPPING_LEVELS:
        return False
    if "-Dwarnings" in tokens:
        return True
    return any(
        first == "-D" and second == "warnings"
        for first, second in zip(tokens, tokens[1:])
    )


def _joined(tokens: list[str]) -> list[str]:
    """Return the tokens with ``-Z value`` and ``-C value`` spells joined.

    ``rustc`` accepts ``-Zthreads=8`` and ``-Z threads=8`` alike, so a marker
    check must read both as ``-Zthreads=8`` rather than miss the spaced form.
    """
    joined: list[str] = []
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token in {"-Z", "-C"} and index + 1 < len(tokens):
            joined.append(f"{token}{tokens[index + 1]}")
            index += 2
        else:
            joined.append(token)
            index += 1
    return joined


def _names_marker(tokens: list[str], marker: str) -> bool:
    """Return whether a token names the development flag ``marker``.

    ``-Zthreads`` and ``-fuse-ld`` appear inside a longer token, while a bare
    linker name is a whole word. Matching the joined text would reject an
    unrelated value such as ``--cfg=xmoldx``.
    """
    if marker == LINKER_NAME:
        return marker in tokens
    return any(marker in token for token in tokens)


def _rustflags_problems(where: str, value: str) -> list[str]:
    """Return the complaints about one release step's ``rustflags`` value."""
    if not value:
        return [f"{where}: passes no rustflags, so the exclusion is implicit"]
    tokens = shlex.split(value)
    joined = _joined(tokens)
    problems: list[str] = []
    if not _denies_warnings(tokens):
        problems.append(f"{where}: rustflags {value!r} does not deny warnings")
    problems.extend(
        f"{where}: rustflags {value!r} names the development flag {marker}"
        for marker in STANDARD_FLAG_MARKERS
        if _names_marker(joined, marker)
    )
    return problems


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
    for job_id, index, step in _release_steps(workflow):
        judged += 1
        where = f"{file}:{job_id}:step {index}"
        value = str((step.get("with") or {}).get("rustflags", "")).strip()
        problems.extend(_rustflags_problems(where, value))
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
        ("-D warnings -Z threads=8", "-Zthreads"),
        ("-D warnings -Clink-arg=-fuse-ld=mold", "-fuse-ld"),
        ("-D warnings -C link-arg=-fuse-ld=mold", "-fuse-ld"),
        ("-D warnings=1", "does not deny warnings"),
        ("-C opt-level=3", "does not deny warnings"),
        ("-D warnings --cap-lints allow", "does not deny warnings"),
        ("-D warnings --cap-lints=warn", "does not deny warnings"),
        ("-D warnings --cap-lints deny", ""),
        ("-D warnings --cfg=xmoldx", ""),
    ],
    ids=[
        "explicit",
        "absent",
        "empty",
        "frontend_leak",
        "spaced_frontend_leak",
        "linker_leak",
        "spaced_linker_leak",
        "warnings_not_a_pair",
        "no_warning_policy",
        "capped_by_allow",
        "capped_by_warn",
        "cap_keeps_the_policy",
        "unrelated_value_is_not_a_flag",
    ],
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
