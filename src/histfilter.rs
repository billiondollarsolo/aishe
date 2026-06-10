//! A history wrapper adding zsh-style filtering: consecutive-duplicate removal
//! (`HIST_IGNORE_DUPS`) and pattern exclusion (`HISTIGNORE`).
//!
//! reedline persists every submitted line via [`History::save`]. This wrapper
//! delegates the whole [`History`] trait to an inner store and only intercepts
//! `save`: a line equal to the previous saved line, or matching one of the
//! configured glob patterns, is not persisted (so it never clutters history or
//! up-arrow recall). The `ignorespace` case (don't save lines starting with a
//! space) is handled separately by reedline's own exclusion prefix.

use reedline::{
    History, HistoryItem, HistoryItemId, HistorySessionId, Result as HistResult, SearchQuery,
};
use regex::Regex;

pub struct FilteredHistory {
    inner: Box<dyn History>,
    ignore_dups: bool,
    patterns: Vec<Regex>,
    /// The most recently persisted command line (for dup detection).
    last: Option<String>,
}

impl FilteredHistory {
    /// Wrap `inner`. `ignore_dups` drops consecutive duplicates; each entry in
    /// `ignore_globs` is a glob (`*` wildcard) that, when it matches a command,
    /// excludes it from history.
    pub fn new(inner: Box<dyn History>, ignore_dups: bool, ignore_globs: &[String]) -> Self {
        let patterns = ignore_globs
            .iter()
            .filter_map(|g| Regex::new(&glob_to_regex(g)).ok())
            .collect();
        Self {
            inner,
            ignore_dups,
            patterns,
            last: None,
        }
    }

    fn should_ignore(&self, command: &str) -> bool {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return true;
        }
        if self.ignore_dups && self.last.as_deref() == Some(command) {
            return true;
        }
        self.patterns.iter().any(|re| re.is_match(trimmed))
    }
}

/// Convert a zsh-style glob (`*` matches anything) into an anchored regex.
fn glob_to_regex(glob: &str) -> String {
    let mut re = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    re
}

impl History for FilteredHistory {
    fn save(&mut self, h: HistoryItem) -> HistResult<HistoryItem> {
        if self.should_ignore(&h.command_line) {
            // Not persisted: return the item unchanged (id stays None, which the
            // editor treats as "nothing to record/recall for this entry").
            return Ok(h);
        }
        let saved = self.inner.save(h)?;
        self.last = Some(saved.command_line.clone());
        Ok(saved)
    }

    fn load(&self, id: HistoryItemId) -> HistResult<HistoryItem> {
        self.inner.load(id)
    }

    fn count(&self, query: SearchQuery) -> HistResult<i64> {
        self.inner.count(query)
    }

    fn search(&self, query: SearchQuery) -> HistResult<Vec<HistoryItem>> {
        self.inner.search(query)
    }

    fn update(
        &mut self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> HistResult<()> {
        self.inner.update(id, updater)
    }

    fn clear(&mut self) -> HistResult<()> {
        self.last = None;
        self.inner.clear()
    }

    fn delete(&mut self, h: HistoryItemId) -> HistResult<()> {
        self.inner.delete(h)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn session(&self) -> Option<HistorySessionId> {
        self.inner.session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_to_regex_matches() {
        // `*` is a wildcard: "ls*" matches anything starting with "ls".
        let re = Regex::new(&glob_to_regex("ls*")).unwrap();
        assert!(re.is_match("ls"));
        assert!(re.is_match("ls -la"));
        assert!(re.is_match("lsof"));
        // an exact pattern (no wildcard) is anchored, so only "ls" matches.
        let re = Regex::new(&glob_to_regex("ls")).unwrap();
        assert!(re.is_match("ls"));
        assert!(!re.is_match("ls -la"));
        assert!(!re.is_match("lsof"));
    }

    #[test]
    fn glob_special_chars_escaped() {
        let re = Regex::new(&glob_to_regex("git commit*")).unwrap();
        assert!(re.is_match("git commit -m x"));
        assert!(!re.is_match("git push"));
        // a literal dot is escaped
        let re = Regex::new(&glob_to_regex("a.b")).unwrap();
        assert!(re.is_match("a.b"));
        assert!(!re.is_match("axb"));
    }

    // A minimal in-memory History to test the filtering behavior.
    #[derive(Default)]
    struct MemHistory {
        items: Vec<String>,
    }
    impl History for MemHistory {
        fn save(&mut self, mut h: HistoryItem) -> HistResult<HistoryItem> {
            self.items.push(h.command_line.clone());
            h.id = Some(HistoryItemId::new(self.items.len() as i64));
            Ok(h)
        }
        fn load(&self, _: HistoryItemId) -> HistResult<HistoryItem> {
            unimplemented!()
        }
        fn count(&self, _: SearchQuery) -> HistResult<i64> {
            Ok(self.items.len() as i64)
        }
        fn search(&self, _: SearchQuery) -> HistResult<Vec<HistoryItem>> {
            Ok(vec![])
        }
        fn update(
            &mut self,
            _: HistoryItemId,
            _: &dyn Fn(HistoryItem) -> HistoryItem,
        ) -> HistResult<()> {
            Ok(())
        }
        fn clear(&mut self) -> HistResult<()> {
            self.items.clear();
            Ok(())
        }
        fn delete(&mut self, _: HistoryItemId) -> HistResult<()> {
            Ok(())
        }
        fn sync(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn session(&self) -> Option<HistorySessionId> {
            None
        }
    }

    fn save(h: &mut FilteredHistory, cmd: &str) {
        let _ = h.save(HistoryItem::from_command_line(cmd));
    }

    #[test]
    fn dedup_skips_consecutive_duplicates() {
        let mut h = FilteredHistory::new(Box::new(MemHistory::default()), true, &[]);
        save(&mut h, "ls");
        save(&mut h, "ls"); // dup, skipped
        save(&mut h, "pwd");
        save(&mut h, "ls"); // not consecutive, kept
        assert_eq!(
            h.count(SearchQuery::everything(
                reedline::SearchDirection::Forward,
                None
            ))
            .unwrap(),
            3
        );
    }

    #[test]
    fn histignore_excludes_patterns() {
        let mut h = FilteredHistory::new(
            Box::new(MemHistory::default()),
            false,
            &["ls".into(), "cd *".into()],
        );
        save(&mut h, "ls"); // excluded
        save(&mut h, "cd /tmp"); // excluded
        save(&mut h, "git status"); // kept
        assert_eq!(
            h.count(SearchQuery::everything(
                reedline::SearchDirection::Forward,
                None
            ))
            .unwrap(),
            1
        );
    }
}
