"""Rules and fixtures for the release identity contract.

``release_identity_test`` drives these rules against synthetic cases and the
repository's own workflow. Keeping the readers here mirrors
``suite_lane_rules``: the test module holds the cases, and this module holds
the judgement.

The expressions are compared as text: these rules do not evaluate GitHub
expressions. The resolver's ``run`` body is executed separately, because a
wiring check alone would not notice an empty script.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"
RELEASE = WORKFLOWS / "release.yml"

#: The shared actions whose steps consume the resolved identity.
LINUX_PACKAGES_ACTION = "linux-packages@"
MACOS_PACKAGE_ACTION = "macos-package@"

#: The metadata step that resolves the identity.
IDENTITY_STEP = "release_identity"

#: The metadata outputs the packaging steps must read.
MAINTAINER_EXPRESSION = "${{ needs.metadata.outputs.maintainer }}"
IDENTIFIER_EXPRESSION = "${{ needs.metadata.outputs.macos_identifier }}"

#: What the metadata outputs must map to.
MAINTAINER_OUTPUT = "${{ steps.release_identity.outputs.maintainer }}"
IDENTIFIER_OUTPUT = "${{ steps.release_identity.outputs.macos_identifier }}"

#: The maintainer the workflow used before it read configuration.
MAINTAINER_DEFAULT = "Payton McIntosh <pmcintosh@df12.net>"

#: The candidates the resolution step receives. Each is the exact text a
#: folded YAML scalar produces, so the default must stay last: an earlier
#: operand would win and make the overrides unreachable.
MAINTAINER_CANDIDATE = (
    "${{ inputs.maintainer || vars.RELEASE_MAINTAINER || '"
    + MAINTAINER_DEFAULT
    + "' }}"
)
IDENTIFIER_CANDIDATE = (
    "${{ inputs.macos-identifier || vars.RELEASE_MACOS_IDENTIFIER || "
    "format('net.df12.{0}', steps.bin_name.outputs.bin-name) }}"
)


def normalised(value: Any) -> str:
    """Return the value as text with runs of whitespace collapsed.

    A folded YAML scalar joins its lines with spaces, so an expression split
    across lines must compare equal to its single-line spelling.
    """
    return " ".join(str(value).split())


def _workflow_call(workflow: dict[str, Any]) -> dict[str, Any]:
    """Return the ``workflow_call`` trigger, tolerating a YAML 1.1 ``True`` key."""
    trigger = workflow.get("on") or workflow.get(True) or {}
    return trigger.get("workflow_call") or {}


def packaging_steps(
    workflow: dict[str, Any], action: str
) -> Iterator[tuple[str, int, dict[str, Any]]]:
    """Yield ``(job_id, index, step)`` for every step using ``action``."""
    for job_id, job in (workflow.get("jobs") or {}).items():
        for index, step in enumerate(job.get("steps") or []):
            if action in str(step.get("uses", "")):
                yield job_id, index, step


def _identity_step(workflow: dict[str, Any]) -> dict[str, Any] | None:
    """Return the metadata job's identity resolution step, if it has one."""
    metadata = (workflow.get("jobs") or {}).get("metadata") or {}
    for step in metadata.get("steps") or []:
        if step.get("id") == IDENTITY_STEP:
            return step
    return None


def _input_problems(workflow: dict[str, Any], file: str) -> list[str]:
    """Return the complaints about the optional ``workflow_call`` inputs."""
    inputs = _workflow_call(workflow).get("inputs") or {}
    problems: list[str] = []
    for name in ("maintainer", "macos-identifier"):
        spec = inputs.get(name)
        if spec is None:
            problems.append(f"{file}: the workflow_call input {name!r} is missing")
            continue
        if spec.get("required") is not False:
            problems.append(f"{file}: the workflow_call input {name!r} must be optional")
        if spec.get("type") != "string":
            problems.append(f"{file}: the workflow_call input {name!r} must be a string")
        if "default" not in spec or spec["default"] != "":
            problems.append(
                f"{file}: the workflow_call input {name!r} must default to an empty string"
            )
    return problems


def _output_problems(workflow: dict[str, Any], file: str) -> list[str]:
    """Return the complaints about the metadata job's identity outputs."""
    metadata = (workflow.get("jobs") or {}).get("metadata") or {}
    outputs = metadata.get("outputs") or {}
    problems: list[str] = []
    for name, expected in (
        ("maintainer", MAINTAINER_OUTPUT),
        ("macos_identifier", IDENTIFIER_OUTPUT),
    ):
        if name not in outputs:
            problems.append(f"{file}: the metadata job does not declare the {name!r} output")
            continue
        actual = normalised(outputs[name])
        if actual != expected:
            problems.append(
                f"{file}: the metadata output {name!r} is {actual!r}, not {expected!r}"
            )
    return problems


def _fallback_problems(workflow: dict[str, Any], file: str) -> list[str]:
    """Return the complaints about the resolution step's candidate expressions.

    Each value is compared whole rather than by fragments: a default placed
    before an override contains every fragment while making the override
    unreachable, and a malformed expression can still contain the pieces.
    """
    step = _identity_step(workflow)
    if step is None:
        return [f"{file}: no {IDENTITY_STEP!r} step resolves the release identity"]
    env = step.get("env") or {}
    problems: list[str] = []
    for variable, expected in (
        ("MAINTAINER_CANDIDATE", MAINTAINER_CANDIDATE),
        ("MACOS_IDENTIFIER_CANDIDATE", IDENTIFIER_CANDIDATE),
    ):
        actual = normalised(env.get(variable, ""))
        if actual != normalised(expected):
            problems.append(f"{file}: {variable} is {actual!r}, not {expected!r}")
    return problems


def _maintainer_problems(where: str, value: str) -> list[str]:
    """Return the complaints about one Linux packaging step's maintainer input."""
    if not value:
        return [f"{where}: passes an empty maintainer"]
    if value != MAINTAINER_EXPRESSION:
        return [
            f"{where}: passes the literal maintainer {value!r}, not the metadata output"
        ]
    return []


def _identifier_problems(where: str, value: str) -> list[str]:
    """Return the complaints about one macOS packaging step's identifier input."""
    if not value:
        return [f"{where}: passes an empty identifier"]
    if value != IDENTIFIER_EXPRESSION:
        return [
            f"{where}: passes the literal identifier {value!r}, not the metadata output"
        ]
    return []


def identity_problems(workflow: dict[str, Any], file: str) -> tuple[list[str], int]:
    """Return the complaints about a workflow's release identity and their count.

    A workflow with no packaging step is not judged: only a workflow that
    packages artefacts needs the inputs, outputs and resolution step.

    Parameters
    ----------
    workflow:
        The parsed workflow.
    file:
        The file name, for complaints.

    Returns
    -------
    tuple[list[str], int]
        The complaints, and how many packaging steps were judged.
    """
    judged = sum(
        1
        for action in (LINUX_PACKAGES_ACTION, MACOS_PACKAGE_ACTION)
        for _ in packaging_steps(workflow, action)
    )
    if not judged:
        return [], 0
    problems = _input_problems(workflow, file)
    problems.extend(_output_problems(workflow, file))
    problems.extend(_fallback_problems(workflow, file))
    for job_id, index, step in packaging_steps(workflow, LINUX_PACKAGES_ACTION):
        where = f"{file}:{job_id}:step {index}"
        value = str((step.get("with") or {}).get("maintainer", "")).strip()
        problems.extend(_maintainer_problems(where, value))
    for job_id, index, step in packaging_steps(workflow, MACOS_PACKAGE_ACTION):
        where = f"{file}:{job_id}:step {index}"
        value = str((step.get("with") or {}).get("identifier", "")).strip()
        problems.extend(_identifier_problems(where, value))
    return problems, judged


def resolver_script() -> str:
    """Return the real workflow's identity resolution script."""
    workflow = yaml.safe_load(RELEASE.read_text(encoding="utf-8"))
    step = _identity_step(workflow)
    assert step is not None, "no release_identity step found, so the resolver cannot be run"
    return str(step.get("run", ""))


def run_resolver(
    maintainer: str, identifier: str, output: Path
) -> subprocess.CompletedProcess[str]:
    """Run the real resolver with the candidates and a disposable output file."""
    return subprocess.run(
        ["bash", "-c", resolver_script()],
        env={
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "MAINTAINER_CANDIDATE": maintainer,
            "MACOS_IDENTIFIER_CANDIDATE": identifier,
            "GITHUB_OUTPUT": str(output),
        },
        capture_output=True,
        text=True,
        check=False,
    )