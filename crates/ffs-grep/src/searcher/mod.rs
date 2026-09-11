use crate::{
    matcher::{LineTerminator, Match, Matcher},
    searcher::glue::{MultiLine, SliceByLine},
    sink::{Sink, SinkError},
};

mod core;
mod glue;

/// We use this type alias since we want the ergonomics of a matcher's `Match`
/// type, but in practice, we use it for arbitrary ranges, so give it a more
/// accurate name. This is only used in the searcher's internals.
type Range = Match;

/// An error that can occur when building a searcher.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub(crate) enum ConfigError {
    /// Occurs when a matcher reports a line terminator that is different than
    /// the one configured in the searcher.
    MismatchedLineTerminators {
        /// The matcher's line terminator.
        matcher: LineTerminator,
        /// The searcher's line terminator.
        searcher: LineTerminator,
    },
}

impl std::error::Error for ConfigError {}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            ConfigError::MismatchedLineTerminators { matcher, searcher } => {
                write!(
                    f,
                    "grep config error: mismatched line terminators, \
                     matcher has {:?} but searcher has {:?}",
                    matcher, searcher
                )
            }
        }
    }
}

/// The internal configuration of a searcher.
#[derive(Clone, Debug)]
pub(crate) struct Config {
    /// The line terminator to use.
    pub(crate) line_term: LineTerminator,
    /// Whether to count line numbers.
    pub(crate) line_number: bool,
    /// Whether to enable matching across multiple lines.
    multi_line: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            line_term: LineTerminator::default(),
            line_number: true,
            multi_line: false,
        }
    }
}

/// A builder for configuring a searcher.
#[derive(Clone, Debug)]
pub struct SearcherBuilder {
    config: Config,
}

impl Default for SearcherBuilder {
    fn default() -> SearcherBuilder {
        SearcherBuilder::new()
    }
}

impl SearcherBuilder {
    /// Create a new searcher builder with a default configuration.
    pub fn new() -> SearcherBuilder {
        SearcherBuilder {
            config: Config::default(),
        }
    }

    /// Build a searcher.
    pub fn build(&self) -> Searcher {
        Searcher {
            config: self.config.clone(),
        }
    }

    /// Whether to count and include line numbers with matching lines.
    pub fn line_number(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.line_number = yes;
        self
    }

    /// Whether to enable multi line search or not.
    pub fn multi_line(&mut self, yes: bool) -> &mut SearcherBuilder {
        self.config.multi_line = yes;
        self
    }
}

/// A searcher executes searches over a haystack and writes results to a caller
/// provided sink.
#[derive(Clone, Debug)]
pub struct Searcher {
    pub(crate) config: Config,
}

impl Searcher {
    /// Create a new searcher with a default configuration.
    pub fn new() -> Searcher {
        SearcherBuilder::new().build()
    }

    /// Execute a search over the given slice and write the results to the
    /// given sink.
    pub fn search_slice<M, S>(&self, matcher: M, slice: &[u8], write_to: S) -> Result<(), S::Error>
    where
        M: Matcher,
        S: Sink,
    {
        self.check_config(&matcher)
            .map_err(S::Error::error_message)?;

        if self.multi_line_with_matcher(&matcher) {
            MultiLine::new(self, matcher, slice, write_to).run()
        } else {
            SliceByLine::new(self, matcher, slice, write_to).run()
        }
    }

    /// Check that the searcher's configuration and the matcher are consistent.
    fn check_config<M: Matcher>(&self, matcher: M) -> Result<(), ConfigError> {
        let matcher_line_term = match matcher.line_terminator() {
            None => return Ok(()),
            Some(line_term) => line_term,
        };
        if matcher_line_term != self.config.line_term {
            return Err(ConfigError::MismatchedLineTerminators {
                matcher: matcher_line_term,
                searcher: self.config.line_term,
            });
        }
        Ok(())
    }
}

impl Default for Searcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        matcher::{Match, Matcher, NoError},
        sink::{Sink, SinkFinish, SinkMatch},
    };

    /// Literal matcher recording the `at` values it is called with, so tests
    /// can assert the searcher threads absolute positions (no re-slicing).
    struct AtRecordingMatcher {
        needle: Vec<u8>,
        calls: std::cell::RefCell<Vec<usize>>,
    }

    impl Matcher for AtRecordingMatcher {
        type Error = NoError;

        fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
            self.calls.borrow_mut().push(at);
            let rel = memchr::memmem::find(&haystack[at..], &self.needle);
            Ok(rel.map(|pos| Match::new(at + pos, at + pos + self.needle.len())))
        }

        fn line_terminator(&self) -> Option<crate::matcher::LineTerminator> {
            Some(crate::matcher::LineTerminator::byte(b'\n'))
        }
    }

    struct CollectingSink {
        lines: Vec<(u64, String)>,
    }

    impl Sink for CollectingSink {
        type Error = std::io::Error;

        fn matched(
            &mut self,
            _searcher: &Searcher,
            mat: &SinkMatch<'_>,
        ) -> Result<bool, Self::Error> {
            let n = mat.line_number().unwrap_or(0);
            let text = String::from_utf8_lossy(mat.bytes()).into_owned();
            self.lines.push((n, text));
            Ok(true)
        }

        fn finish(&mut self, _: &Searcher, _: &SinkFinish) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn find_by_line_threads_absolute_positions() {
        let hay = b"aaa\nneedle one\nbbb\nneedle two\n";
        let matcher = AtRecordingMatcher {
            needle: b"needle".to_vec(),
            calls: std::cell::RefCell::new(Vec::new()),
        };
        let searcher = SearcherBuilder::new().line_number(true).build();
        let mut sink = CollectingSink { lines: Vec::new() };
        searcher.search_slice(&matcher, hay, &mut sink).unwrap();

        assert_eq!(sink.lines.len(), 2);
        assert_eq!(sink.lines[0].0, 2);
        assert_eq!(sink.lines[1].0, 4);
        // Absolute positions strictly increase: no restart-from-zero re-slice.
        let calls = matcher.calls.borrow();
        assert!(calls.len() >= 2);
        for w in calls.windows(2) {
            assert!(
                w[1] > w[0],
                "matcher called with non-increasing at: {calls:?}"
            );
        }
    }

    #[test]
    fn multiline_match_finds_across_lines() {
        struct SubMatcher;
        impl Matcher for SubMatcher {
            type Error = NoError;
            fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, NoError> {
                memchr::memmem::find(&haystack[at..], b"one\nbbb")
                    .map(|pos| Match::new(at + pos, at + pos + 7))
                    .pipe(Ok)
            }
        }
        let hay = b"needle one\nbbb\ntail\n";
        let searcher = SearcherBuilder::new()
            .line_number(true)
            .multi_line(true)
            .build();
        let mut sink = CollectingSink { lines: Vec::new() };
        searcher.search_slice(SubMatcher, hay, &mut sink).unwrap();
        assert_eq!(sink.lines.len(), 1);
    }

    trait Pipe: Sized {
        fn pipe<F, T>(self, f: F) -> T
        where
            F: FnOnce(Self) -> T,
        {
            f(self)
        }
    }
    impl<T> Pipe for T {}
}

/// Configuration query methods used by the sink and internal search core.
impl Searcher {
    /// Returns the line terminator used by this searcher.
    #[inline]
    pub fn line_terminator(&self) -> LineTerminator {
        self.config.line_term
    }

    /// Returns true if and only if this searcher is configured to count line
    /// numbers.
    #[inline]
    pub fn line_number(&self) -> bool {
        self.config.line_number
    }

    /// Returns true if and only if this searcher is configured to perform
    /// multi line search.
    #[inline]
    pub fn multi_line(&self) -> bool {
        self.config.multi_line
    }

    /// Returns true if and only if this searcher will choose a multi-line
    /// strategy given the provided matcher.
    pub fn multi_line_with_matcher<M: Matcher>(&self, matcher: M) -> bool {
        if !self.multi_line() {
            return false;
        }
        if let Some(line_term) = matcher.line_terminator()
            && line_term == self.line_terminator()
        {
            return false;
        }
        true
    }
}
