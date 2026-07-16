#!/usr/bin/env -S uv run python
# /// script
# requires-python = ">=3.13"
# dependencies = [
#   "cyclopts==4.21.1",
#   "pathspec==1.1.1",
#   "plumbum==2.0.1",
# ]
# ///
"""Enforce exact phrase corrections alongside the Typos scanner.

The checker loads shared and repository-local phrase policy, then scans
eligible tracked UTF-8 text. It complements Typos by enforcing exact phrase
corrections in human-facing source comments and tests.

Run it from the repository root with
``uv run scripts/typos_rollout_check.py --repository .``.
"""

from __future__ import annotations

from collections import abc as cabc
from dataclasses import dataclass
import os
from pathlib import Path
import re
import tomllib

from cyclopts import App
from pathspec import GitIgnoreSpec
from plumbum import local

app = App()

POLICY_PATHS = frozenset(
    {
        Path(".typos-oxendict-base.toml"),
        Path("typos.local.toml"),
        Path("typos.toml"),
    }
)


@dataclass(frozen=True)
class PhraseFinding:
    """Describe one prohibited phrase.

    Attributes
    ----------
    path
        Repository-relative path containing the phrase.
    line
        One-based source line number.
    column
        One-based source column number.
    phrase
        Source phrase preserving its original case.
    correction
        Replacement prescribed by the spelling policy.
    """

    path: Path
    line: int
    column: int
    phrase: str
    correction: str


@dataclass(frozen=True)
class PhrasePolicy:
    """Hold the effective policy needed by the phrase scanner.

    Attributes
    ----------
    phrase_corrections
        Ordered prohibited phrase and replacement pairs.
    ignore_patterns
        Regular expressions that mask ignored spans.
    excluded_files
        Gitignore-style patterns for excluded paths.
    """

    phrase_corrections: tuple[tuple[str, str], ...]
    ignore_patterns: tuple[str, ...]
    excluded_files: tuple[str, ...]


def _document(path: Path) -> dict[str, object]:
    """Load one TOML policy document."""
    with path.open("rb") as stream:
        return tomllib.load(stream)


def _table(document: dict[str, object], name: str) -> dict[str, object]:
    """Return a TOML table, rejecting a present value of the wrong shape."""
    value = document.get(name, {})
    if not isinstance(value, dict):
        message = f"{name!r} must be a table"
        raise TypeError(message)
    return value


def _strings(table: dict[str, object], key: str) -> tuple[str, ...]:
    """Return a validated string list from effective Typos policy."""
    value = table.get(key, [])
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        message = f"{key!r} must be a list of strings"
        raise TypeError(message)
    return tuple(value)


def _phrases(document: dict[str, object]) -> dict[str, str]:
    """Return validated phrase corrections from one policy document."""
    corrections = _table(_table(document, "phrases"), "corrections")
    if not all(isinstance(correction, str) for correction in corrections.values()):
        message = "phrase corrections must map strings to strings"
        raise TypeError(message)
    return dict(corrections)


def load_policy(repository: Path) -> PhrasePolicy:
    """Load generated scan policy and shared phrase corrections.

    Parameters
    ----------
    repository
        Repository containing the generated, shared, and local policies.

    Returns
    -------
    PhrasePolicy
        Effective phrase corrections, ignored spans, and path exclusions.

    Raises
    ------
    FileNotFoundError
        A required generated or shared policy file is missing.
    OSError
        A policy file cannot be read.
    TypeError
        A policy document contains a value of the wrong shape.
    tomllib.TOMLDecodeError
        A policy file contains invalid TOML.
    """
    generated = _document(repository / "typos.toml")
    shared_cache = repository / ".typos-oxendict-base.toml"
    if not shared_cache.is_file():
        message = (
            f"{shared_cache} is missing; regenerate the spelling configuration "
            "as documented in docs/developers-guide.md"
        )
        raise FileNotFoundError(message)
    phrases = _phrases(_document(shared_cache))
    local_overlay = repository / "typos.local.toml"
    if local_overlay.exists():
        phrases.update(_phrases(_document(local_overlay)))
    return PhrasePolicy(
        phrase_corrections=tuple(sorted(phrases.items())),
        ignore_patterns=_strings(_table(generated, "default"), "extend-ignore-re"),
        excluded_files=_strings(_table(generated, "files"), "extend-exclude"),
    )


def _tracked(repository: Path) -> tuple[Path, ...]:
    """Return present tracked paths in deterministic order."""
    # Plumbum snapshots its environment, so resolve Git inside the active
    # process environment where command shims and their controls are visible.
    with local.env(**os.environ):
        raw = local["git"]["-C", str(repository), "ls-files", "-z"]()
    paths = (Path(item) for item in sorted(filter(None, raw.split("\0"))))
    present: list[Path] = []
    for relative in paths:
        try:
            (repository / relative).lstat()
        except FileNotFoundError:
            continue
        present.append(relative)
    return tuple(present)


def _masked(text: str, patterns: tuple[str, ...]) -> str:
    """Blank ignored spans while preserving line and column positions."""
    ignored = bytearray(len(text))
    for pattern in patterns:
        for match in re.finditer(pattern, text):
            ignored[match.start() : match.end()] = b"\x01" * (
                match.end() - match.start()
            )
    return "".join(
        "\n" if character == "\n" else " " if ignored[index] else character
        for index, character in enumerate(text)
    )


def _phrase_findings(
    relative: Path,
    text: str,
    masked: str,
    phrase_corrections: tuple[tuple[str, str], ...],
) -> cabc.Iterator[PhraseFinding]:
    """Yield exact phrase findings from one masked tracked file."""
    for phrase, correction in phrase_corrections:
        for match in re.finditer(
            rf"(?<![\w-]){re.escape(phrase)}(?![\w-])", masked, re.IGNORECASE
        ):
            previous = masked.rfind("\n", 0, match.start())
            yield PhraseFinding(
                relative,
                masked.count("\n", 0, match.start()) + 1,
                match.start() - previous,
                text[match.start() : match.end()],
                correction,
            )


def check_phrase_corrections(
    repository: Path, policy: PhrasePolicy
) -> tuple[PhraseFinding, ...]:
    """Find prohibited exact phrases in tracked UTF-8 text.

    Parameters
    ----------
    repository
        Repository whose tracked files should be scanned.
    policy
        Effective corrections, ignored spans, and path exclusions.

    Returns
    -------
    tuple[PhraseFinding, ...]
        Findings in deterministic path, phrase, and source order.

    Raises
    ------
    OSError
        An eligible tracked file cannot be read.
    UnicodeDecodeError
        An eligible tracked file is not UTF-8 text.
    plumbum.commands.processes.ProcessExecutionError
        Git cannot enumerate the repository's tracked files.
    """
    found = []
    exclusion_spec = GitIgnoreSpec.from_lines(policy.excluded_files)
    for relative in _tracked(repository):
        if relative in POLICY_PATHS or exclusion_spec.match_file(relative.as_posix()):
            continue
        text = (repository / relative).read_text(encoding="utf-8")
        masked = _masked(text, policy.ignore_patterns)
        found.extend(
            _phrase_findings(relative, text, masked, policy.phrase_corrections)
        )
    return tuple(found)


def run(repository: Path | None = None) -> int:
    """Report prohibited phrases and return the spelling-gate status."""
    repository = Path.cwd() if repository is None else repository
    findings = check_phrase_corrections(repository, load_policy(repository))
    for item in findings:
        print(
            f"{item.path}:{item.line}:{item.column}: {item.phrase} -> {item.correction}"
        )
    return 2 if findings else 0


@app.default
def check(*, repository: Path | None = None) -> int:
    """Run exact phrase enforcement for a repository.

    Parameters
    ----------
    repository
        Repository to check, defaulting to the current directory.

    Returns
    -------
    int
        Two when prohibited phrases are found, otherwise zero.
    """
    return run(repository)


def main() -> None:
    """Parse command-line arguments and run the phrase checker."""
    raise SystemExit(app())


if __name__ == "__main__":
    main()
