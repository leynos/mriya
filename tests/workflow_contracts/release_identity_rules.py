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
from typing import Any, NamedTuple

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

#: The optional inputs and the fields that make each one optional.
EXPECTED_INPUT_NAMES = ("maintainer", "macos-identifier")
EXPECTED_INPUT = {"required": False, "type": "string", "default": ""}

#: The metadata outputs and what each must map to.
EXPECTED_OUTPUTS = (
    ("maintainer", MAINTAINER_OUTPUT),
    ("macos_identifier", IDENTIFIER_OUTPUT),
)

#: The resolution step candidates and their required expressions.
EXPECTED_CANDIDATES = (
    ("MAINTAINER_CANDIDATE", MAINTAINER_CANDIDATE),
    ("MACOS_IDENTIFIER_CANDIDATE", IDENTIFIER_CANDIDATE),
)


class PackagingInput(NamedTuple):
    """One packaging-step input the contract holds.

    Grouping the four details keeps ``_step_problems`` within the argument
    limit and states the rule once, rather than once per packaging action.
    """

    action: str
    field: str
    expected: str
    noun: str


PACKAGING_INPUTS = (
    PackagingInput(
        LINUX_PACKAGES_ACTION, "maintainer", MAINTAINER_EXPRESSION, "maintainer"
    ),
    PackagingInput(
        MACOS_PACKAGE_ACTION, "identifier", IDENTIFIER_EXPRESSION, "identifier"
    ),
)


def normalized(value: Any) -> str:
    """Return the value as text with runs of whitespace collapsed.

    A folded YAML scalar joins its lines with spaces, so an expression split
    across lines must compare equal to its single-line spelling.
    """
    return " ".join(str(value).split())


def _workflow_call(workflow: dict[str, Any]) -> dict[str, Any]:
    """Return the ``workflow_call`` trigger, tolerating a YAML 1.1 ``True`` key."""
    trigger = workflow.get("on", workflow.get(True, {})) or {}
    return trigger.get("workflow_call", {})


def packaging_steps(
    workflow: dict[str, Any], action: str
) -> Iterator[tuple[str, int, dict[str, Any]]]:
    """Yield ``(job_id, index, step)`` for every step using ``action``."""
    for job_id, job in workflow.get("jobs", {}).items():
        for index, step in enumerate(job.get("steps", [])):
            if action in str(step.get("uses", "")):
                yield job_id, index, step


def _identity_step(workflow: dict[str, Any]) -> dict[str, Any] | None:
    """Return the metadata job's identity resolution step, if it has one."""
    metadata = workflow.get("jobs", {}).get("metadata", {})
    for step in metadata.get("steps", []):
        if step.get("id") == IDENTITY_STEP:
            return step
    return None


def _matches_optional_string(spec: Any) -> bool:
    """Return whether an input spec is an optional string with an empty default."""
    if not isinstance(spec, dict):
        return False
    return {key: spec.get(key) for key in EXPECTED_INPUT} == EXPECTED_INPUT


def _input_problems(workflow: dict[str, Any], file: str) -> list[str]:
    """Return the complaints about the optional ``workflow_call`` inputs."""
    inputs = _workflow_call(workflow).get("inputs", {})
    return [
        f"{file}: the workflow_call input {name!r} must be an optional string"
        " with an empty default"
        for name in EXPECTED_INPUT_NAMES
        if not _matches_optional_string(inputs.get(name))
    ]


def _output_problems(workflow: dict[str, Any], file: str) -> list[str]:
    """Return the complaints about the metadata job's identity outputs."""
    metadata = workflow.get("jobs", {}).get("metadata", {})
    outputs = metadata.get("outputs", {})
    problems = [
        f"{file}: the metadata job does not declare the {name!r} output"
        for name, _ in EXPECTED_OUTPUTS
        if name not in outputs
    ]
    problems.extend(
        f"{file}: the metadata output {name!r} is {normalized(outputs[name])!r},"
        f" not {expected!r}"
        for name, expected in EXPECTED_OUTPUTS
        if name in outputs and normalized(outputs[name]) != expected
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
    env = step.get("env", {})
    return [
        f"{file}: {variable} is {normalized(env.get(variable, ''))!r},"
        f" not {expected!r}"
        for variable, expected in EXPECTED_CANDIDATES
        if normalized(env.get(variable, "")) != normalized(expected)
    ]


def _expression_problems(
    where: str, value: str, expected: str, noun: str
) -> list[str]:
    """Return the complaints about one packaging input value."""
    if not value:
        return [f"{where}: passes an empty {noun}"]
    if value != expected:
        return [f"{where}: passes the literal {noun} {value!r}, not the metadata output"]
    return []


def _step_problems(
    workflow: dict[str, Any], file: str, contract: PackagingInput
) -> list[str]:
    """Return the complaints about one kind of packaging step's input."""
    problems: list[str] = []
    for job_id, index, step in packaging_steps(workflow, contract.action):
        where = f"{file}:{job_id}:step {index}"
        value = str(step.get("with", {}).get(contract.field, "")).strip()
        problems.extend(
            _expression_problems(where, value, contract.expected, contract.noun)
        )
    return problems


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
        for contract in PACKAGING_INPUTS
        for _ in packaging_steps(workflow, contract.action)
    )
    if not judged:
        return [], 0
    problems = (
        _input_problems(workflow, file)
        + _output_problems(workflow, file)
        + _fallback_problems(workflow, file)
    )
    for contract in PACKAGING_INPUTS:
        problems.extend(_step_problems(workflow, file, contract))
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