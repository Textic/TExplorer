//! Everything-style search query parser & matcher shared by the MFT index and parallel fallback.
//!
//! Supported syntax:
//! - Multiple space-separated terms (logical AND) and `"quoted phrases"`
//! - Negated terms with `!` (e.g. `!target`, `!ext:pdb`)
//! - Extension filters: `ext:rs` or `ext:rs;toml;md`
//! - Entry type filters: `dir:` / `folder:` (folders only), `file:` (files only)
//! - Size comparisons: `size:>50mb`, `size:<10kb`, `size:>=1gib`, `size:0`
//! - Path filter: `path:X:\TExplorer` or `in:C:\Users`
//! - Case-insensitive regular expressions: `/^cargo.*\.toml$/`

use regex::{Regex, RegexBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeOp {
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Equal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeComparison {
    pub op: SizeOp,
    pub bytes: u64,
}

impl SizeComparison {
    pub fn matches(self, size: u64) -> bool {
        match self.op {
            SizeOp::Greater => size > self.bytes,
            SizeOp::GreaterEq => size >= self.bytes,
            SizeOp::Less => size < self.bytes,
            SizeOp::LessEq => size <= self.bytes,
            SizeOp::Equal => size == self.bytes,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedQuery {
    pub raw: String,
    pub include_terms: Vec<String>,
    pub exclude_terms: Vec<String>,
    pub include_exts: Vec<String>,
    pub exclude_exts: Vec<String>,
    pub only_dirs: bool,
    pub only_files: bool,
    pub size_filters: Vec<SizeComparison>,
    pub path_filters: Vec<String>,
    pub regexes: Vec<Regex>,
}

/// Lightweight view of an indexed or walked filesystem entry evaluated against a `ParsedQuery`.
#[derive(Debug, Clone, Copy)]
pub struct QueryCandidate<'a> {
    pub name_lower: &'a str,
    pub is_dir: bool,
    pub size: u64,
}

impl ParsedQuery {
    /// Returns `true` when the query contains no filtering criteria at all.
    pub fn is_empty(&self) -> bool {
        self.include_terms.is_empty()
            && self.exclude_terms.is_empty()
            && self.include_exts.is_empty()
            && self.exclude_exts.is_empty()
            && !self.only_dirs
            && !self.only_files
            && self.size_filters.is_empty()
            && self.path_filters.is_empty()
            && self.regexes.is_empty()
    }

    /// Fast pre-filter evaluated strictly on name, directory flag, and size before reconstructing full path.
    pub fn matches_fast(&self, cand: QueryCandidate<'_>) -> bool {
        if self.only_dirs && !cand.is_dir {
            return false;
        }
        if self.only_files && cand.is_dir {
            return false;
        }
        if !self.size_filters.is_empty() {
            // Size filters exclude directories unless `dir:` was explicitly combined.
            if cand.is_dir && !self.only_dirs {
                return false;
            }
            for sf in &self.size_filters {
                if !sf.matches(cand.size) {
                    return false;
                }
            }
        }
        if !self.include_exts.is_empty() || !self.exclude_exts.is_empty() {
            let ext = if cand.is_dir {
                ""
            } else {
                cand.name_lower
                    .rsplit_once('.')
                    .map(|(_, e)| e)
                    .unwrap_or("")
            };
            if !self.include_exts.is_empty() && !self.include_exts.iter().any(|e| e == ext) {
                return false;
            }
            if !self.exclude_exts.is_empty() && self.exclude_exts.iter().any(|e| e == ext) {
                return false;
            }
        }
        for term in &self.include_terms {
            if !cand.name_lower.contains(term.as_str()) {
                return false;
            }
        }
        for term in &self.exclude_terms {
            if cand.name_lower.contains(term.as_str()) {
                return false;
            }
        }
        for re in &self.regexes {
            if !re.is_match(cand.name_lower) {
                return false;
            }
        }
        true
    }

    /// Evaluates path-level predicates (`path:`, `in:`, plus `!term` against full path segments).
    pub fn matches_full_path(&self, full_path_lower: &str, scope_lower: Option<&str>) -> bool {
        if let Some(scope) = scope_lower
            && !path_within_scope(full_path_lower, scope)
        {
            return false;
        }
        for pf in &self.path_filters {
            if !full_path_lower.contains(pf.as_str()) {
                return false;
            }
        }
        for ex in &self.exclude_terms {
            if full_path_lower.contains(ex.as_str()) {
                return false;
            }
        }
        true
    }

    /// Computes a lightweight relevance score (lower is better) for sorting top hits.
    pub fn relevance_rank(&self, name_lower: &str, is_dir: bool) -> u8 {
        if let Some(first) = self.include_terms.first() {
            if name_lower == first {
                return 0;
            }
            if name_lower.starts_with(first.as_str()) {
                return if is_dir { 1 } else { 2 };
            }
            return if is_dir { 3 } else { 4 };
        }
        if is_dir { 3 } else { 4 }
    }
}

/// Checks if `full_path_lower` equals or is nested inside `scope_lower`.
pub fn path_within_scope(full_path_lower: &str, scope_lower: &str) -> bool {
    let clean_scope = scope_lower.trim_end_matches(['\\', '/']);
    if clean_scope.is_empty() {
        return true;
    }
    if full_path_lower == clean_scope {
        return true;
    }
    if let Some(rest) = full_path_lower.strip_prefix(clean_scope) {
        return rest.starts_with('\\') || rest.starts_with('/');
    }
    false
}

/// Parses an Everything-style query string into structured predicates.
pub fn parse_query(raw: &str) -> ParsedQuery {
    let mut q = ParsedQuery {
        raw: raw.to_string(),
        include_terms: Vec::new(),
        exclude_terms: Vec::new(),
        include_exts: Vec::new(),
        exclude_exts: Vec::new(),
        only_dirs: false,
        only_files: false,
        size_filters: Vec::new(),
        path_filters: Vec::new(),
        regexes: Vec::new(),
    };

    for token in tokenize_query(raw) {
        if token.is_empty() {
            continue;
        }
        let (negated, body) = if let Some(rest) = token.strip_prefix('!') {
            (true, rest)
        } else {
            (false, token.as_str())
        };
        if body.is_empty() {
            continue;
        }

        let lower = body.to_ascii_lowercase();

        // Regex literal `/pattern/`
        if !negated && body.len() >= 3 && body.starts_with('/') && body.ends_with('/') {
            let pattern = &body[1..body.len() - 1];
            if let Ok(re) = RegexBuilder::new(pattern).case_insensitive(true).build() {
                q.regexes.push(re);
                continue;
            }
        }

        // Type shortcuts (`dir:`, `folder:`, `file:`)
        if matches!(lower.as_str(), "dir:" | "folder:" | "is:dir" | "is:folder") {
            if negated {
                q.only_files = true;
            } else {
                q.only_dirs = true;
            }
            continue;
        }
        if matches!(lower.as_str(), "file:" | "is:file") {
            if negated {
                q.only_dirs = true;
            } else {
                q.only_files = true;
            }
            continue;
        }

        // Extension filter (`ext:rs;toml;md`)
        if let Some(exts_raw) = lower.strip_prefix("ext:") {
            let exts: Vec<String> = exts_raw
                .split([';', ','])
                .map(|s| s.trim().trim_start_matches('.').to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !exts.is_empty() {
                if negated {
                    q.exclude_exts.extend(exts);
                } else {
                    q.include_exts.extend(exts);
                }
                continue;
            }
        }

        // Size filter (`size:>50mb`, `size:<=10kb`, `size:0`)
        if let Some(size_raw) = lower.strip_prefix("size:")
            && let Some(cmp) = parse_size_comparison(size_raw)
        {
            q.size_filters.push(cmp);
            continue;
        }

        // Path filter (`path:X:\TExplorer` or `in:downloads`)
        if let Some(p_raw) = lower
            .strip_prefix("path:")
            .or_else(|| lower.strip_prefix("in:"))
        {
            let norm = p_raw.trim().replace('/', "\\");
            if !norm.is_empty() {
                q.path_filters.push(norm);
                continue;
            }
        }

        if negated {
            q.exclude_terms.push(lower);
        } else {
            q.include_terms.push(lower);
        }
    }

    q
}

fn tokenize_query(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;

    for ch in raw.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            other => cur.push(other),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_size_comparison(raw: &str) -> Option<SizeComparison> {
    let s = raw.trim();
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (SizeOp::GreaterEq, r)
    } else if let Some(r) = s.strip_prefix("<=") {
        (SizeOp::LessEq, r)
    } else if let Some(r) = s.strip_prefix('>') {
        (SizeOp::Greater, r)
    } else if let Some(r) = s.strip_prefix('<') {
        (SizeOp::Less, r)
    } else if let Some(r) = s.strip_prefix('=') {
        (SizeOp::Equal, r)
    } else {
        (SizeOp::Equal, s)
    };

    let bytes = parse_byte_literal(rest.trim())?;
    Some(SizeComparison { op, bytes })
}

fn parse_byte_literal(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let split_at = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let num: f64 = s[..split_at].parse().ok()?;
    let unit = s[split_at..].trim();
    let mult: f64 = match unit {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((num * mult).max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches_everything_syntax() {
        let q = parse_query("invoice ext:pdf;docx size:>1mb !draft path:finance");
        assert_eq!(q.include_terms, vec!["invoice"]);
        assert_eq!(q.exclude_terms, vec!["draft"]);
        assert_eq!(q.include_exts, vec!["pdf", "docx"]);
        assert_eq!(q.path_filters, vec!["finance"]);
        assert_eq!(q.size_filters.len(), 1);
        assert_eq!(q.size_filters[0].bytes, 1024 * 1024);

        assert!(q.matches_fast(QueryCandidate {
            name_lower: "invoice_2026.pdf",
            is_dir: false,
            size: 2 * 1024 * 1024,
        }));
        assert!(!q.matches_fast(QueryCandidate {
            name_lower: "invoice_draft.pdf",
            is_dir: false,
            size: 2 * 1024 * 1024,
        }));
        assert!(!q.matches_fast(QueryCandidate {
            name_lower: "invoice_2026.pdf",
            is_dir: false,
            size: 512,
        }));
        assert!(q.matches_full_path(r"d:\docs\finance\invoice_2026.pdf", Some(r"d:\docs")));
        assert!(!q.matches_full_path(r"d:\docs\personal\invoice_2026.pdf", Some(r"d:\docs")));
    }

    #[test]
    fn supports_dir_file_and_regex_filters() {
        let q_dir = parse_query("dir: src");
        assert!(q_dir.matches_fast(QueryCandidate {
            name_lower: "src",
            is_dir: true,
            size: 0,
        }));
        assert!(!q_dir.matches_fast(QueryCandidate {
            name_lower: "src.rs",
            is_dir: false,
            size: 100,
        }));

        let q_re = parse_query(r"/^cargo\.(toml|lock)$/");
        assert!(q_re.matches_fast(QueryCandidate {
            name_lower: "cargo.toml",
            is_dir: false,
            size: 100,
        }));
        assert!(!q_re.matches_fast(QueryCandidate {
            name_lower: "my_cargo.toml",
            is_dir: false,
            size: 100,
        }));
    }
}
