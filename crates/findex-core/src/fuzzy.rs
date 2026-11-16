use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Fuzzy file-name matcher (nucleo, the matcher used by Helix). One per thread; reuse it.
pub struct Fuzzy {
    matcher: Matcher,
    pattern: Pattern,
    buf: Vec<char>,
}

impl Fuzzy {
    pub fn new(query: &str) -> Fuzzy {
        let mut cfg = Config::DEFAULT;
        cfg.prefer_prefix = true;
        Fuzzy {
            matcher: Matcher::new(cfg),
            pattern: Pattern::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy),
            buf: Vec::new(),
        }
    }

    /// Higher is better. `None` means no match.
    pub fn score(&mut self, name: &str) -> Option<u32> {
        self.pattern.score(Utf32Str::new(name, &mut self.buf), &mut self.matcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequence_and_ranks_prefix() {
        let mut f = Fuzzy::new("rdm");
        assert!(f.score("README.md").is_some());
        assert!(f.score("cargo.toml").is_none());
        let mut f = Fuzzy::new("main");
        assert!(f.score("main.rs").unwrap() > f.score("my_animation.rs").unwrap_or(0));
    }
}
