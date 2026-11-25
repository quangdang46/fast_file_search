/*!
A collection of routines for performing operations on lines.
*/

use bstr::ByteSlice;

use crate::matcher::{LineTerminator, Match};

/// An explicit iterator over lines in a particular slice of bytes.
///
/// This iterator avoids borrowing the bytes themselves, and instead requires
/// callers to explicitly provide the bytes when moving through the iterator.
///
/// Line terminators are considered part of the line they terminate. All lines
/// yielded by the iterator are guaranteed to be non-empty.
#[derive(Debug)]
pub struct LineStep {
    line_term: u8,
    pos: usize,
    end: usize,
}

impl LineStep {
    /// Create a new line iterator over the given range of bytes using the
    /// given line terminator.
    pub fn new(line_term: u8, start: usize, end: usize) -> LineStep {
        LineStep {
            line_term,
            pos: start,
            end,
        }
    }

    /// Like next, but returns a `Match` instead of a tuple.
    #[inline(always)]
    pub fn next_match(&mut self, bytes: &[u8]) -> Option<Match> {
        self.next_impl(bytes).map(|(s, e)| Match::new(s, e))
    }

    #[inline(always)]
    fn next_impl(&mut self, bytes: &[u8]) -> Option<(usize, usize)> {
        // Clamp out-of-contract ranges instead of panicking: `end`/`pos`
        // beyond the slice just mean "no more lines".
        let end = self.end.min(bytes.len());
        let bytes = &bytes[..end];
        let pos = self.pos.min(bytes.len());
        match bytes[pos..].find_byte(self.line_term) {
            None => {
                if pos < bytes.len() {
                    let m = (pos, bytes.len());
                    assert!(m.0 <= m.1);

                    self.pos = m.1;
                    Some(m)
                } else {
                    None
                }
            }
            Some(line_end) => {
                let m = (pos, pos + line_end + 1);
                assert!(m.0 <= m.1);

                self.pos = m.1;
                Some(m)
            }
        }
    }
}

/// Count the number of occurrences of `line_term` in `bytes`.
pub fn count(bytes: &[u8], line_term: u8) -> u64 {
    memchr::memchr_iter(line_term, bytes).count() as u64
}

/// Given a line that possibly ends with a terminator, return that line without
/// the terminator.
#[inline(always)]
pub fn without_terminator(bytes: &[u8], line_term: LineTerminator) -> &[u8] {
    let line_term = line_term.as_bytes();
    let start = bytes.len().saturating_sub(line_term.len());
    if bytes.get(start..) == Some(line_term) {
        return &bytes[..bytes.len() - line_term.len()];
    }
    bytes
}

/// Return the start and end offsets of the lines containing the given range
/// of bytes.
///
/// Line terminators are considered part of the line they terminate.
#[inline(always)]
pub fn locate(bytes: &[u8], line_term: u8, range: Match) -> Match {
    // Clamp out-of-contract ranges to the slice instead of panicking.
    let start = range.start().min(bytes.len());
    let end = range.end().min(bytes.len()).max(start);
    let line_start = bytes[..start].rfind_byte(line_term).map_or(0, |i| i + 1);
    let line_end = if end > line_start && bytes[end - 1] == line_term {
        end
    } else {
        bytes[end..]
            .find_byte(line_term)
            .map_or(bytes.len(), |i| end + i + 1)
    };
    Match::new(line_start, line_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHERLOCK: &str = "\
For the Doctor Watsons of this world, as opposed to the Sherlock
Holmeses, success in the province of detective work must always
be, to a very large extent, the result of luck. Sherlock Holmes
can extract a clew from a wisp of straw or a flake of cigar ash;
but Doctor Watson has to have it taken out for him and dusted,
and exhibited clearly, with a label attached.\
";

    fn m(start: usize, end: usize) -> Match {
        Match::new(start, end)
    }

    fn lines(text: &str) -> Vec<&str> {
        let mut results = vec![];
        let mut it = LineStep::new(b'\n', 0, text.len());
        while let Some(m) = it.next_match(text.as_bytes()) {
            results.push(&text[m]);
        }
        results
    }

    fn line_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
        let mut results = vec![];
        let mut it = LineStep::new(b'\n', 0, text.len());
        while let Some(m) = it.next_match(text.as_bytes()) {
            results.push(m.start()..m.end());
        }
        results
    }

    fn loc(text: &str, start: usize, end: usize) -> Match {
        locate(text.as_bytes(), b'\n', Match::new(start, end))
    }

    #[test]
    fn line_count() {
        assert_eq!(0, count(b"", b'\n'));
        assert_eq!(1, count(b"\n", b'\n'));
        assert_eq!(2, count(b"\n\n", b'\n'));
        assert_eq!(2, count(b"a\nb\nc", b'\n'));
    }

    #[test]
    fn line_locate() {
        let t = SHERLOCK;
        let lines = line_ranges(t);

        assert_eq!(
            loc(t, lines[0].start, lines[0].end),
            m(lines[0].start, lines[0].end)
        );
        assert_eq!(
            loc(t, lines[0].start + 1, lines[0].end),
            m(lines[0].start, lines[0].end)
        );
        assert_eq!(
            loc(t, lines[0].end - 1, lines[0].end),
            m(lines[0].start, lines[0].end)
        );
        assert_eq!(
            loc(t, lines[0].end, lines[0].end),
            m(lines[1].start, lines[1].end)
        );

        assert_eq!(
            loc(t, lines[5].start, lines[5].end),
            m(lines[5].start, lines[5].end)
        );
        assert_eq!(
            loc(t, lines[5].start + 1, lines[5].end),
            m(lines[5].start, lines[5].end)
        );
        assert_eq!(
            loc(t, lines[5].end - 1, lines[5].end),
            m(lines[5].start, lines[5].end)
        );
        assert_eq!(
            loc(t, lines[5].end, lines[5].end),
            m(lines[5].start, lines[5].end)
        );
    }

    #[test]
    fn line_locate_weird() {
        assert_eq!(loc("", 0, 0), m(0, 0));

        assert_eq!(loc("\n", 0, 1), m(0, 1));
        assert_eq!(loc("\n", 1, 1), m(1, 1));

        assert_eq!(loc("\n\n", 0, 0), m(0, 1));
        assert_eq!(loc("\n\n", 0, 1), m(0, 1));
        assert_eq!(loc("\n\n", 1, 1), m(1, 2));
        assert_eq!(loc("\n\n", 1, 2), m(1, 2));
        assert_eq!(loc("\n\n", 2, 2), m(2, 2));

        assert_eq!(loc("a\nb\nc", 0, 1), m(0, 2));
        assert_eq!(loc("a\nb\nc", 1, 2), m(0, 2));
        assert_eq!(loc("a\nb\nc", 2, 3), m(2, 4));
        assert_eq!(loc("a\nb\nc", 3, 4), m(2, 4));
        assert_eq!(loc("a\nb\nc", 4, 5), m(4, 5));
        assert_eq!(loc("a\nb\nc", 5, 5), m(4, 5));
    }

    #[test]
    fn line_iter() {
        assert_eq!(lines("abc"), vec!["abc"]);

        assert_eq!(lines("abc\n"), vec!["abc\n"]);
        assert_eq!(lines("abc\nxyz"), vec!["abc\n", "xyz"]);
        assert_eq!(lines("abc\nxyz\n"), vec!["abc\n", "xyz\n"]);

        assert_eq!(lines("abc\n\n"), vec!["abc\n", "\n"]);
        assert_eq!(lines("abc\n\n\n"), vec!["abc\n", "\n", "\n"]);
        assert_eq!(lines("abc\n\nxyz"), vec!["abc\n", "\n", "xyz"]);
        assert_eq!(lines("abc\n\nxyz\n"), vec!["abc\n", "\n", "xyz\n"]);
        assert_eq!(lines("abc\nxyz\n\n"), vec!["abc\n", "xyz\n", "\n"]);

        assert_eq!(lines("\n"), vec!["\n"]);
        assert_eq!(lines(""), Vec::<&str>::new());
    }

    #[test]
    fn line_iter_empty() {
        let mut it = LineStep::new(b'\n', 0, 0);
        assert_eq!(it.next_match(b"abc"), None);
    }

    #[test]
    fn line_step_clamps_out_of_contract_range() {
        // `end` beyond the slice and `pos` beyond `end` must not panic.
        let mut it = LineStep::new(b'\n', 0, 100);
        assert_eq!(it.next_match(b"abc"), Some(m(0, 3)));
        assert_eq!(it.next_match(b"abc"), None);

        let mut it = LineStep::new(b'\n', 50, 10);
        assert_eq!(it.next_match(b"abc\n"), None);
    }

    #[test]
    fn locate_clamps_out_of_contract_range() {
        assert_eq!(loc("a\nb\n", 10, 20), m(4, 4));
        assert_eq!(loc("a\nb", 5, 9), m(2, 3));
    }
}
