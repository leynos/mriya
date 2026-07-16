"""Test the exact phrase-policy helper and its command boundary.

The suite imports ``typos_rollout_check.py`` and validates policy loading,
tracked-file discovery, masking, exclusions, diagnostics, and Cyclopts command
compatibility. Run it from the repository root with
``make spelling-helper-test``.
"""

from __future__ import annotations

import importlib
from pathlib import Path
import re
import types
import typing as typ

from plumbum.commands.processes import ProcessExecutionError
import pytest

if typ.TYPE_CHECKING:
    from cmd_mox import CmdMox

SCRIPTS = Path(__file__).resolve().parents[1]
REPOSITORY_ROOT = SCRIPTS.parent
PROHIBITED = "hand" + "-written"
TITLE_PROHIBITED = "Hand" + "-written"


@pytest.fixture
def checker(monkeypatch: pytest.MonkeyPatch) -> types.ModuleType:
    """Import the standalone checker through its runtime module path."""
    monkeypatch.syspath_prepend(str(SCRIPTS))
    importlib.invalidate_caches()
    return importlib.import_module("typos_rollout_check")


def write_files(path: Path, files: dict[str, str]) -> None:
    """Write a small repository fixture without invoking host commands."""
    for relative, content in files.items():
        target = path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")


def expect_tracked(cmd_mox: CmdMox, path: Path, *files: str) -> None:
    """Expect deterministic tracked-file enumeration through Plumbum."""
    cmd_mox.mock("git").with_args("-C", str(path), "ls-files", "-z").returns(
        exit_code=0, stdout="\0".join(files) + "\0"
    )


def policy_files(*, local_phrase: str = "") -> dict[str, str]:
    """Return minimal generated, shared, and local policy documents."""
    return {
        "typos.toml": (
            f"# Policy for {PROHIBITED} corrections.\n"
            '[files]\nextend-exclude = ["*.md", "!README.md"]\n\n'
            '[default]\nextend-ignore-re = ["`[^`\\\\n]+`"]\n'
        ),
        ".typos-oxendict-base.toml": (
            f'[phrases.corrections]\n"{PROHIBITED}" = "handwritten"\n'
        ),
        "typos.local.toml": local_phrase,
    }


def make_variable(name: str) -> str:
    """Return one simple Makefile variable used by the helper environment."""
    makefile = (REPOSITORY_ROOT / "Makefile").read_text(encoding="utf-8")
    match = re.search(rf"^{re.escape(name)} := (.+)$", makefile, re.MULTILINE)
    if match is None:
        message = f"Makefile does not define {name}"
        raise ValueError(message)
    return match.group(1)


class TestPhrasePolicyChecker:
    """Exercise policy, scanning, and command boundaries."""

    def test_cyclopts_metadata_matches_makefile_pin(self) -> None:
        """Keep the PEP 723 dependency aligned with the test environment."""
        helper = (SCRIPTS / "typos_rollout_check.py").read_text(encoding="utf-8")
        version = make_variable("CYCLOPTS_VERSION")

        assert f'"cyclopts=={version}"' in helper, (
            "Cyclopts PEP 723 metadata drifted from the Makefile pin"
        )

    def test_load_policy_combines_shared_and_local_phrases(
        self, checker: types.ModuleType, tmp_path: Path
    ) -> None:
        """Combine shared phrases with generated scan settings."""
        files = policy_files(
            local_phrase=('[phrases.corrections]\n"fit-for-purpose" = "suitable"\n')
        )
        write_files(tmp_path, files)

        policy = checker.load_policy(tmp_path)

        assert policy.phrase_corrections == (
            ("fit-for-purpose", "suitable"),
            (PROHIBITED, "handwritten"),
        ), "shared and local corrections were not combined"
        assert policy.ignore_patterns == (r"`[^`\n]+`",)
        assert policy.excluded_files == ("*.md", "!README.md")

        (tmp_path / ".typos-oxendict-base.toml").unlink()
        with pytest.raises(FileNotFoundError, match=r"docs/developers-guide\.md"):
            checker.load_policy(tmp_path)

    def test_checker_preserves_boundaries_masking_and_exclusions(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
    ) -> None:
        """Report phrases only when boundaries and policy allow them."""
        write_files(
            tmp_path,
            {
                "README.md": (
                    f"{PROHIBITED}\n{TITLE_PROHIBITED} prose\n"
                    + "pre-hand"
                    + "-written\n"
                    + f"`{PROHIBITED}`\n"
                ),
                "skip.md": f"{PROHIBITED}\n",
                **policy_files(),
            },
        )
        expect_tracked(cmd_mox, tmp_path, "README.md", "skip.md")

        findings = checker.check_phrase_corrections(
            tmp_path, checker.load_policy(tmp_path)
        )

        assert [(item.line, item.phrase) for item in findings] == [
            (1, PROHIBITED),
            (2, TITLE_PROHIBITED),
        ], "phrase boundaries, masking, or exclusions changed"

    @pytest.mark.parametrize(
        "patterns",
        [(r"`", r"`[^`\n]+`"), (r"`[^`\n]+`", r"`")],
    )
    def test_masking_unions_overlapping_patterns(
        self, checker: types.ModuleType, patterns: tuple[str, ...]
    ) -> None:
        """Mask overlapping spans independently of policy order."""
        text = f"before `{PROHIBITED}`\nafter"

        expected = "before " + " " * 14 + "\nafter"
        assert checker._masked(text, patterns) == expected

    def test_cyclopts_command_reports_location_and_exit_two(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
        capsys: pytest.CaptureFixture[str],
    ) -> None:
        """Preserve the repository flag, diagnostic, and failure status."""
        write_files(
            tmp_path,
            {"README.md": f"Prefer {PROHIBITED}.\n", **policy_files()},
        )
        expect_tracked(cmd_mox, tmp_path, "README.md")

        with pytest.raises(SystemExit) as exit_status:
            checker.app(["--repository", str(tmp_path)], exit_on_error=False)

        assert exit_status.value.code == 2, "the command accepted a prohibited phrase"
        assert capsys.readouterr().out == (
            f"README.md:1:8: {PROHIBITED} -> handwritten\n"
        ), "the diagnostic omitted its source location or correction"

    def test_cyclopts_command_accepts_clean_repository(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
        capsys: pytest.CaptureFixture[str],
    ) -> None:
        """Return success without output when tracked prose is clean."""
        write_files(
            tmp_path,
            {"README.md": "Already handwritten.\n", **policy_files()},
        )
        expect_tracked(cmd_mox, tmp_path, "README.md")

        with pytest.raises(SystemExit) as exit_status:
            checker.app(["--repository", str(tmp_path)], exit_on_error=False)

        assert exit_status.value.code == 0, "the command rejected clean prose"
        assert capsys.readouterr().out == "", (
            "the command emitted a clean-run diagnostic"
        )

    def test_non_utf8_tracked_text_is_rejected(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
    ) -> None:
        """Fail closed when eligible tracked text is not UTF-8."""
        write_files(tmp_path, policy_files())
        (tmp_path / "binary.dat").write_bytes(b"\xff\xfe")
        expect_tracked(cmd_mox, tmp_path, "binary.dat")

        with pytest.raises(UnicodeDecodeError):
            checker.check_phrase_corrections(tmp_path, checker.load_policy(tmp_path))

    def test_indexed_path_absent_during_enumeration_is_skipped(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
    ) -> None:
        """Ignore an intentionally deleted path that remains in the index."""
        write_files(tmp_path, policy_files())
        expect_tracked(cmd_mox, tmp_path, "missing.txt")

        findings = checker.check_phrase_corrections(
            tmp_path, checker.load_policy(tmp_path)
        )

        assert findings == (), "an absent indexed path produced findings"

    def test_path_removed_after_enumeration_is_rejected(
        self,
        checker: types.ModuleType,
        monkeypatch: pytest.MonkeyPatch,
        tmp_path: Path,
    ) -> None:
        """Fail closed when a tracked path disappears before it is read."""
        write_files(tmp_path, policy_files())

        def missing_after_enumeration(_: Path) -> tuple[Path, ...]:
            """Return a path that disappeared after tracked-file enumeration."""
            return (Path("missing.txt"),)

        monkeypatch.setattr(checker, "_tracked", missing_after_enumeration)

        with pytest.raises(FileNotFoundError):
            checker.check_phrase_corrections(tmp_path, checker.load_policy(tmp_path))

    def test_git_failure_is_not_hidden(
        self,
        checker: types.ModuleType,
        cmd_mox: CmdMox,
        tmp_path: Path,
    ) -> None:
        """Surface tracked-file discovery failures from Plumbum."""
        write_files(tmp_path, policy_files())
        cmd_mox.mock("git").with_args("-C", str(tmp_path), "ls-files", "-z").returns(
            exit_code=128, stderr="not a repository"
        )

        with pytest.raises(ProcessExecutionError, match="not a repository"):
            checker.check_phrase_corrections(tmp_path, checker.load_policy(tmp_path))
