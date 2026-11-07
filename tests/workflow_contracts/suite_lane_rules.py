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
from typing import Any, Mapping

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
            for name in current:
                targets[name]["recipe"].append(line.strip())
            continue
        match = _TARGET_RE.match(line)
        if match is None or line.startswith((".PHONY", "#")):
            current = []
            continue
        current = match.group(1).split()
        prerequisites = [word for word in match.group(2).split() if word != "|"]
        for name in current:
            entry = targets.setdefault(name, {"prerequisites": [], "recipe": []})
            entry["prerequisites"].extend(prerequisites)
    return targets


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
    reaching = {
        name
        for name, entry in targets.items()
        if any(SUITE_COMMAND_RE.search(line) for line in entry["recipe"])
    }
    if not reaching:
        message = "the Makefile has no goal that runs cargo nextest"
        raise ContractGapError(message)
    changed = True
    while changed:
        changed = False
        for name, entry in targets.items():
            if name in reaching:
                continue
            called = [
                m.group(1)
                for line in entry["recipe"]
                for m in _SUBMAKE_RE.finditer(line)
            ]
            if any(goal in reaching for goal in [*entry["prerequisites"], *called]):
                reaching.add(name)
                changed = True
    return reaching


def _matrix_values(job: Mapping[str, Any], key: str) -> list[str]:
    """Return every value a job's matrix gives ``key``, ``include`` legs too."""
    matrix = (job.get("strategy") or {}).get("matrix") or {}
    values: list[Any] = []
    declared = matrix.get(key)
    if isinstance(declared, list):
        values.extend(declared)
    for leg in matrix.get("include") or []:
        if key in leg:
            values.append(leg[key])
    return [str(value) for value in values]


def _labels(job: Mapping[str, Any]) -> list[str]:
    """Return the runner labels of a job, in scalar, list or mapping form.

    Raises
    ------
    ContractGapError
        When a ``matrix`` reference cannot be resolved or the form is unknown.
    """
    runs_on = job.get("runs-on")
    if isinstance(runs_on, str):
        raw = [runs_on]
    elif isinstance(runs_on, list):
        raw = [str(label) for label in runs_on]
    elif isinstance(runs_on, dict):
        labels = runs_on.get("labels", [])
        raw = [labels] if isinstance(labels, str) else [str(x) for x in labels]
        raw.append(str(runs_on.get("group", "")))
    else:
        message = f"runs-on has an unreadable form: {runs_on!r}"
        raise ContractGapError(message)
    resolved: list[str] = []
    for label in raw:
        reference = _MATRIX_REF_RE.match(label.strip())
        if reference is None:
            resolved.append(label)
            continue
        values = _matrix_values(job, reference.group(1))
        if not values:
            message = f"matrix reference {label!r} resolves to no value"
            raise ContractGapError(message)
        resolved.extend(values)
    return resolved


def is_linux(job: Mapping[str, Any]) -> bool:
    """Return whether any runner label of the job names Ubuntu or Linux."""
    return any(
        "ubuntu" in label.lower() or "linux" in label.lower() for label in _labels(job)
    )


def _runs_suite(step: Mapping[str, Any], suite_goals: set[str]) -> bool:
    """Return whether a step runs the suite."""
    uses = str(step.get("uses", ""))
    if any(marker in uses for marker in SUITE_ACTION_MARKERS):
        return True
    command = str(step.get("run", ""))
    if SUITE_COMMAND_RE.search(command):
        return True
    return any(
        goal in suite_goals
        for match in _SUBMAKE_RE.finditer(command)
        for goal in [match.group(1)]
    )


def _installs_tools(step: Mapping[str, Any]) -> bool:
    """Return whether a step runs the installer as a command."""
    return _INSTALL_RE.search(str(step.get("run", ""))) is not None


def _is_soft(step: Mapping[str, Any]) -> bool:
    """Return whether a step may be skipped or may fail without failing the job."""
    return "if" in step or step.get("continue-on-error", False) is not False


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
    first_suite = next(
        (i for i, step in enumerate(steps) if _runs_suite(step, suite_goals)), None
    )
    if first_suite is None:
        return []
    installers = [(i, step) for i, step in enumerate(steps) if _installs_tools(step)]
    if not installers:
        return [f"{name}: runs the suite but never runs {INSTALL_COMMAND}"]
    problems: list[str] = []
    sound = [(i, step) for i, step in installers if not _is_soft(step)]
    for i, step in installers:
        if "if" in step:
            problems.append(f"{name}: step {i} runs {INSTALL_COMMAND} under an if:")
        if step.get("continue-on-error", False) is not False:
            problems.append(
                f"{name}: step {i} runs {INSTALL_COMMAND} with continue-on-error"
            )
    if not any(i < first_suite for i, _ in sound):
        problems.append(
            f"{name}: no unconditional {INSTALL_COMMAND} step precedes the suite "
            f"at step {first_suite}"
        )
    return problems


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
        if "steps" not in job or not is_linux(job):
            continue
        steps = job["steps"]
        if not any(_runs_suite(step, suite_goals) for step in steps):
            continue
        judged += 1
        problems.extend(job_problems(f"{file}:{job_id}", job, suite_goals))
    return problems, judged
