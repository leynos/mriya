"""Contract: the release workflow resolves its package identity from configuration.

The Linux package maintainer and the macOS package identifier were literals in
``.github/workflows/release.yml``. Both now resolve once, in the ``metadata``
job, from an optional ``workflow_call`` input, then a repository variable, then
a default. The packaging steps read the metadata outputs, so a literal left
behind in either place, a missing output, a missing input or an empty
identifier fails ``make test-workflow-contracts``.

The expressions themselves are compared as text: these tests do not evaluate
GitHub expressions.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Iterator
from pathlib import Path
from typing import Any

import pytest
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

#: The literal values that used to sit in the packaging steps.
OLD_MAINTAINER = MAINTAINER_DEFAULT
OLD_IDENTIFIER = "net.df12.mriya"

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


def _normalised(value: Any) -> str:
    """Return the value as text with runs of whitespace collapsed.

    A folded YAML scalar joins its lines with spaces, so an expression split
    across lines must compare equal to its single-line spelling.
    """
    return " ".join(str(value).split())


def _workflow_call(workflow: dict[str, Any]) -> dict[str, Any]:
    """Return the ``workflow_call`` trigger, tolerating a YAML 1.1 ``True`` key."""
    trigger = workflow.get("on") or workflow.get(True) or {}
    return trigger.get("workflow_call") or {}


def _packaging_steps(
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
        actual = _normalised(outputs[name])
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
        actual = _normalised(env.get(variable, ""))
        if actual != _normalised(expected):
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
        for _ in _packaging_steps(workflow, action)
    )
    if not judged:
        return [], 0
    problems = _input_problems(workflow, file)
    problems.extend(_output_problems(workflow, file))
    problems.extend(_fallback_problems(workflow, file))
    for job_id, index, step in _packaging_steps(workflow, LINUX_PACKAGES_ACTION):
        where = f"{file}:{job_id}:step {index}"
        value = str((step.get("with") or {}).get("maintainer", "")).strip()
        problems.extend(_maintainer_problems(where, value))
    for job_id, index, step in _packaging_steps(workflow, MACOS_PACKAGE_ACTION):
        where = f"{file}:{job_id}:step {index}"
        value = str((step.get("with") or {}).get("identifier", "")).strip()
        problems.extend(_identifier_problems(where, value))
    return problems, judged


def _synthetic(
    *,
    include_maintainer_input: bool = True,
    include_identifier_input: bool = True,
    include_maintainer_output: bool = True,
    include_identifier_output: bool = True,
    maintainer_candidate: str = MAINTAINER_CANDIDATE,
    identifier_candidate: str = IDENTIFIER_CANDIDATE,
    linux_maintainer: str = MAINTAINER_EXPRESSION,
    macos_identifier: str = IDENTIFIER_EXPRESSION,
) -> dict[str, Any]:
    """Return a workflow wiring the identity the way the contract requires."""
    inputs: dict[str, Any] = {}
    if include_maintainer_input:
        inputs["maintainer"] = {"required": False, "type": "string", "default": ""}
    if include_identifier_input:
        inputs["macos-identifier"] = {"required": False, "type": "string", "default": ""}
    outputs: dict[str, Any] = {}
    if include_maintainer_output:
        outputs["maintainer"] = MAINTAINER_OUTPUT
    if include_identifier_output:
        outputs["macos_identifier"] = IDENTIFIER_OUTPUT
    return {
        "on": {"workflow_call": {"inputs": inputs}},
        "jobs": {
            "metadata": {
                "outputs": outputs,
                "steps": [
                    {
                        "id": IDENTITY_STEP,
                        "env": {
                            "MAINTAINER_CANDIDATE": maintainer_candidate,
                            "MACOS_IDENTIFIER_CANDIDATE": identifier_candidate,
                        },
                        "run": "true",
                    }
                ],
            },
            "packaging": {
                "steps": [
                    {
                        "uses": f"org/shared/.github/actions/{LINUX_PACKAGES_ACTION}abc",
                        "with": {"maintainer": linux_maintainer},
                    },
                    {
                        "uses": f"org/shared/.github/actions/{MACOS_PACKAGE_ACTION}abc",
                        "with": {"identifier": macos_identifier},
                    },
                ]
            },
        },
    }


@pytest.mark.parametrize(
    ("overrides", "fragment"),
    [
        ({}, ""),
        ({"linux_maintainer": OLD_MAINTAINER}, "literal maintainer"),
        ({"macos_identifier": OLD_IDENTIFIER}, "literal identifier"),
        ({"include_maintainer_output": False}, "does not declare"),
        ({"macos_identifier": ""}, "empty identifier"),
        ({"include_maintainer_input": False}, "input 'maintainer' is missing"),
        ({"include_identifier_input": False}, "input 'macos-identifier' is missing"),
        ({"maintainer_candidate": "literal@example.test"}, "MAINTAINER_CANDIDATE"),
        (
            {
                "maintainer_candidate": (
                    "${{ 'Payton McIntosh <pmcintosh@df12.net>' || "
                    "inputs.maintainer || vars.RELEASE_MAINTAINER }}"
                )
            },
            "MAINTAINER_CANDIDATE",
        ),
        (
            {"maintainer_candidate": MAINTAINER_CANDIDATE.removesuffix("}")},
            "MAINTAINER_CANDIDATE",
        ),
        (
            {"identifier_candidate": "${{ inputs.macos-identifier }}"},
            "MACOS_IDENTIFIER_CANDIDATE",
        ),
        (
            {
                "identifier_candidate": (
                    "${{ inputs.macos-identifier || "
                    "vars.RELEASE_MACOS_IDENTIFIER || '' }}"
                )
            },
            "MACOS_IDENTIFIER_CANDIDATE",
        ),
        (
            {"identifier_candidate": IDENTIFIER_CANDIDATE.removesuffix("}")},
            "MACOS_IDENTIFIER_CANDIDATE",
        ),
    ],
    ids=[
        "configured",
        "literal_maintainer",
        "literal_identifier",
        "missing_output",
        "empty_identifier",
        "missing_maintainer_input",
        "missing_identifier_input",
        "maintainer_candidate_replaced",
        "maintainer_default_first",
        "maintainer_candidate_malformed",
        "identifier_variable_lost",
        "identifier_bin_name_lost",
        "identifier_candidate_malformed",
    ],
)
def test_release_identity_cases(overrides: dict[str, Any], fragment: str) -> None:
    """Each way of dropping the configuration is a complaint; the wired form is not."""
    problems, judged = identity_problems(_synthetic(**overrides), "w.yml")
    assert judged == 2
    if fragment:
        assert any(fragment in problem for problem in problems), problems
    else:
        assert not problems, problems


def test_the_repository_release_identity_is_configured() -> None:
    """The real release workflow reads both values from metadata, and both are judged."""
    workflow = yaml.safe_load(RELEASE.read_text(encoding="utf-8"))
    problems, judged = identity_problems(workflow, RELEASE.name)
    linux_steps = list(_packaging_steps(workflow, LINUX_PACKAGES_ACTION))
    macos_steps = list(_packaging_steps(workflow, MACOS_PACKAGE_ACTION))
    assert linux_steps, "no linux-packages step found, so the maintainer contract proves nothing"
    assert macos_steps, "no macos-package step found, so the identifier contract proves nothing"
    assert judged == len(linux_steps) + len(macos_steps)
    assert not problems, problems


def _resolver_script() -> str:
    """Return the real workflow's identity resolution script."""
    workflow = yaml.safe_load(RELEASE.read_text(encoding="utf-8"))
    step = _identity_step(workflow)
    assert step is not None, "no release_identity step found, so the resolver cannot be run"
    return str(step.get("run", ""))


def _run_resolver(
    maintainer: str, identifier: str, output: Path
) -> subprocess.CompletedProcess[str]:
    """Run the real resolver with the candidates and a disposable output file."""
    return subprocess.run(
        ["bash", "-c", _resolver_script()],
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


@pytest.mark.parametrize(
    ("maintainer", "identifier", "expected"),
    [
        (
            MAINTAINER_DEFAULT,
            "net.df12.mriya",
            f"maintainer={MAINTAINER_DEFAULT}\nmacos_identifier=net.df12.mriya\n",
        ),
        (
            "  Ada Lovelace <ada@example.test>  ",
            "\tcom.example.mriya\t",
            "maintainer=Ada Lovelace <ada@example.test>\n"
            "macos_identifier=com.example.mriya\n",
        ),
    ],
    ids=["exact", "trimmed"],
)
def test_the_resolver_publishes_trimmed_identities(
    maintainer: str, identifier: str, expected: str, tmp_path: Path
) -> None:
    """Valid candidates are trimmed and written to the step outputs."""
    output = tmp_path / "output"
    output.write_text("", encoding="utf-8")
    result = _run_resolver(maintainer, identifier, output)
    assert result.returncode == 0, result.stdout + result.stderr
    assert output.read_text(encoding="utf-8") == expected


@pytest.mark.parametrize(
    ("maintainer", "identifier"),
    [
        ("   ", "net.df12.mriya"),
        (MAINTAINER_DEFAULT, "   "),
        ("Ada <a@b.test>\nEvil <e@b.test>", "net.df12.mriya"),
        (MAINTAINER_DEFAULT, "net.df12.mriya\nnet.df12.other"),
        (MAINTAINER_DEFAULT, "net.df12.mriya extra"),
        (MAINTAINER_DEFAULT, "net.df12.mriya\textra"),
    ],
    ids=[
        "empty_maintainer",
        "empty_identifier",
        "multi_line_maintainer",
        "multi_line_identifier",
        "identifier_space",
        "identifier_tab",
    ],
)
def test_the_resolver_refuses_unusable_identities(
    maintainer: str, identifier: str, tmp_path: Path
) -> None:
    """An empty, multi-line or whitespace-bearing value fails and publishes nothing."""
    output = tmp_path / "output"
    output.write_text("", encoding="utf-8")
    result = _run_resolver(maintainer, identifier, output)
    assert result.returncode != 0, result.stdout + result.stderr
    assert output.read_text(encoding="utf-8") == ""
    assert "::error::" in result.stdout