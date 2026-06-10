//! zsh-style history expansion for the interactive REPL.
//!
//! Supports the common, unambiguous event designators:
//! - `!!`            — the previous command line
//! - `!$` `!^` `!*`  — last / first / all argument(s) of the previous command
//! - `!-N`           — the command N entries back (whole line)
//! - `!!:$` `!!:^` `!!:*` `!!:N`, and the same on `!-N` — word selection
//! - `^old^new`      — quick substitution on the previous command
//!
//! Deliberately **not** supported: `!prefix` / `!N` (absolute). `!prefix` would
//! collide with aishe's `!cmd` force-shell prefix, so any `!` followed by a word
//! character is left untouched — `!ls` stays "force-shell ls", never history.
//!
//! Expansion is quote-aware: ignored inside single quotes (but active in double
//! quotes, like zsh), and `\!` is a literal `!`.

/// Expand history references in `line` against `history` (oldest → newest
/// command lines). Returns `Ok(Some(expanded))` when something changed,
/// `Ok(None)` when nothing applies, or `Err(message)` when a reference has no
/// matching event.
pub fn expand(line: &str, history: &[String]) -> Result<Option<String>, String> {
    // Quick substitution: `^old^new[^]` rewrites the previous command.
    if let Some(rest) = line.strip_prefix('^') {
        let mut parts = rest.splitn(3, '^');
        let old = parts.next().unwrap_or("");
        let new = parts.next().unwrap_or("");
        if old.is_empty() {
            return Err("substitution: empty pattern".to_string());
        }
        let prev = history.last().ok_or("^: event not found")?;
        if !prev.contains(old) {
            return Err(format!("{old}: substitution failed"));
        }
        return Ok(Some(prev.replacen(old, new, 1)));
    }

    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut changed = false;

    while i < chars.len() {
        let c = chars[i];
        if in_single {
            out.push(c);
            if c == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                // `\!` → literal `!`; other escapes pass through unchanged.
                if chars.get(i + 1) == Some(&'!') {
                    out.push('!');
                    i += 2;
                } else if let Some(n) = chars.get(i + 1) {
                    out.push('\\');
                    out.push(*n);
                    i += 2;
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            '\'' if !in_double => {
                in_single = true;
                out.push(c);
                i += 1;
            }
            '"' => {
                in_double = !in_double;
                out.push(c);
                i += 1;
            }
            '!' => match parse_bang(&chars, i, history) {
                Some(Ok((repl, consumed))) => {
                    out.push_str(&repl);
                    i += consumed;
                    changed = true;
                }
                Some(Err(e)) => return Err(e),
                None => {
                    out.push('!');
                    i += 1;
                }
            },
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }

    Ok(if changed { Some(out) } else { None })
}

/// Try to parse a history event beginning at `chars[i] == '!'`. Returns `None`
/// if it isn't an event (leave the `!` literal — this preserves `!cmd`).
fn parse_bang(
    chars: &[char],
    i: usize,
    history: &[String],
) -> Option<Result<(String, usize), String>> {
    let next = *chars.get(i + 1)?;

    // `!$` `!^` `!*` — word designators on the previous command.
    if matches!(next, '$' | '^' | '*') {
        let prev = match history.last() {
            Some(p) => p,
            None => return Some(Err("!: event not found".to_string())),
        };
        let words: Vec<&str> = prev.split_whitespace().collect();
        return Some(Ok((select_word(&words, next), 2)));
    }

    // Resolve the event line for `!!` and `!-N`.
    let (event_line, base_consumed) = if next == '!' {
        match history.last() {
            Some(p) => (p.clone(), 2),
            None => return Some(Err("!!: event not found".to_string())),
        }
    } else if next == '-' {
        let mut j = i + 2;
        let mut digits = String::new();
        while let Some(c) = chars.get(j) {
            if c.is_ascii_digit() {
                digits.push(*c);
                j += 1;
            } else {
                break;
            }
        }
        if digits.is_empty() {
            return None; // `!-` followed by non-digit: literal
        }
        let n: usize = digits.parse().ok()?;
        if n == 0 || n > history.len() {
            return Some(Err(format!("!-{n}: event not found")));
        }
        (history[history.len() - n].clone(), j - i)
    } else {
        // `!` followed by a word char (or anything else): leave literal so the
        // `!cmd` force-shell prefix keeps working.
        return None;
    };

    // Optional `:designator` word selection.
    let mut consumed = base_consumed;
    if chars.get(i + consumed) == Some(&':') {
        if let Some(d) = chars.get(i + consumed + 1).copied() {
            if matches!(d, '$' | '^' | '*') || d.is_ascii_digit() {
                let words: Vec<&str> = event_line.split_whitespace().collect();
                consumed += 2;
                return Some(Ok((select_word(&words, d), consumed)));
            }
        }
    }
    Some(Ok((event_line, consumed)))
}

/// Apply a word designator to a command's words (`words[0]` is the command).
fn select_word(words: &[&str], designator: char) -> String {
    match designator {
        '$' => words.last().copied().unwrap_or("").to_string(),
        '^' => words.get(1).copied().unwrap_or("").to_string(),
        '*' => words.get(1..).map(|w| w.join(" ")).unwrap_or_default(),
        d if d.is_ascii_digit() => {
            let n = d.to_digit(10).unwrap() as usize;
            words.get(n).copied().unwrap_or("").to_string()
        }
        _ => words.join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bang_bang_is_previous_line() {
        assert_eq!(
            expand("!!", &h(&["ls -la"])).unwrap().as_deref(),
            Some("ls -la")
        );
        assert_eq!(
            expand("sudo !!", &h(&["apt update"])).unwrap().as_deref(),
            Some("sudo apt update")
        );
    }

    #[test]
    fn word_designators() {
        assert_eq!(
            expand("!$", &h(&["git commit x"])).unwrap().as_deref(),
            Some("x")
        );
        assert_eq!(
            expand("!^", &h(&["git commit x"])).unwrap().as_deref(),
            Some("commit")
        );
        assert_eq!(
            expand("echo !*", &h(&["git add a b"])).unwrap().as_deref(),
            Some("echo add a b")
        );
        assert_eq!(
            expand("!!:2", &h(&["git add a b"])).unwrap().as_deref(),
            Some("a")
        );
    }

    #[test]
    fn n_back_and_quick_sub() {
        assert_eq!(
            expand("!-2", &h(&["first", "second"])).unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            expand("^ls^ls -la", &h(&["ls"])).unwrap().as_deref(),
            Some("ls -la")
        );
    }

    #[test]
    fn force_shell_prefix_is_preserved() {
        // `!ls` is aishe's force-shell prefix, NOT history — must not expand.
        assert_eq!(expand("!ls", &h(&["ls -la"])).unwrap(), None);
        assert_eq!(expand("!grep foo", &h(&["grep bar"])).unwrap(), None);
    }

    #[test]
    fn literal_and_quoted_bangs_are_untouched() {
        assert_eq!(expand("[ ! -f x ]", &h(&["y"])).unwrap(), None); // space after !
        assert_eq!(expand("echo hi!", &h(&["y"])).unwrap(), None); // ! at end
        assert_eq!(expand("echo 'no !! here'", &h(&["y"])).unwrap(), None); // single-quoted
        assert_eq!(
            expand("echo \\!!", &h(&["y"])).unwrap(),
            None // escaped: no expansion (shell handles the backslash)
        );
    }

    #[test]
    fn expands_inside_double_quotes() {
        assert_eq!(
            expand("echo \"!!\"", &h(&["hi"])).unwrap().as_deref(),
            Some("echo \"hi\"")
        );
    }

    #[test]
    fn missing_event_is_error() {
        assert!(expand("!!", &[]).is_err());
        assert!(expand("!-9", &h(&["only one"])).is_err());
        assert!(expand("^missing^x", &h(&["abc"])).is_err());
    }

    #[test]
    fn no_history_reference_is_none() {
        assert_eq!(expand("echo hello", &h(&["x"])).unwrap(), None);
    }
}
