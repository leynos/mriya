//! The static shapes behind the lint gate: that `lint` lists Clippy before Whitaker and is built
//! serially, that each half's recipe is a plain command that cannot be soft-failed, and that CI
//! runs exactly `make lint` in a step that cannot mask a failure. The Makefile and workflow are
//! read at compile time, so a change to either is judged by the same run as the tests it affects.

use rstest::rstest;

const MAKEFILE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Makefile"));
const CI_WORKFLOW: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/.github/workflows/ci.yml"
));

/// A Make rule whose recipe the contract reads.
#[derive(Clone, Copy)]
enum Rule {
    /// The Clippy half of the lint gate.
    LintClippy,
    /// The Whitaker half of the lint gate.
    LintWhitaker,
}

impl Rule {
    /// The rule header, including the colon, as it appears in the Makefile.
    fn header(self) -> &'static str {
        match self {
            Self::LintClippy => "lint-clippy:",
            Self::LintWhitaker => "lint-whitaker:",
        }
    }
}

/// One rule's recipe: its tab-stripped command lines, in order.
#[derive(Debug)]
struct Recipe(Vec<&'static str>);

impl Recipe {
    /// Reads the recipe of `rule` from the Makefile, or `None` when the rule is absent.
    fn of(rule: Rule) -> Option<Self> {
        let header = rule.header();
        let mut lines = MAKEFILE
            .lines()
            .skip_while(|line| !line.starts_with(header))
            .skip(1);
        let recipe: Vec<&'static str> = lines
            .by_ref()
            .take_while(|line| line.starts_with('\t'))
            .map(|line| line.trim_start_matches('\t'))
            .collect();
        (!recipe.is_empty()).then_some(Self(recipe))
    }

    /// Wraps planted recipe lines, so the masking cases need no Makefile rule of their own.
    fn planted(lines: &'static [&'static str]) -> Self { Self(lines.to_vec()) }

    /// Returns why the recipe could be soft-failed, or an empty list when it cannot.
    fn masking_problems(&self, gate: Gate) -> Vec<String> {
        const MASKS: &[&str] = &["||", "|", ";", "command -v", "which ", "true", "exit 0"];
        let needle = gate.needle();
        let mut problems = Vec::new();
        if !self.0.iter().any(|line| line.contains(needle)) {
            problems.push(format!("the recipe never runs `{needle}`"));
        }
        for line in &self.0 {
            if line.starts_with('-') || line.starts_with("@-") {
                problems.push(format!("`{line}` ignores errors"));
            }
            for mask in MASKS.iter().filter(|mask| line.contains(**mask)) {
                problems.push(format!("`{line}` contains `{mask}`"));
            }
        }
        problems
    }
}

/// The gate command a recipe must name for the masking reader.
#[derive(Clone, Copy)]
enum Gate {
    /// The Whitaker invocation.
    Whitaker,
    /// The Clippy invocation.
    Clippy,
}

impl Gate {
    /// The Make variable spelling the recipe must contain.
    fn needle(self) -> &'static str {
        match self {
            Self::Whitaker => "$(WHITAKER)",
            Self::Clippy => "$(CARGO) clippy",
        }
    }
}

/// A masking shape the recipe reader must name, for a planted recipe.
#[derive(Clone, Copy)]
enum Fault {
    /// The recipe as written: no masking, so no problem is expected.
    None,
    /// Failure suppressed with `|| true`.
    OrTrue,
    /// Errors ignored with a `-` recipe prefix.
    Ignored,
    /// An existence guard before the gate.
    Guarded,
    /// A pipeline that hides the gate's status.
    Piped,
    /// The gate is not invoked at all.
    NeverRuns,
}

impl Fault {
    /// The fragment the reader must name, or `None` when no fault was planted.
    fn expected(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::OrTrue => Some("||"),
            Self::Ignored => Some("ignores errors"),
            Self::Guarded => Some("command -v"),
            Self::Piped => Some("|"),
            Self::NeverRuns => Some("never runs"),
        }
    }
}

/// Scenario: the Whitaker and Clippy recipes as written, and ones that mask the gate.
///
/// Invariant: the real recipes carry no existence guard, `|| true`, ignored-error prefix or
/// pipeline; each masking shape is named when it appears.
#[rstest]
#[case::as_written(Recipe::planted(&["unset CARGO_ENCODED_RUSTFLAGS && RUSTFLAGS=\"-D warnings\" $(WHITAKER) --all -- --workspace"]), Fault::None)]
#[case::soft_failed(Recipe::planted(&["$(WHITAKER) --all || true"]), Fault::OrTrue)]
#[case::ignored(Recipe::planted(&["-$(WHITAKER) --all"]), Fault::Ignored)]
#[case::guarded(Recipe::planted(&["command -v $(WHITAKER) && $(WHITAKER) --all"]), Fault::Guarded)]
#[case::piped(Recipe::planted(&["$(WHITAKER) --all | tee log"]), Fault::Piped)]
#[case::removed(Recipe::planted(&["echo skipped"]), Fault::NeverRuns)]
fn the_gate_recipe_reader_refuses_masking(
    #[case] recipe: Recipe,
    #[case] fault: Fault,
) -> Result<(), String> {
    let problems = recipe.masking_problems(Gate::Whitaker);
    match fault.expected() {
        None if problems.is_empty() => Ok(()),
        Some(needle)
            if !problems.is_empty() && problems.iter().any(|problem| problem.contains(needle)) =>
        {
            Ok(())
        }
        _ => Err(format!("for {recipe:?} the reader said {problems:?}")),
    }
}

/// Scenario: the Makefile as written.
///
/// Invariant: `lint` lists Clippy then Whitaker, is built serially, and each half's recipe is a
/// plain command that cannot be soft-failed.
#[test]
fn the_makefile_binds_whitaker_into_a_serial_lint() -> Result<(), String> {
    let header = MAKEFILE
        .lines()
        .find(|line| line.starts_with("lint:"))
        .ok_or("no `lint:` rule")?;
    let before_hash = header.split("##").next().unwrap_or(header);
    if before_hash.split_whitespace().collect::<Vec<_>>()
        != ["lint:", "lint-clippy", "lint-whitaker"]
    {
        return Err(format!("`lint` reaches `{before_hash}`"));
    }
    if !MAKEFILE.lines().any(|line| line == ".NOTPARALLEL: lint") {
        return Err("`lint` is not marked `.NOTPARALLEL`".to_owned());
    }
    let whitaker = Recipe::of(Rule::LintWhitaker).ok_or("`lint-whitaker` has no recipe")?;
    let clippy = Recipe::of(Rule::LintClippy).ok_or("`lint-clippy` has no recipe")?;
    let mut problems = whitaker.masking_problems(Gate::Whitaker);
    problems.extend(clippy.masking_problems(Gate::Clippy));
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{problems:#?}"))
    }
}

/// Returns the lines of the workflow's `Lint` step, up to the next step.
fn lint_step_of() -> Option<Vec<&'static str>> {
    let header = "- name: Lint";
    let mut lines = CI_WORKFLOW.lines().skip_while(|line| line.trim() != header);
    lines.next()?;
    Some(
        lines
            .take_while(|line| !line.trim_start().starts_with("- name:"))
            .collect(),
    )
}

/// Scenario: the CI step that runs the gate.
///
/// Invariant: it runs exactly `make lint`, and nothing lets its failure pass; GitHub fails a step
/// whose shell exits non-zero.
#[test]
fn the_ci_lint_step_runs_make_lint_and_cannot_pass_on_failure() -> Result<(), String> {
    let step = lint_step_of().ok_or("ci.yml has no `Lint` step")?;
    let runs: Vec<&str> = step
        .iter()
        .filter_map(|line| line.trim().strip_prefix("run:"))
        .map(str::trim)
        .collect();
    if runs != ["make lint"] {
        return Err(format!("the Lint step runs {runs:?}"));
    }
    let masked = step.iter().find(|line| {
        line.contains("continue-on-error") || line.contains("||") || line.contains("shell:")
    });
    masked.map_or(Ok(()), |line| {
        Err(format!("the Lint step can mask a failure: `{line}`"))
    })
}
