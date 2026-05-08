//! Block-oriented streaming filter.
//!
//! Real subprocess hookup happens in `scry exec` (CLI layer). This module
//! provides the `StreamFilter` trait + `BlockStreamFilter` to apply a chain of
//! filters and `BlockHandler`s to a producer of lines.

/// One processing stage that may transform or drop a line.
pub trait StreamFilter: Send + Sync {
    /// Process one line. Return Some(transformed) to keep, None to drop.
    fn process_line<'a>(&self, line: &'a str) -> Option<std::borrow::Cow<'a, str>>;
}

/// Block-level handler. Receives groups of lines (e.g. a function definition
/// or a stack frame) and may decide whether to emit them, summarize them, or skip.
pub trait BlockHandler: Send + Sync {
    fn open_block(&mut self) {}
    fn handle_line(&mut self, line: &str);
    fn close_block(&mut self) -> Option<String> {
        None
    }
}

/// A line- and block-oriented stream filter applying a chain of transformers
/// and an optional block handler.
pub struct BlockStreamFilter {
    filters: Vec<Box<dyn StreamFilter>>,
    handler: Option<Box<dyn BlockHandler>>,
}

impl BlockStreamFilter {
    /// Empty filter — passthrough.
    #[must_use]
    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
            handler: None,
        }
    }

    /// Append a [`StreamFilter`] to the chain.
    #[must_use]
    pub fn with_filter(mut self, f: Box<dyn StreamFilter>) -> Self {
        self.filters.push(f);
        self
    }

    /// Set the [`BlockHandler`].
    #[must_use]
    pub fn with_handler(mut self, h: Box<dyn BlockHandler>) -> Self {
        self.handler = Some(h);
        self
    }

    /// Process all lines from `iter` and produce the joined output.
    pub fn run<I, S>(&mut self, iter: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = String::new();
        if let Some(handler) = self.handler.as_mut() {
            handler.open_block();
        }
        'outer: for line in iter {
            let line = line.as_ref();
            let mut current = std::borrow::Cow::Borrowed(line);
            for f in &self.filters {
                match f.process_line(&current) {
                    Some(next) => current = std::borrow::Cow::Owned(next.into_owned()),
                    None => continue 'outer,
                }
            }
            if let Some(handler) = self.handler.as_mut() {
                handler.handle_line(&current);
            }
            out.push_str(&current);
            if !current.ends_with('\n') {
                out.push('\n');
            }
        }
        if let Some(handler) = self.handler.as_mut() {
            if let Some(suffix) = handler.close_block() {
                out.push_str(&suffix);
            }
        }
        out
    }
}

impl Default for BlockStreamFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StripBlank;
    impl StreamFilter for StripBlank {
        fn process_line<'a>(&self, line: &'a str) -> Option<std::borrow::Cow<'a, str>> {
            if line.trim().is_empty() {
                None
            } else {
                Some(std::borrow::Cow::Borrowed(line))
            }
        }
    }

    struct UpperCase;
    impl StreamFilter for UpperCase {
        fn process_line<'a>(&self, line: &'a str) -> Option<std::borrow::Cow<'a, str>> {
            Some(std::borrow::Cow::Owned(line.to_uppercase()))
        }
    }

    struct CountLines(usize);
    impl BlockHandler for CountLines {
        fn handle_line(&mut self, _line: &str) {
            self.0 += 1;
        }
        fn close_block(&mut self) -> Option<String> {
            Some(format!("[{} lines]", self.0))
        }
    }

    #[test]
    fn chains_filters() {
        let mut bsf = BlockStreamFilter::new()
            .with_filter(Box::new(StripBlank))
            .with_filter(Box::new(UpperCase));
        let out = bsf.run(["alpha", "", "beta"].iter());
        assert!(out.contains("ALPHA"));
        assert!(out.contains("BETA"));
        assert!(!out.contains("alpha"));
    }

    #[test]
    fn block_handler_runs_close() {
        let mut bsf = BlockStreamFilter::new().with_handler(Box::new(CountLines(0)));
        let out = bsf.run(["one", "two", "three"].iter());
        assert!(out.contains("[3 lines]"));
    }
}
