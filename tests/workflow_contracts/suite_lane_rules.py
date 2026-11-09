"""Rules for the workflow jobs that run the test suite on Linux.

The development build standard makes ``make test`` stop in ``check-build-tools``
when the pinned ``mold`` is missing. A job that runs the suite can reach that
preflight directly (``make test``), through the coverage action, or through a
Make goal that depends on the suite, and a coverage build never links with
``mold`` yet still executes tests that start Cargo. So every Linux job that runs
the suite installs the pinned tools itself, unconditionally, before the suite.

The jobs are derived from the workflows and the Makefile, never listed, so a
newly added suite job is held to the rule without anyone editing this file. A
reading that finds nothing is refused instead of passing: a contract that
matches no job proves nothing.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

import re
from collections.abc import Mapping
from typing import Any

#: The command that installs the pinned tools.
INSTALL_COMMAND = "make install-build-tools"

#: Actions whose step runs the suite itself.
SUITE_ACTION_MARKERS = ("generate-coverage@",)

#: Command text that runs the suite directly.
SUITE_COMMAND_RE = re.compile(r"\bnextest\s+run\b|\bcargo-nextest\b")

#: The installer as a command: at the start of a line or after a separator, and
#: not merely named inside another word or an ``echo``.
_INSTALL_RE = re.compile(
    r"(?:^|&&|;|\|\|)\s*make\s+install-build-tools(?![\w-])", re.MULTILINE
)

#: A Make target line: name(s), a colon that is not ``:=``, then prerequisites.
_TARGET_RE = re.compile(r"^([A-Za-z0-9_./%-]+(?:\s+[A-Za-z0-9_./%-]+)*)\s*:(?!=)\s*([^#]*)")

#: ``make goal`` or ``$(MAKE) goal`` inside a recipe.
_SUBMAKE_RE = re.compile(r"(?:\$\(MAKE\)|\bmake)\s+(?:--?[\w-]+\s+)*([A-Za-z0-9_./-]+)")

#: A ``${{ matrix.key }}`` reference.
_MATRIX_REF_RE = re.compile(r"^\$\{\{\s*matrix\.([A-Za-z0-9_-]+)\s*\}\}$")


class ContractGapError(Exception):
    """Raised when the contract cannot read what it must judge.

    A gap is reported, never treated as a pass.
    """


def _is_rule_line(line: str) -> bool:
    """Return whether a line may declare a target rather than be a directive."""
    return not line.startswith((".PHONY", "#"))


def _prerequisites(text: str) -> list[str]:
    """Return the prerequisites on a rule line, with order-only ones removed."""
    return [word for word in text.split() if word != "|"]


def _add_rule_line(
    targets: dict[str, dict[str, list[str]]], line: str
) -> list[str]:
    """Record the targets a rule line declares and return their names."""
    match = _TARGET_RE.match(line)
    if match is None or not _is_rule_line(line):
        return []
    names = match.group(1).split()
    prerequisites = _prerequisites(match.group(2))
    for name in names:
        entry = targets.setdefault(name, {"prerequisites": [], "recipe": []})
        entry["prerequisites"].extend(prerequisites)
    return names


def _add_recipe_line(
    targets: dict[str, dict[str, list[str]]], names: list[str], line: str
) -> None:
    """Append one recipe line to every target the current rule declares."""
    for name in names:
        targets[name]["recipe"].append(line.strip())


def parse_makefile(text: str) -> dict[str, dict[str, list[str]]]:
    """Read each Make target's prerequisites and recipe lines.

    Parameters
    ----------
    text:
        The Makefile text.

    Returns
    -------
    dict[str, dict[str, list[str]]]
        Target name to ``{"prerequisites": [...], "recipe": [...]}``. Targets
        that share a rule line are each given the same entry, and a target
        named twice accumulates its prerequisites and recipe.

    Examples
    --------
    >>> parse_makefile("test: check\\n\\tcargo nextest run\\n")["test"]["recipe"]
    ['cargo nextest run']
    """
    targets: dict[str, dict[str, list[str]]] = {}
    current: list[str] = []
    for line in text.splitlines():
        if line.startswith("\t"):
            _add_recipe_line(targets, current, line)
            continue
        current = _add_rule_line(targets, line)
    return targets


def _direct_suite_goals(
    targets: dict[str, dict[str, list[str]]],
) -> set[str]:
    """Return the goals whose own recipe runs the suite."""
    return {
        name
        for name, entry in targets.items()
        if any(SUITE_COMMAND_RE.search(line) for line in entry["recipe"])
    }


def _target_calls_suite_goal(
    entry: Mapping[str, list[str]], reaching: set[str]
) -> bool:
    """Return whether a target depends on or calls a goal that reaches the suite."""
    called = [
        match.group(1)
        for line in entry["recipe"]
        for match in _SUBMAKE_RE.finditer(line)
    ]
    return any(goal in reaching for goal in [*entry["prerequisites"], *called])


def _close_over_dependencies(
    targets: dict[str, dict[str, list[str]]], reaching: set[str]
) -> set[str]:
    """Extend the reaching goals until no further target depends on one."""
    frontier = set(reaching)
    while frontier:
        new = {
            name
            for name, entry in targets.items()
            if name not in reaching and _target_calls_suite_goal(entry, reaching)
        }
        reaching |= new
        frontier = new
    return reaching


def goals_reaching_suite(makefile: str) -> set[str]:
    """Return the Make goals whose recipe or prerequisites run the suite.

    Parameters
    ----------
    makefile:
        The Makefile text.

    Returns
    -------
    set[str]
        Goals that run ``cargo nextest`` themselves, or depend on, or call
        through ``make``, a goal that does.

    Raises
    ------
    ContractGapError
        When no goal runs the suite, so the derivation would match nothing.
    """
    targets = parse_makefile(makefile)
    reaching = _direct_suite_goals(targets)
    if not reaching:
        message = "the Makefile has no goal that runs cargo nextest"
        raise ContractGapError(message)
    return _close_over_dependencies(targets, reaching)


def _declared_matrix_values(matrix: Mapping[str, Any], key: str) -> list[Any]:
    """Return the values a matrix declares for ``key`` as a plain list."""
    match matrix.get(key):
        case list() as declared:
            return list(declared)
        case _:
            return []


def _included_matrix_values(matrix: Mapping[str, Any], key: str) -> list[Any]:
    """Return the values the ``include`` legs add for ``key``."""
    return [leg[key] for leg in matrix.get("include") or [] if key in leg]


def _matrix_values(job: Mapping[str, Any], key: str) -> list[str]:
    """Return every value a job's matrix gives ``key``, ``include`` legs too."""
    matrix = (job.get("strategy") or {}).get("matrix") or {}
    values = [
        *_declared_matrix_values(matrix, key),
        *_included_matrix_values(matrix, key),
    ]
    return [str(value) for value in values]


def _mapping_labels(runs_on: Mapping[str, Any]) -> list[str]:
    """Return the runner labels of a mapping-form ``runs-on``, group included."""
    match runs_on.get("labels", []):
        case str() as label:
            raw = [label]
        case labels:
            raw = [str(label) for label in labels]
    return [*raw, str(runs_on.get("group", ""))]


def _raw_labels(job: Mapping[str, Any]) -> list[str]:
    """Return a job's runner labels before any matrix reference is resolved.

    Raises
    ------
    ContractGapError
        When ``runs-on`` has a form the contract cannot read.
    """
    match job.get("runs-on"):
        case str() as runs_on:
            return [runs_on]
        case list() as runs_on:
            return [str(label) for label in runs_on]
        case dict() as runs_on:
            return _mapping_labels(runs_on)
        case runs_on:
            message = f"runs-on has an unreadable form: {runs_on!r}"
            raise ContractGapError(message)


def _resolve_label(job: Mapping[str, Any], label: str) -> list[str]:
    """Resolve one runner label, expanding a matrix reference to its values.

    Raises
    ------
    ContractGapError
        When a ``matrix`` reference resolves to no value.
    """
    reference = _MATRIX_REF_RE.match(label.strip())
    if reference is None:
        return [label]
    values = _matrix_values(job, reference.group(1))
    if not values:
        message = f"matrix reference {label!r} resolves to no value"
        raise ContractGapError(message)
    return values


def _labels(job: Mapping[str, Any]) -> list[str]:
    """Return the runner labels of a job, in scalar, list or mapping form.

    Raises
    ------
    ContractGapError
        When a ``matrix`` reference cannot be resolved or the form is unknown.
    """
    return [
        value for label in _raw_labels(job) for value in _resolve_label(job, label)
    ]


def is_linux(job: Mapping[str, Any]) -> bool:
    """Return whether any runner label of the job names Ubuntu or Linux."""
    return any(
        "ubuntu" in label.lower() or "linux" in label.lower() for label in _labels(job)
    )


def _action_runs_suite(step: Mapping[str, Any]) -> bool:
    """Return whether a step uses an action that runs the suite."""
    uses = str(step.get("uses", ""))
    return any(marker in uses for marker in SUITE_ACTION_MARKERS)


def _command_calls_suite_goal(command: str, suite_goals: set[str]) -> bool:
    """Return whether a command calls a Make goal that reaches the suite."""
    return any(
        match.group(1) in suite_goals for match in _SUBMAKE_RE.finditer(command)
    )


def _command_runs_suite(step: Mapping[str, Any], suite_goals: set[str]) -> bool:
    """Return whether a step's command runs the suite directly or through Make."""
    command = str(step.get("run", ""))
    if SUITE_COMMAND_RE.search(command):
        return True
    return _command_calls_suite_goal(command, suite_goals)


def _runs_suite(step: Mapping[str, Any], suite_goals: set[str]) -> bool:
    """Return whether a step runs the suite."""
    return _action_runs_suite(step) or _command_runs_suite(step, suite_goals)


def _installs_tools(step: Mapping[str, Any]) -> bool:
    """Return whether a step runs the installer as a command."""
    return _INSTALL_RE.search(str(step.get("run", ""))) is not None


def _is_soft(step: Mapping[str, Any]) -> bool:
    """Return whether a step may be skipped or may fail without failing the job."""
    return "if" in step or step.get("continue-on-error", False) is not False


def _first_suite_step(steps: list[Any], suite_goals: set[str]) -> int | None:
    """Return the index of the first step that runs the suite, if any."""
    return next(
        (index for index, step in enumerate(steps) if _runs_suite(step, suite_goals)),
        None,
    )


def _installer_steps(steps: list[Any]) -> list[tuple[int, Mapping[str, Any]]]:
    """Return every step that runs the installer, with its index."""
    return [
        (index, step) for index, step in enumerate(steps) if _installs_tools(step)
    ]


def _soft_installer_problems(
    name: str, installers: list[tuple[int, Mapping[str, Any]]]
) -> list[str]:
    """Return the complaints about installer steps that may be skipped or may fail."""
    problems: list[str] = []
    for index, step in installers:
        if "if" in step:
            problems.append(f"{name}: step {index} runs {INSTALL_COMMAND} under an if:")
        if step.get("continue-on-error", False) is not False:
            problems.append(
                f"{name}: step {index} runs {INSTALL_COMMAND} with continue-on-error"
            )
    return problems


def _precedes_suite(
    installers: list[tuple[int, Mapping[str, Any]]], first_suite: int
) -> bool:
    """Return whether an unconditional installer step precedes the suite."""
    return any(
        index < first_suite and not _is_soft(step) for index, step in installers
    )


def job_problems(
    name: str, job: Mapping[str, Any], suite_goals: set[str]
) -> list[str]:
    """Return the complaints about one job that runs the suite on Linux.

    Parameters
    ----------
    name:
        ``file:job`` label for the complaint.
    job:
        The parsed job.
    suite_goals:
        Make goals that reach the suite.

    Returns
    -------
    list[str]
        Empty when the job installs the tools unconditionally, in a step of its
        own, before the first step that runs the suite.
    """
    steps = job.get("steps") or []
    first_suite = _first_suite_step(steps, suite_goals)
    if first_suite is None:
        return []
    installers = _installer_steps(steps)
    if not installers:
        return [f"{name}: runs the suite but never runs {INSTALL_COMMAND}"]
    problems = _soft_installer_problems(name, installers)
    if not _precedes_suite(installers, first_suite):
        problems.append(
            f"{name}: no unconditional {INSTALL_COMMAND} step precedes the suite "
            f"at step {first_suite}"
        )
    return problems


def _judges_suite(job: Mapping[str, Any], suite_goals: set[str]) -> bool:
    """Return whether a job is a Linux job that runs the suite."""
    if "steps" not in job or not is_linux(job):
        return False
    return any(_runs_suite(step, suite_goals) for step in job["steps"])


def workflow_problems(
    file: str, workflow: Mapping[str, Any], suite_goals: set[str]
) -> tuple[list[str], int]:
    """Return the complaints about one workflow and its suite-job count.

    Parameters
    ----------
    file:
        The workflow file name, for complaints.
    workflow:
        The parsed workflow.
    suite_goals:
        Make goals that reach the suite.

    Returns
    -------
    tuple[list[str], int]
        The complaints, and how many Linux jobs that run the suite were judged.
    """
    problems: list[str] = []
    judged = 0
    for job_id, job in (workflow.get("jobs") or {}).items():
        if not _judges_suite(job, suite_goals):
            continue
        judged += 1
        problems.extend(job_problems(f"{file}:{job_id}", job, suite_goals))
    return problems, judged