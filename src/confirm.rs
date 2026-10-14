use std::io::{BufRead, IsTerminal, Write};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Run,
    DryRun,
    Ask,
    Refuse,
}

pub fn decide(writes: bool, yes: bool, dry_run: bool, interactive: bool) -> Decision {
    if dry_run {
        Decision::DryRun
    } else if !writes || yes {
        Decision::Run
    } else if interactive {
        Decision::Ask
    } else {
        Decision::Refuse
    }
}

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

pub fn accepted(answer: &str) -> bool {
    matches!(answer.trim().to_lowercase().as_str(), "s" | "sim" | "y" | "yes")
}

pub fn ask(question: &str) -> Result<bool> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{question} [s/N] ")?;
    stderr.flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(accepted(&answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_run_without_asking() {
        assert_eq!(decide(false, false, false, false), Decision::Run);
        assert_eq!(decide(false, false, false, true), Decision::Run);
    }

    #[test]
    fn writes_need_yes_or_a_terminal() {
        assert_eq!(decide(true, true, false, false), Decision::Run);
        assert_eq!(decide(true, false, false, true), Decision::Ask);
        assert_eq!(decide(true, false, false, false), Decision::Refuse);
    }

    #[test]
    fn dry_run_always_wins() {
        assert_eq!(decide(true, true, true, false), Decision::DryRun);
        assert_eq!(decide(false, false, true, true), Decision::DryRun);
    }

    #[test]
    fn answers_in_portuguese_or_english() {
        assert!(accepted("s\n") && accepted("Sim") && accepted("y"));
        assert!(!accepted("") && !accepted("n") && !accepted("talvez"));
    }
}
