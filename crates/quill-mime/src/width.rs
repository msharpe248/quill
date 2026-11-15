//! How many terminal cells text takes.
//!
//! Wrapping must never produce a row wider than the pane, so every doubt is
//! settled on the wide side: a character this table calls wide may be drawn
//! narrow, never the other way round. A row can then come out a cell or two
//! short, and never too long.

/// The cells `c` takes: none for a mark that joins the character before it,
/// two for East Asian wide characters and emoji, otherwise one.
pub(crate) fn of_char(c: char) -> usize {
    if joins(c) {
        return 0;
    }
    if is_wide(c) { 2 } else { 1 }
}

/// The cells `text` takes.
pub(crate) fn of_str(text: &str) -> usize {
    // The emoji variation selector turns a narrow symbol into a wide one,
    // so it is counted as the cell it adds.
    text.chars()
        .map(|c| if c == EMOJI_SELECTOR { 1 } else { of_char(c) })
        .sum()
}

const JOINER: char = '\u{200d}';
const EMOJI_SELECTOR: char = '\u{fe0f}';

/// Characters drawn in the cell of the character before them: combining
/// marks, joiners, and the invisible marks that take no cell.
fn joins(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036f}'
            | '\u{0483}'..='\u{0489}'
            | '\u{0591}'..='\u{05bd}'
            | '\u{0610}'..='\u{061a}'
            | '\u{064b}'..='\u{065f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2060}'..='\u{2064}'
            | '\u{20d0}'..='\u{20f0}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{fe20}'..='\u{fe2f}'
            | '\u{feff}'
            | '\u{e0000}'..='\u{e0fff}'
    )
}

/// Skin tones follow the emoji they colour and are drawn in its cells.
fn is_emoji_modifier(c: char) -> bool {
    matches!(c, '\u{1f3fb}'..='\u{1f3ff}')
}

/// The two letters of a flag.
fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1f1e6}'..='\u{1f1ff}')
}

fn is_wide(c: char) -> bool {
    matches!(
        c,
        '\u{1100}'..='\u{115f}'
            | '\u{231a}'..='\u{231b}'
            | '\u{2329}'..='\u{232a}'
            | '\u{23e9}'..='\u{23f3}'
            | '\u{25fd}'..='\u{25fe}'
            | '\u{2614}'..='\u{2615}'
            | '\u{2630}'..='\u{2637}'
            | '\u{2648}'..='\u{2653}'
            | '\u{267f}'
            | '\u{268a}'..='\u{268f}'
            | '\u{2693}'
            | '\u{26a1}'
            | '\u{26aa}'..='\u{26ab}'
            | '\u{26bd}'..='\u{26be}'
            | '\u{26c4}'..='\u{26c5}'
            | '\u{26ce}'
            | '\u{26d4}'
            | '\u{26ea}'
            | '\u{26f2}'..='\u{26f5}'
            | '\u{26fa}'
            | '\u{26fd}'
            | '\u{2705}'
            | '\u{270a}'..='\u{270b}'
            | '\u{2728}'
            | '\u{274c}'
            | '\u{274e}'
            | '\u{2753}'..='\u{2757}'
            | '\u{2795}'..='\u{2797}'
            | '\u{27b0}'
            | '\u{27bf}'
            | '\u{2b1b}'..='\u{2b1c}'
            | '\u{2b50}'
            | '\u{2b55}'
            | '\u{2e80}'..='\u{303e}'
            | '\u{3041}'..='\u{a4cf}'
            | '\u{a960}'..='\u{a97f}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{f900}'..='\u{faff}'
            | '\u{fe10}'..='\u{fe19}'
            | '\u{fe30}'..='\u{fe6f}'
            | '\u{ff01}'..='\u{ff60}'
            | '\u{ffe0}'..='\u{ffe6}'
            | '\u{16fe0}'..='\u{18d7f}'
            | '\u{1aff0}'..='\u{1b2ff}'
            | '\u{1d300}'..='\u{1d37f}'
            | '\u{1f000}'..='\u{1faff}'
            | '\u{20000}'..='\u{3fffd}'
    )
}

/// One thing a reader sees as a single character: a letter with its
/// accents, an emoji with its skin tone, a family joined into one picture,
/// a flag. A row is never broken inside one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cluster {
    /// Where it starts in the text, in bytes.
    pub start: usize,
    /// Where it ends.
    pub end: usize,
    pub width: usize,
}

/// The clusters of `text`, in order.
pub(crate) fn clusters(text: &str) -> Clusters<'_> {
    Clusters {
        text,
        chars: text.char_indices().peekable(),
    }
}

pub(crate) struct Clusters<'a> {
    text: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
}

impl Iterator for Clusters<'_> {
    type Item = Cluster;

    fn next(&mut self) -> Option<Cluster> {
        let (start, first) = self.chars.next()?;
        let mut width = of_char(first);
        let mut flag_half = is_regional_indicator(first);
        while let Some(&(_, next)) = self.chars.peek() {
            if next == JOINER {
                // The joiner and what it joins on.
                self.chars.next();
                if let Some((_, joined)) = self.chars.next() {
                    width += of_char(joined);
                }
            } else if next == EMOJI_SELECTOR {
                width += 1;
                self.chars.next();
            } else if joins(next) {
                self.chars.next();
            } else if is_emoji_modifier(next) || (flag_half && is_regional_indicator(next)) {
                flag_half = false;
                width += of_char(next);
                self.chars.next();
            } else {
                break;
            }
        }
        let end = self.chars.peek().map_or(self.text.len(), |(at, _)| *at);
        Some(Cluster { start, end, width })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_of_ordinary_text() {
        assert_eq!(of_str(""), 0);
        assert_eq!(of_str("agenda"), 6);
        assert_eq!(of_str("Tomás"), 5);
        // Decomposed: the accent takes no cell of its own.
        assert_eq!(of_str("Toma\u{301}s"), 5);
        assert_eq!(of_str("陈静"), 4);
        assert_eq!(of_str("ｆｕｌｌ"), 8);
        assert_eq!(of_str("한국어"), 6);
    }

    #[test]
    fn emoji_are_never_counted_narrow() {
        assert_eq!(of_str("☕"), 2);
        assert_eq!(of_str("🎉"), 2);
        // A heart made wide by its selector.
        assert_eq!(of_str("❤\u{fe0f}"), 2);
        // Erring wide: a family is one picture two cells wide, counted as
        // three people. A flag is counted as its two letters.
        assert!(of_str("👨\u{200d}👩\u{200d}👧") >= 2);
        assert!(of_str("🇪🇸") >= 2);
        assert!(of_str("👍🏽") >= 2);
    }

    #[test]
    fn clusters_keep_what_belongs_together() {
        let pieces = |text: &str| -> Vec<String> {
            clusters(text)
                .map(|cluster| text[cluster.start..cluster.end].to_owned())
                .collect()
        };
        assert_eq!(pieces("abc"), ["a", "b", "c"]);
        assert_eq!(pieces("a\u{301}b"), ["a\u{301}", "b"]);
        assert_eq!(
            pieces("x👨\u{200d}👩\u{200d}👧y"),
            ["x", "👨\u{200d}👩\u{200d}👧", "y"]
        );
        assert_eq!(pieces("👍🏽!"), ["👍🏽", "!"]);
        assert_eq!(pieces("🇪🇸🇫🇷"), ["🇪🇸", "🇫🇷"]);
        assert_eq!(pieces("❤\u{fe0f}a"), ["❤\u{fe0f}", "a"]);
        // Marks with nothing before them, and a joiner at the very end.
        assert_eq!(pieces("\u{301}a\u{200d}"), ["\u{301}", "a\u{200d}"]);
        assert_eq!(pieces(""), Vec::<String>::new());
    }

    #[test]
    fn a_cluster_is_as_wide_as_its_text() {
        for text in ["a\u{301}", "❤\u{fe0f}", "👍🏽", "🇪🇸", "👨\u{200d}👩", "陈"] {
            let all: Vec<Cluster> = clusters(text).collect();
            assert_eq!(all.len(), 1, "{text:?}");
            assert_eq!(all[0].width, of_str(text), "{text:?}");
            assert_eq!((all[0].start, all[0].end), (0, text.len()));
        }
    }
}
