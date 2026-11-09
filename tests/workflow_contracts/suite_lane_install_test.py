"""Contract: every Linux job that runs the suite installs the build tools first.

The rules live in ``suite_lane_rules``. These tests drive them against synthetic
workflows in both directions, so a rule that stopped detecting anything fails
here, and then against the repository's own workflows, so a real edit that drops
the installer, moves it after the suite, guards it with ``if:`` or makes it
``continue-on-error`` fails ``make test-workflow-contracts``.
"""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml

import suite_lane_rules as rules

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"

MAKEFILE = """\
all: check-fmt lint test
test: check-build-tools
\tcargo nextest run --all-targets
check-build-tools:
\t@scripts/check-build-tools.sh
install-build-tools:
\t@scripts/install-build-tools.sh
lint: check-build-tools
\tcargo clippy
build-all: all
"""


def _workflow(runs_on: str, steps: str, extra: str = "") -> dict:
    """Return a one-job workflow with the given ``runs-on`` and step list."""
    text = f"jobs:\n  suite:\n    runs-on: {runs_on}\n{extra}    steps:\n{steps}"
    return yaml.safe_load(text)


INSTALL = "      - run: make install-build-tools\n"
SUITE = "      - run: make test\n"


def _problems(workflow: dict) -> tuple[list[str], int]:
    """Judge a synthetic workflow against the synthetic Makefile."""
    goals = rules.goals_reaching_suite(MAKEFILE)
    return rules.workflow_problems("w.yml", workflow, goals)


def test_the_goals_that_reach_the_suite_include_indirect_dependants() -> None:
    """``all`` and ``build-all`` reach the suite only through prerequisites."""
    assert rules.goals_reaching_suite(MAKEFILE) == {"test", "all", "build-all"}, (
        "indirect dependants of the suite goal were not derived"
    )


def test_a_makefile_without_a_suite_goal_is_a_gap() -> None:
    """A Makefile that never runs nextest is refused rather than passed."""
    with pytest.raises(rules.ContractGapError):
        rules.goals_reaching_suite("lint:\n\tcargo clippy\n")


def test_install_before_the_suite_is_sound() -> None:
    """The baseline: an unconditional install precedes the suite."""
    assert _problems(_workflow("ubuntu-latest", INSTALL + SUITE)) == ([], 1), (
        "an unconditional install before the suite must pass"
    )


@pytest.mark.parametrize(
    ("label", "steps", "fragment"),
    [
        ("dropped", SUITE, "never runs make install-build-tools"),
        ("after_the_suite", SUITE + INSTALL, "no unconditional"),
        (
            "guarded_by_if",
            "      - run: make install-build-tools\n        if: always()\n" + SUITE,
            "under an if:",
        ),
        (
            "continue_on_error",
            "      - run: make install-build-tools\n        continue-on-error: true\n"
            + SUITE,
            "continue-on-error",
        ),
        (
            "named_but_not_a_command",
            "      - run: echo make install-build-tools-not\n" + SUITE,
            "never runs make install-build-tools",
        ),
    ],
)
def test_a_broken_install_is_reported(label: str, steps: str, fragment: str) -> None:
    """Each way of weakening the installer is a complaint, not a pass."""
    problems, judged = _problems(_workflow("ubuntu-latest", steps))
    assert judged == 1, label
    assert any(fragment in problem for problem in problems), (label, problems)


@pytest.mark.parametrize(
    "suite_step",
    [
        "      - run: make test\n",
        "      - run: make all\n",
        "      - run: make build-all\n",
        "      - run: cargo nextest run --all-targets\n",
        "      - run: cargo-nextest run\n",
        "      - uses: leynos/shared-actions/.github/actions/generate-coverage@abc\n",
    ],
    ids=["make_test", "make_all", "make_indirect", "nextest", "cargo_nextest", "coverage"],
)
def test_every_way_of_reaching_the_suite_is_derived(suite_step: str) -> None:
    """A new Linux job reaching the suite is held to the rule without a listing."""
    problems, judged = _problems(_workflow("ubuntu-latest", suite_step))
    assert judged == 1, "the job was not recognized as a Linux suite job"
    assert problems, "a suite job without an installer was not reported"


@pytest.mark.parametrize(
    "runs_on",
    [
        "ubuntu-latest",
        "[self-hosted, ubuntu-24.04]",
        "{group: pool, labels: [linux-x64]}",
        "{group: pool, labels: ubuntu-latest}",
    ],
    ids=["scalar", "list", "mapping_list", "mapping_scalar"],
)
def test_every_runs_on_form_is_recognized_as_linux(runs_on: str) -> None:
    """Scalar, list and mapping runner forms all count as Linux."""
    assert _problems(_workflow(runs_on, SUITE))[1] == 1, (
        f"{runs_on!r} was not recognized as a Linux runner"
    )


def test_a_matrix_reference_is_resolved_across_include_legs() -> None:
    """A Linux leg of an ``include`` matrix makes the job a Linux job."""
    extra = (
        "    strategy:\n      matrix:\n        include:\n"
        "          - runner: windows-latest\n          - runner: ubuntu-latest\n"
    )
    workflow = _workflow("${{ matrix.runner }}", SUITE, extra)
    assert _problems(workflow)[1] == 1, (
        "the Linux leg of an include matrix was not judged"
    )


def test_an_unresolvable_matrix_reference_is_a_gap() -> None:
    """A reference the matrix does not define is reported, not skipped."""
    with pytest.raises(rules.ContractGapError):
        _problems(_workflow("${{ matrix.runner }}", SUITE))


@pytest.mark.parametrize(
    ("runs_on", "steps"),
    [
        ("windows-latest", SUITE),
        ("macos-15", SUITE),
        ("ubuntu-latest", "      - run: make lint\n"),
    ],
    ids=["windows", "macos", "linux_without_the_suite"],
)
def test_narrow_cases_are_not_judged(runs_on: str, steps: str) -> None:
    """Non-Linux suite jobs and Linux jobs that never run the suite pass."""
    assert _problems(_workflow(runs_on, steps)) == ([], 0), (
        f"{runs_on!r} should not be judged for a suite it does not run"
    )


def test_the_repository_workflows_hold_the_rule() -> None:
    """Every real Linux suite job installs the tools unconditionally and first."""
    goals = rules.goals_reaching_suite((ROOT / "Makefile").read_text(encoding="utf-8"))
    problems: list[str] = []
    judged = 0
    for path in sorted(WORKFLOWS.glob("*.yml")):
        workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
        found, count = rules.workflow_problems(path.name, workflow, goals)
        problems.extend(found)
        judged += count
    assert judged > 0, "no Linux job runs the suite, so the contract proves nothing"
    assert not problems, problems
