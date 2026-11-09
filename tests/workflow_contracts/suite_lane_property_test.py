"""Property: suite-goal reachability agrees with a graph-reachability oracle.

``goals_reaching_suite`` closes over prerequisites and ``make`` calls. The
oracle here is an independent least fixed point over the same edges, so a
closure that misses a cycle, a branch, or a goal that never reaches the suite
disagrees with it. Hypothesis supplies the shapes a hand-written table would
take too long to enumerate: chains, branches, cycles, and disconnected goals.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

from typing import Any

from hypothesis import given, settings
from hypothesis import strategies as st

import suite_lane_rules as rules

#: Goal names a generated Makefile may declare.
NAMES = ["suite", "alpha", "beta", "gamma", "delta"]

#: Every edge a goal may depend on another goal.
EDGES = st.sets(
    st.sampled_from([(owner, dep) for owner in NAMES for dep in NAMES if owner != dep])
)


@st.composite
def makefiles(draw: Any) -> tuple[str, set[tuple[str, str]], str]:
    """Draw a Makefile, the edges it declares, and the goal that runs the suite."""
    suite_goal = draw(st.sampled_from(NAMES))
    edges = draw(EDGES)
    # A recipe may call a goal instead of naming it as a prerequisite; both
    # kinds of edge must reach the suite goal.
    calls = draw(st.sets(st.sampled_from(sorted(edges)))) if edges else set()
    lines: list[str] = []
    for name in NAMES:
        prerequisites = sorted(
            dep for (owner, dep) in edges - calls if owner == name
        )
        lines.append(f"{name}: {' '.join(prerequisites)}")
        if name == suite_goal:
            lines.append("\tcargo nextest run")
        lines.extend(
            f"\t$(MAKE) {dep}" for (owner, dep) in sorted(calls) if owner == name
        )
    return "\n".join(lines) + "\n", edges, suite_goal


def reachable(edges: set[tuple[str, str]], suite_goal: str) -> set[str]:
    """Return the goals that depend on or call the suite goal, directly or not."""
    reaching = {suite_goal}
    while True:
        extended = reaching | {
            owner for (owner, dep) in edges if dep in reaching
        }
        if extended == reaching:
            return reaching
        reaching = extended


@given(makefiles())
@settings(deadline=None)
def test_goals_reaching_suite_matches_graph_reachability(
    case: tuple[str, set[tuple[str, str]], str],
) -> None:
    """The derivation equals an independent least fixed point over the edges."""
    text, edges, suite_goal = case
    expected = reachable(edges, suite_goal)
    assert rules.goals_reaching_suite(text) == expected, text