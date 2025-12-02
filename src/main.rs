//! Binary entry point for the Mriya CLI.

fn main() {
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Entry point must remain non-const while CLI is wired up"
    )]
    fn _placeholder() {}
}
