//! `.exoignore` — exclude files from the build context (E2).
//!
//! A gitignore-lite matcher so build COPY steps don't pull `node_modules`,
//! `.git`, or secrets into image layers. Keeping junk out of layers is both a
//! size win (smaller, more dedup-friendly layers) and a security one (no
//! accidental secret baked into a pushable image).

/// A parsed set of ignore patterns.
#[derive(Debug, Clone, Default)]
pub struct ExoIgnore {
    patterns: Vec<String>,
}

impl ExoIgnore {
    /// Parse `.exoignore` text: one pattern per line; `#` comments and blank
    /// lines ignored. Patterns match either a full forward-slash relative path
    /// or any single path component (so `node_modules` matches it at any depth).
    pub fn parse(text: &str) -> Self {
        let patterns = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.trim_end_matches('/').to_string())
            .collect();
        Self { patterns }
    }

    /// Whether nothing is ignored (skip-the-work fast path).
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Is this relative path (forward-slash separated) excluded?
    pub fn is_ignored(&self, rel_path: &str) -> bool {
        let rel = rel_path.replace('\\', "/");
        let rel = rel.trim_start_matches("./").trim_start_matches('/');
        self.patterns.iter().any(|p| {
            // Match the whole path, or any individual component.
            wildcard_match(p, rel) || rel.split('/').any(|seg| wildcard_match(p, seg))
        })
    }
}

/// Minimal glob: `*` matches any run (including empty) within a segment; `?`
/// matches one char. No regex dependency.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    // Classic two-pointer wildcard match with backtracking on `*`.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = pi;
            mark = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_components_and_globs() {
        let ig = ExoIgnore::parse(
            "# junk\nnode_modules/\n.git\n*.log\nsecrets.env\nbuild/cache\n",
        );
        assert!(ig.is_ignored("node_modules"));
        assert!(ig.is_ignored("src/node_modules/x.js")); // component at depth
        assert!(ig.is_ignored(".git/config"));
        assert!(ig.is_ignored("logs/app.log")); // *.log
        assert!(ig.is_ignored("secrets.env"));
        assert!(ig.is_ignored("build/cache")); // full-path pattern
        assert!(!ig.is_ignored("src/main.py"));
        assert!(!ig.is_ignored("build/output")); // build/cache != build/output
    }

    #[test]
    fn empty_ignores_nothing() {
        let ig = ExoIgnore::parse("\n#only comments\n");
        assert!(ig.is_empty());
        assert!(!ig.is_ignored("anything"));
    }

    #[test]
    fn wildcard_edge_cases() {
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(wildcard_match("a?c", "abc"));
        assert!(!wildcard_match("a?c", "ac"));
        assert!(wildcard_match("*.tar.gz", "layer.tar.gz"));
    }
}
