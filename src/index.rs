//! Semantic-history indexing: embed shell-history commands into the on-disk
//! vector store ([`crate::semhist`]). Shared by `aishe history index` (the CLI)
//! and the opt-in auto-index on interactive-shell exit, so both go through one
//! tested path.

use std::path::Path;

use crate::config::Config;

/// Outcome of an [`reindex`] run.
pub struct Indexed {
    /// Commands newly embedded this run.
    pub added: usize,
    /// Total commands in the store afterwards (capped at [`crate::semhist::STORE_CAP`]).
    pub total: usize,
}

/// Why an index run produced nothing, distinct from an error.
pub enum Skip {
    /// No history-log commands to index yet.
    NoHistory,
    /// Everything is already embedded.
    UpToDate(usize),
}

/// Embed not-yet-indexed history commands (or all, with `rebuild`) from `hist`
/// into the vector store at `store`. Incremental by default: only commands not
/// already present are embedded, in batches. Returns `Ok(Ok(Indexed))` on work
/// done, `Ok(Err(Skip))` when there was nothing to do, and `Err` on an embedding
/// or write failure (partial progress is saved before an embedding error).
pub fn reindex(
    config: &Config,
    store: &Path,
    hist: &Path,
    rebuild: bool,
) -> Result<Result<Indexed, Skip>, String> {
    let candidates = crate::semhist::candidates(&crate::histlog::read(hist));
    if candidates.is_empty() {
        return Ok(Err(Skip::NoHistory));
    }

    // Incremental by default: skip commands already embedded. `rebuild` starts the
    // store fresh.
    let mut existing: Vec<crate::semhist::Entry> = if rebuild {
        Vec::new()
    } else {
        crate::semhist::load(store)
    };
    let already: std::collections::HashSet<String> =
        existing.iter().map(|e| e.cmd.clone()).collect();
    let todo: Vec<String> = candidates
        .into_iter()
        .filter(|c| !already.contains(c))
        .collect();
    if todo.is_empty() {
        return Ok(Err(Skip::UpToDate(existing.len())));
    }

    let provider = crate::providers::embedder(config).map_err(|e| e.to_string())?;
    let model = &config.aishe.embedding_model;
    let mut added = 0usize;
    for chunk in todo.chunks(128) {
        let batch: Vec<String> = chunk.to_vec();
        let vecs = match provider.embed(&batch, model) {
            Ok(v) => v,
            Err(e) => {
                // Persist what we managed before the failure so progress isn't lost.
                if added > 0 {
                    let _ = crate::semhist::save(store, &existing);
                }
                return Err(format!("embedding failed: {e}"));
            }
        };
        for (cmd, vec) in batch.into_iter().zip(vecs) {
            existing.push(crate::semhist::Entry { cmd, vec });
            added += 1;
        }
    }
    crate::semhist::save(store, &existing)
        .map_err(|e| format!("writing {}: {e}", store.display()))?;
    let total = existing.len().min(crate::semhist::STORE_CAP);
    Ok(Ok(Indexed { added, total }))
}

// The end-to-end behavior (embed → store → search, incremental vs up-to-date, the
// no-history case) is covered by `tests/semhist.rs` against the built binary,
// which activates the fake embedder via a *subprocess* env var — avoiding the
// process-global `set_var` that would race parallel unit tests.
