"""Contract: the release workflow resolves its package identity from configuration.

The Linux package maintainer and the macOS package identifier were literals in
``.github/workflows/release.yml``. Both now resolve once, in the ``metadata``
job, from an optional ``workflow_call`` input, then a repository variable, then
a default. The packaging steps read the metadata outputs, so a literal left
behind in either place, a missing output, a missing input or an empty
identifier fails ``make test-workflow-contracts``. The resolver's own script is
run as well, because the wiring checks would not notice an empty body.

Run via ``make test-workflow-contracts``.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest
import yaml

import release_identity_rules as rules

#: The literal values that used to sit in the packaging steps.
OLD_MAINTAINER = rules.MAINTAINER_DEFAULT
OLD_IDENTIFIER = "net.df12.mriya"


#: The optional inputs the synthetic workflow declares, and their specs.
SYNTHETIC_INPUTS = {
    "maintainer": {"required": False, "type": "string", "default": ""},
    "macos-identifier": {"required": False, "type": "string", "default": ""},
}

#: The metadata outputs the synthetic workflow declares, and their values.
SYNTHETIC_OUTPUTS = {
    "maintainer": rules.MAINTAINER_OUTPUT,
    "macos_identifier": rules.IDENTIFIER_OUTPUT,
}

#: The packaging inputs the synthetic workflow passes, and their values.
SYNTHETIC_PACKAGING = {
    "maintainer": rules.MAINTAINER_EXPRESSION,
    "identifier": rules.IDENTIFIER_EXPRESSION,
}


def _declared(names: dict[str, Any], drop: str | None) -> dict[str, Any]:
    """Return the declarations without ``drop``, when one is named."""
    return {name: value for name, value in names.items() if name != drop}


def _synthetic(options: dict[str, Any] | None = None) -> dict[str, Any]:
    """Return a workflow wiring the identity the way the contract requires.

    ``options`` names the deviations a case needs, so a case states only what
    it withholds or replaces:

    - ``drop_input``, ``drop_output``: the ``workflow_call`` input or metadata
      output to leave out, named by its YAML key.
    - ``maintainer_candidate``, ``identifier_candidate``: replacements for the
      resolver's ``env`` values.
    - ``maintainer``, ``identifier``: replacements for the packaging inputs.
    """
    chosen = options or {}
    packaging = {
        "maintainer": chosen.get("maintainer", SYNTHETIC_PACKAGING["maintainer"]),
        "identifier": chosen.get("identifier", SYNTHETIC_PACKAGING["identifier"]),
    }
    return {
        "on": {
            "workflow_call": {
                "inputs": _declared(SYNTHETIC_INPUTS, chosen.get("drop_input"))
            }
        },
        "jobs": {
            "metadata": {
                "outputs": _declared(SYNTHETIC_OUTPUTS, chosen.get("drop_output")),
                "steps": [
                    {
                        "id": rules.IDENTITY_STEP,
                        "env": {
                            "MAINTAINER_CANDIDATE": chosen.get(
                                "maintainer_candidate", rules.MAINTAINER_CANDIDATE
                            ),
                            "MACOS_IDENTIFIER_CANDIDATE": chosen.get(
                                "identifier_candidate", rules.IDENTIFIER_CANDIDATE
                            ),
                        },
                        "run": "true",
                    }
                ],
            },
            "packaging": {
                "steps": [
                    {
                        "uses": f"org/shared/.github/actions/{action}abc",
                        "with": {field: packaging[field]},
                    }
                    for action, field in (
                        (rules.LINUX_PACKAGES_ACTION, "maintainer"),
                        (rules.MACOS_PACKAGE_ACTION, "identifier"),
                    )
                ]
            },
        },
    }


@pytest.mark.parametrize(
    ("overrides", "fragment"),
    [
        ({}, ""),
        ({"maintainer": OLD_MAINTAINER}, "literal maintainer"),
        ({"identifier": OLD_IDENTIFIER}, "literal identifier"),
        ({"drop_output": "maintainer"}, "does not declare"),
        ({"identifier": ""}, "empty identifier"),
        ({"drop_input": "maintainer"}, "input 'maintainer' must"),
        ({"drop_input": "macos-identifier"}, "input 'macos-identifier' must"),
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
            {"maintainer_candidate": rules.MAINTAINER_CANDIDATE.removesuffix("}")},
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
            {"identifier_candidate": rules.IDENTIFIER_CANDIDATE.removesuffix("}")},
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
    problems, judged = rules.identity_problems(_synthetic(overrides), "w.yml")
    assert judged == 2
    if fragment:
        assert any(fragment in problem for problem in problems), problems
    else:
        assert not problems, problems


def test_the_repository_release_identity_is_configured() -> None:
    """The real release workflow reads both values from metadata, and both are judged."""
    workflow = yaml.safe_load(rules.RELEASE.read_text(encoding="utf-8"))
    problems, judged = rules.identity_problems(workflow, rules.RELEASE.name)
    linux_steps = list(rules.packaging_steps(workflow, rules.LINUX_PACKAGES_ACTION))
    macos_steps = list(rules.packaging_steps(workflow, rules.MACOS_PACKAGE_ACTION))
    assert linux_steps, "no linux-packages step found, so the maintainer contract proves nothing"
    assert macos_steps, "no macos-package step found, so the identifier contract proves nothing"
    assert judged == len(linux_steps) + len(macos_steps)
    assert not problems, problems


@pytest.mark.parametrize(
    ("maintainer", "identifier", "expected"),
    [
        (
            rules.MAINTAINER_DEFAULT,
            "net.df12.mriya",
            f"maintainer={rules.MAINTAINER_DEFAULT}\nmacos_identifier=net.df12.mriya\n",
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
    result = rules.run_resolver(maintainer, identifier, output)
    assert result.returncode == 0, result.stdout + result.stderr
    assert output.read_text(encoding="utf-8") == expected


@pytest.mark.parametrize(
    ("maintainer", "identifier"),
    [
        ("   ", "net.df12.mriya"),
        (rules.MAINTAINER_DEFAULT, "   "),
        ("Ada <a@b.test>\nEvil <e@b.test>", "net.df12.mriya"),
        (rules.MAINTAINER_DEFAULT, "net.df12.mriya\nnet.df12.other"),
        (rules.MAINTAINER_DEFAULT, "net.df12.mriya extra"),
        (rules.MAINTAINER_DEFAULT, "net.df12.mriya\textra"),
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
    result = rules.run_resolver(maintainer, identifier, output)
    assert result.returncode != 0, result.stdout + result.stderr
    assert output.read_text(encoding="utf-8") == ""
    assert "::error::" in result.stdout