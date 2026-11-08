//! Shallow parser that splits one Bash command line into segments. It never executes or expands anything (text only).
//! Unknown forms (command substitution, backticks, process substitution, heredoc) are flagged `complex = true`; the classifier treats them as closed (block).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    /// True if the word has no unquoted `$`, glob (`* ? [`) or leading `~`: the counter may use it literally as a path.
    pub literal: bool,
    /// Word that started with a quote. The word right after the PowerShell call operator `&` is false (it is the command to run).
    pub quoted: bool,
}

/// Separator **before** a segment. Only `AndAnd` means "this segment runs only if the previous one succeeded"; `cd` following relies on that alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Start,
    AndAnd,
    /// `;`, newline, lone `&`, `(`, `)` - runs even if the previous one failed
    Seq,
    OrOr,
    Pipe,
}

#[derive(Debug, Clone, Default)]
pub struct Segment {
    pub words: Vec<Word>,
    /// Output redirect target (`>`, `>>`, `&>`, `N>`). None = target is not literal (or missing).
    pub redirects: Vec<Option<String>>,
    pub joined_by: Join,
}

#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub segments: Vec<Segment>,
    pub complex: bool,
    /// Inner text of a matched Bash `$(...)` (if it contains nesting, backticks, heredoc, process substitution or a comment, the segment is `complex` instead).
    pub substs: Vec<String>,
}

pub fn parse(cmd: &str) -> Parsed {
    parse_with(cmd, false)
}

/// PowerShell tool command: backslash is a path separator (not an escape). Other rules match the Bash variant -
/// backtick (PowerShell escape) and `$(` remain complex; `$x` and globs are not literal.
pub fn parse_ps(cmd: &str) -> Parsed {
    parse_with(cmd, true)
}

#[allow(unused_assignments)] // the end_word!/end_seg! macros reset state even after the last word
fn parse_with(cmd: &str, ps: bool) -> Parsed {
    let c: Vec<char> = cmd.chars().collect();
    let mut out = Parsed::default();
    let mut seg = Segment::default();
    let mut cur = String::new();
    let mut lit = true;
    let mut in_word = false;
    let mut quoted = false;
    let mut called = false; // after PowerShell `&` (call operator): next word is the command to run
    let mut pending_out = false; // next word is the output redirect target
    let mut skip_in = false; // next word is an input redirect target (ignored)
    let mut i = 0;

    macro_rules! end_word {
        () => {
            if in_word {
                let w = Word { text: std::mem::take(&mut cur), literal: lit, quoted: quoted && !called };
                called = false;
                if pending_out {
                    // PowerShell `> $null` / `2>$null` is a discard target (exactly `$null`; `$nullx` and other variables are treated normally)
                    if !(ps && w.text.eq_ignore_ascii_case("$null")) {
                        seg.redirects.push(if w.literal { Some(w.text) } else { None });
                    }
                    pending_out = false;
                } else if skip_in {
                    skip_in = false;
                } else {
                    seg.words.push(w);
                }
            }
            cur.clear();
            lit = true;
            quoted = false;
            in_word = false;
        };
    }
    macro_rules! end_seg {
        ($j:expr) => {
            end_word!();
            if pending_out {
                seg.redirects.push(None);
                pending_out = false;
            }
            skip_in = false;
            if !seg.words.is_empty() || !seg.redirects.is_empty() {
                out.segments.push(std::mem::take(&mut seg));
                seg.joined_by = $j;
            } else if matches!(seg.joined_by, Join::Start | Join::AndAnd) {
                // Across an empty segment (`&& \n`, `; (`) the weaker separator (not &&) is kept, so following stays conservative
                seg.joined_by = $j;
            }
        };
    }

    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        match ch {
            ' ' | '\t' | '\r' => {
                end_word!();
            }
            '\n' | ';' | '(' | ')' => {
                end_seg!(Join::Seq);
            }
            '&' => {
                if next == Some('>') {
                    end_word!();
                    i += 1;
                    if c.get(i + 1) == Some(&'>') {
                        i += 1;
                    }
                    pending_out = true;
                } else if next == Some('&') {
                    i += 1;
                    end_seg!(Join::AndAnd);
                } else {
                    end_seg!(Join::Seq);
                    called = ps;
                }
            }
            '|' => {
                let j = if next == Some('|') { Join::OrOr } else { Join::Pipe };
                if next == Some('|') || next == Some('&') {
                    i += 1;
                }
                end_seg!(j);
            }
            '`' => {
                out.complex = true;
                lit = false;
                in_word = true;
            }
            '$' => {
                // matched `$(...)` = a word segment (not literal) + its inner text goes to substs
                if let Some(e) = (!ps && next == Some('(') && c.get(i + 2) != Some(&'(')).then(|| subst_end(&c, i + 2)).flatten() {
                    out.substs.push(c[i + 2..e].iter().collect());
                    cur.extend(&c[i..=e]);
                    lit = false;
                    in_word = true;
                    i = e + 1;
                    continue;
                }
                if next == Some('(') || next == Some('{') {
                    out.complex = next == Some('(') || out.complex;
                }
                cur.push('$');
                lit = false;
                in_word = true;
            }
            '<' => {
                if next == Some('<') || next == Some('(') {
                    out.complex = true;
                    i += 1;
                } else {
                    end_word!();
                    skip_in = true;
                }
            }
            '>' => {
                if next == Some('(') {
                    out.complex = true;
                    i += 1;
                } else {
                    // An fd number like the 2 in `2>` is not a word
                    if in_word && !cur.is_empty() && cur.chars().all(|d| d.is_ascii_digit()) {
                        cur.clear();
                        in_word = false;
                        lit = true;
                    } else {
                        end_word!();
                    }
                    if next == Some('>') || next == Some('|') {
                        i += 1;
                    }
                    if c.get(i + 1) == Some(&'&') {
                        // `>&2`, `2>&1` = fd duplication, no file target
                        i += 1;
                        while i + 1 < c.len() && (c[i + 1].is_ascii_digit() || c[i + 1] == '-') {
                            i += 1;
                        }
                    } else {
                        pending_out = true;
                    }
                }
            }
            '\'' => {
                quoted |= !in_word;
                in_word = true;
                i += 1;
                while i < c.len() && c[i] != '\'' {
                    cur.push(c[i]);
                    i += 1;
                }
            }
            '"' => {
                quoted |= !in_word;
                in_word = true;
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if !ps && c[i] == '\\' && i + 1 < c.len() && matches!(c[i + 1], '"' | '\\' | '$' | '`') {
                        i += 1;
                        cur.push(c[i]);
                    } else if let Some(e) = (!ps && c[i] == '$' && c.get(i + 1) == Some(&'(') && c.get(i + 2) != Some(&'(')).then(|| subst_end(&c, i + 2)).flatten() {
                        // `"$(...)"` inside double quotes follows the same rule
                        out.substs.push(c[i + 2..e].iter().collect());
                        cur.extend(&c[i..=e]);
                        lit = false;
                        i = e + 1;
                        continue;
                    } else {
                        if c[i] == '$' {
                            lit = false;
                            if c.get(i + 1) == Some(&'(') {
                                out.complex = true;
                            }
                        }
                        if c[i] == '`' {
                            out.complex = true;
                            lit = false;
                        }
                        cur.push(c[i]);
                    }
                    i += 1;
                }
            }
            '\\' if ps => {
                in_word = true;
                cur.push(ch);
            }
            '\\' => {
                in_word = true;
                if let Some(n) = next {
                    if n != '\n' {
                        cur.push(n);
                    }
                    i += 1;
                }
            }
            '#' if !in_word => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '*' | '?' | '[' => {
                lit = false;
                in_word = true;
                cur.push(ch);
            }
            '~' if !in_word => {
                lit = false;
                in_word = true;
                cur.push(ch);
            }
            _ => {
                in_word = true;
                cur.push(ch);
            }
        }
        i += 1;
    }
    end_seg!(Join::Start);
    out
}

/// Position of the matching `)` for a `$(`, scanning from just after it (`from`). Returns None if the inside has a nested `$(`, backtick, `<(`, `>(`, heredoc, a word-initial `#` (comment) or an unclosed quote
/// (the caller then falls back to `complex`). Parentheses inside quotes are not counted.
fn subst_end(c: &[char], from: usize) -> Option<usize> {
    let (mut depth, mut i, mut word_start) = (0usize, from, true);
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        match ch {
            '\\' => i += 1,
            '\'' => {
                i += 1;
                while c.get(i)? != &'\'' {
                    i += 1;
                }
            }
            '"' => {
                i += 1;
                while c.get(i)? != &'"' {
                    match c[i] {
                        '\\' => i += 1,
                        '`' => return None,
                        '$' if c.get(i + 1) == Some(&'(') => return None,
                        _ => {}
                    }
                    i += 1;
                }
            }
            '`' => return None,
            '$' | '<' | '>' if next == Some('(') => return None,
            '<' if next == Some('<') => return None,
            '#' if word_start => return None,
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
        word_start = ch.is_whitespace() || ";|&(".contains(ch);
        i += 1;
    }
    None
}

/// "Deep words" for risky-pattern search: re-split on whitespace and shell symbols, including quoted text, and lowercase.
pub fn deep_words<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = vec![];
    for t in texts {
        for w in t.split(|ch: char| ch.is_whitespace() || ";&|()<>`$'\"=,{}".contains(ch)) {
            if !w.is_empty() {
                out.push(w.to_lowercase());
            }
        }
    }
    out
}

/// Lowercase name with directory prefix and `.exe` stripped (`/usr/bin/rm` -> `rm`).
pub fn base(w: &str) -> String {
    let b = w.rsplit(['/', '\\']).next().unwrap_or(w).to_lowercase();
    b.strip_suffix(".exe").map(|s| s.to_string()).unwrap_or(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn words(p: &Parsed, i: usize) -> Vec<String> {
        p.segments[i].words.iter().map(|w| w.text.clone()).collect()
    }
    #[test]
    fn splits_operators_and_redirects() {
        let p = parse("ls -la && echo 'a b' > out.txt 2>&1 | tee x; cat <in.txt");
        assert_eq!(words(&p, 0), ["ls", "-la"]);
        assert_eq!(words(&p, 1), ["echo", "a b"]);
        assert_eq!(p.segments[1].redirects, vec![Some("out.txt".to_string())]);
        assert_eq!(words(&p, 2), ["tee", "x"]);
        assert_eq!(words(&p, 3), ["cat"]);
        assert!(!p.complex);
    }
    #[test]
    fn records_join_before_each_segment() {
        let j = |s: &str| parse(s).segments.iter().map(|g| g.joined_by).collect::<Vec<_>>();
        use Join::*;
        assert_eq!(j("a && b; c || d | e & f\ng"), [Start, AndAnd, Seq, OrOr, Pipe, Seq, Seq]);
        assert_eq!(j("a |& b"), [Start, Pipe]);
        assert_eq!(j("a &&\nb"), [Start, Seq]);
        assert_eq!(j("a && (b)"), [Start, Seq]);
        assert_eq!(j("a > x && b"), [Start, AndAnd]);
    }
    #[test]
    fn marks_complex_forms() {
        // a matched `$(rm x)` goes to substs instead of complex (the classifier treats the inner `rm x` as a write -> the usual card)
        assert_eq!(parse("echo $(rm x)").substs, ["rm x"]);
        assert!(parse("echo `rm x`").complex);
        assert!(parse("cat <<EOF\nx\nEOF").complex);
        assert!(parse("diff <(ls) b").complex);
        assert!(!parse("echo \"$HOME\"").complex);
    }
    #[test]
    fn keeps_plain_substitutions_whole() {
        // a `$(...)` with a matching `)` is one word (not literal) + its inner text goes to substs
        let p = parse("du -sh $(git rev-parse --show-toplevel) && ls");
        assert!(!p.complex);
        assert_eq!(p.substs, ["git rev-parse --show-toplevel"]);
        assert_eq!(words(&p, 0), ["du", "-sh", "$(git rev-parse --show-toplevel)"]);
        assert!(!p.segments[0].words[2].literal);
        assert_eq!(words(&p, 1), ["ls"]);
        let q = parse(r#"echo "$(git log -1 --format="%h (x)")" b"#);
        assert!(!q.complex);
        assert_eq!(q.substs, [r#"git log -1 --format="%h (x)""#]);
        assert_eq!(words(&q, 0), ["echo", r#"$(git log -1 --format="%h (x)")"#, "b"]);
        assert_eq!(parse("echo $(ls (a) b)").substs, ["ls (a) b"]);
        assert!(parse("echo '$(rm x)'").substs.is_empty());
    }
    #[test]
    fn odd_substitutions_stay_complex() {
        for s in ["echo $(echo $(ls))", "echo $(echo `ls`)", "echo $((1+2))", "echo $(ls", "echo $(cat <(ls))", "echo $(cat <<E\nx\nE\n)", "echo $(ls # )\n)", r#"echo "$(echo "$(ls)")""#] {
            let p = parse(s);
            assert!(p.complex, "{s}");
        }
        // PowerShell subexpressions are left as is (handled by the PowerShell interpreter path)
        let p = parse_ps("echo $(ls)");
        assert!(p.complex && p.substs.is_empty());
    }
    #[test]
    fn null_and_fd_redirects() {
        let p = parse("cmd 2>/dev/null >&2");
        assert_eq!(p.segments[0].redirects, vec![Some("/dev/null".to_string())]);
        assert_eq!(words(&p, 0), ["cmd"]);
    }
    #[test]
    fn non_literal_words() {
        let p = parse("rm -rf *.log $X ~/a b");
        let lits: Vec<bool> = p.segments[0].words.iter().map(|w| w.literal).collect();
        assert_eq!(lits, [true, true, false, false, false, true]);
    }
}
