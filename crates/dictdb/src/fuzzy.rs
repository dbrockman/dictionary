//! Typo-tolerant matching: a Levenshtein automaton run over the key fst.
//!
//! The automaton accepts every key that starts with something within a small
//! edit distance of the query, so intersecting it with the fst visits only
//! the branches that can still match. Its cost depends on the query and the
//! distance, not on the size of the dictionary.

use std::sync::OnceLock;

use levenshtein_automata::{DFA, LevenshteinAutomatonBuilder, SINK_STATE};

/// Queries shorter than this (in characters) are never matched fuzzily; with
/// so few letters almost everything is one edit away.
const MIN_LEN: usize = 4;
/// Queries at least this long allow two edits instead of one.
const TWO_EDITS_LEN: usize = 8;

/// How closely the key of a fuzzy hit matches a misspelled query. Closer
/// matches order first: by `prefix_edits`, then by `word_edits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FuzzyMatch {
    /// Edits that turn the query into the start of the key.
    pub prefix_edits: u8,
    /// Edits that turn the query into the whole key, or one more than the
    /// allowed number when the key is farther away. Among keys that start
    /// with an equally close near miss, one that is a near miss as a whole
    /// (`house` for `huose`) ranks ahead of one that is not (`hosed`).
    pub word_edits: u8,
}

/// The number of edits allowed for `query` (already normalized): 0, 1 or 2.
fn max_distance(query: &str) -> u8 {
    match query.chars().count() {
        n if n < MIN_LEN => 0,
        n if n < TWO_EDITS_LEN => 1,
        _ => 2,
    }
}

/// An automaton matching keys that start with something within an edit or
/// two of the normalized `query`, or `None` if the query is too short to
/// match fuzzily. A transposition of two letters counts as one edit.
pub(crate) fn automaton(query: &str) -> Option<Fuzzy> {
    // Building a builder takes a few milliseconds, so each is built once, on
    // the first query that needs it.
    static BUILDERS: [OnceLock<LevenshteinAutomatonBuilder>; 2] =
        [OnceLock::new(), OnceLock::new()];
    let max = max_distance(query);
    if max == 0 {
        return None;
    }
    let builder =
        BUILDERS[max as usize - 1].get_or_init(|| LevenshteinAutomatonBuilder::new(max, true));
    Some(Fuzzy {
        prefix: builder.build_prefix_dfa(query),
        word: builder.build_dfa(query),
        max,
    })
}

pub(crate) struct Fuzzy {
    prefix: DFA,
    word: DFA,
    max: u8,
}

impl Fuzzy {
    /// How closely `key`, which the automaton accepted in `state`, matches.
    pub(crate) fn rank(&self, state: u32, key: &[u8]) -> FuzzyMatch {
        FuzzyMatch {
            word_edits: self.word.eval(key).to_u8().min(self.max + 1),
            prefix_edits: self.prefix.distance(state).to_u8(),
        }
    }
}

impl fst::Automaton for Fuzzy {
    type State = u32;

    fn start(&self) -> u32 {
        self.prefix.initial_state()
    }

    fn is_match(&self, state: &u32) -> bool {
        self.prefix.distance(*state).to_u8() <= self.max
    }

    fn can_match(&self, state: &u32) -> bool {
        *state != SINK_STATE
    }

    fn accept(&self, state: &u32, byte: u8) -> u32 {
        self.prefix.transition(*state, byte)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fst::Automaton;

    fn rank(query: &str, key: &str) -> Option<(u8, u8)> {
        let aut = automaton(query).unwrap();
        let state = key.bytes().fold(aut.start(), |s, b| aut.accept(&s, b));
        let m = aut.rank(state, key.as_bytes());
        aut.is_match(&state)
            .then_some((m.word_edits, m.prefix_edits))
    }

    #[test]
    fn allows_more_edits_for_longer_queries() {
        assert_eq!(max_distance("teh"), 0);
        assert!(automaton("teh").is_none());
        assert_eq!(max_distance("aple"), 1);
        assert_eq!(max_distance("recomend"), 2);
        assert_eq!(max_distance("ハロー"), 0);
    }

    #[test]
    fn matches_misspelled_words_and_prefixes() {
        assert_eq!(rank("recomend", "recommend"), Some((1, 1)));
        assert_eq!(rank("recomend", "recommendation"), Some((3, 1)));
        assert_eq!(rank("recomen", "recommend"), Some((2, 1)));
        // A transposition is one edit.
        assert_eq!(rank("huose", "house"), Some((1, 1)));
        assert_eq!(rank("huose", "hosed"), Some((2, 1)));
        assert_eq!(rank("aple", "apple"), Some((1, 1)));
        assert_eq!(rank("aple", "banana"), None);
    }
}
