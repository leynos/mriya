"""Test the exact phrase-policy helper and its command boundary.

The suite imports ``typos_rollout_check.py`` and validates policy loading,
tracked-file discovery, masking, exclusions, diagnostics, and Cyclopts command
compatibility. Run it from the repository root with
``make spelling-helper-test``.
"""

from __future__ import annotations

from dataclasses import dataclass
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


@dataclass(frozen=True, slots=True, kw_only=True)
class CommandCase:
    """Describe one spelling CLI input and its expected process result."""

    content: str
    expected_code: int
    expected_output: str


@dataclass(frozen=True, slots=True, kw_only=True)
class CommandContext:
    """Bundle the checker module with its command-mocking boundary."""

    checker: types.ModuleType
    cmd_mox: CmdMox


@pytest.fixture
def checker(monkeypatch: pytest.MonkeyPatch) -> types.ModuleType:
    """Import the standalone checker through its runtime module path."""
    monkeypatch.syspath_prepend(str(SCRIPTS))
    importlib.invalidate_caches()
    return importlib.import_module("typos_rollout_check")


@pytest.fixture
def command_context(checker: types.ModuleType, cmd_mox: CmdMox) -> CommandContext:
    """Provide the focused dependencies used by spelling CLI cases."""
    return CommandContext(checker=checker, cmd_mox=cmd_mox)


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

    @pytest.mark.parametrize(
        ("make_variable_name", "dependency"),
        [("CYCLOPTS_VERSION", "cyclopts"), ("PLUMBUM_VERSION", "plumbum")],
    )
    def test_metadata_matches_makefile_pin(
        self, make_variable_name: str, dependency: str
    ) -> None:
        """Keep PEP 723 dependencies aligned with the test environment."""
        helper = (SCRIPTS / "typos_rollout_check.py").read_text(encoding="utf-8")
        version = make_variable(make_variable_name)

        assert f'"{dependency}=={version}"' in helper, (
            f"{dependency} PEP 723 metadata drifted from the Makefile pin"
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

    @pytest.mark.parametrize(
        ("invalid_policy", "message"),
        [
            pytest.param(
                (".typos-oxendict-base.toml", "phrases = []\n"),
                "'phrases' must be a table",
                id="shared-phrases-table",
            ),
            pytest.param(
                ("typos.local.toml", "[phrases]\ncorrections = []\n"),
                "'corrections' must be a table",
                id="local-corrections-table",
            ),
            pytest.param(
                (
                    "typos.toml",
                    "[files]\nextend-exclude = [1]\n[default]\nextend-ignore-re = []\n",
                ),
                "'extend-exclude' must be a list of strings",
                id="generated-exclusions-list",
            ),
            pytest.param(
                (
                    ".typos-oxendict-base.toml",
                    '[phrases.corrections]\n"bad" = 1\n',
                ),
                "phrase corrections must map strings to strings",
                id="phrase-correction-values",
            ),
        ],
    )
    def test_load_policy_rejects_malformed_shapes(
        self,
        checker: types.ModuleType,
        tmp_path: Path,
        invalid_policy: tuple[str, str],
        message: str,
    ) -> None:
        """Reject malformed shared, local, and generated policy values."""
        relative, malformed = invalid_policy
        files = policy_files()
        files[relative] = malformed
        write_files(tmp_path, files)

        with pytest.raises(TypeError, match=message):
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

    @pytest.mark.parametrize(
        "case",
        [
            pytest.param(
                CommandCase(
                    content=f"Prefer {PROHIBITED}.\n",
                    expected_code=2,
                    expected_output=(f"README.md:1:8: {PROHIBITED} -> handwritten\n"),
                ),
                id="prohibited-phrase",
            ),
            pytest.param(
                CommandCase(
                    content="Already handwritten.\n",
                    expected_code=0,
                    expected_output="",
                ),
                id="clean-prose",
            ),
        ],
    )
    def test_cyclopts_command_reports_status_and_diagnostic(
        self,
        command_context: CommandContext,
        tmp_path: Path,
        capsys: pytest.CaptureFixture[str],
        case: CommandCase,
    ) -> None:
        """Preserve the repository flag, gate status, and diagnostic."""
        write_files(
            tmp_path,
            {"README.md": case.content, **policy_files()},
        )
        expect_tracked(command_context.cmd_mox, tmp_path, "README.md")

        with pytest.raises(SystemExit) as exit_status:
            command_context.checker.app(
                ["--repository", str(tmp_path)], exit_on_error=False
            )

        assert exit_status.value.code == case.expected_code, (
            "the command returned the wrong spelling-gate status"
        )
        assert capsys.readouterr().out == case.expected_output, (
            "the command emitted the wrong diagnostic"
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
