//! One frame, two renderers.
//!
//! # Why the frame is a value and not a string
//!
//! Because there are two front ends and they must not drift. If each rendered
//! its own text, the plain transcript and the terminal display would be two
//! descriptions of the flow rather than two views of it, and the acceptance
//! oracle that compares a headless run with a keyboard run would be comparing
//! nothing. So the state machine produces a [`Frame`] -- rows of styled spans,
//! already laid out by [`crate::tui::layout`] -- and the renderers only decide
//! how a span looks.
//!
//! # Why a span carries a style rather than an escape sequence
//!
//! Because the headless renderer has no colour at all, and the layout must be
//! readable without one. A span says what a piece of text *is* -- structure, a
//! label, a value, the thing being worked on -- and the terminal renderer
//! decides that structure and labels are dim and the current thing is bold.
//! Strip the styling and the frame still reads, because the alignment and the
//! box are doing the work. That is checkable, and the transcript checks it on
//! every run.
//!
//! # The four styles, and why there are only four
//!
//! `Frame` for the box and the pane titles, `Label` for the words naming
//! something, `Value` for the thing named, `Strong` for the step being worked
//! and the field the cursor is on. A fifth would have to mean something a
//! reader could name, and nothing here does.

/// What a piece of text is, which is what decides how it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// The box, the dividers and the pane titles.
    Frame,
    /// A word naming something: `state:`, `value`, a key name.
    Label,
    /// The thing named.
    Value,
    /// The step being worked, and the field the cursor is on.
    Strong,
}

/// One run of text with one style.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// One line of a frame.
pub type Row = Vec<Span>;

pub fn frame(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Style::Frame,
    }
}
pub fn label(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Style::Label,
    }
}
pub fn value(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Style::Value,
    }
}
pub fn strong(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Style::Strong,
    }
}

/// The display width of a piece of text, in the columns it occupies.
///
/// Counted in `char`s, which is right for everything this front end draws:
/// box-drawing characters, the focus marker and every letter of every label
/// are one column each, and nothing here is allowed to be wider.
pub fn cells(text: &str) -> usize {
    text.chars().count()
}

pub fn row_width(row: &Row) -> usize {
    row.iter().map(|span| cells(&span.text)).sum()
}

/// One rendering of the session: the rectangle, and what a grep needs.
pub struct Frame {
    pub step: &'static str,
    pub state: &'static str,
    /// The frame proper: one row per terminal line, every row the same width.
    pub rows: Vec<Row>,
    /// Diagnostics repeated below the drawing, one per line and unwrapped.
    ///
    /// A remedy inside a pane is wrapped to the pane, which is right for a
    /// person and useless to a script: the sentence it needs is no longer on
    /// any single line. So the transcript carries the diagnostic once more
    /// underneath, whole. The terminal renderer does not, because a person is
    /// looking at the pane, where the same words already are.
    pub trailer: Vec<String>,
}

impl Frame {
    /// The transcript's frame header.
    pub fn header(&self, number: usize) -> String {
        format!(
            "--- frame {number} (step: {}, state: {}) ---",
            self.step, self.state
        )
    }

    /// The plain-text rendering: what the headless front end appends to the
    /// transcript. No escape sequence, and no trailing whitespace trimmed --
    /// the padding is the layout, and a trimmed row would not close.
    pub fn plain(&self) -> String {
        let mut out = String::new();
        for row in &self.rows {
            for span in row {
                out.push_str(&span.text);
            }
            out.push('\n');
        }
        for line in &self.trailer {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// The terminal rendering: the background painted, the screen cleared,
    /// the cursor sent home, and each span in the one attribute its style
    /// asks for. The frame ends by putting the terminal back the way it was
    /// found.
    ///
    /// # Why every row is placed by address and no row ends in a line break
    ///
    /// The frame is exactly as tall as the terminal it is drawn for. A line
    /// break after the last row on a terminal of exactly that height is a
    /// scroll: the top border goes up a row and the next frame is drawn one
    /// row low, and it was measured doing exactly that on a live 80x24 pane.
    /// So no row is followed by anything; each begins with `ESC[row;1H`,
    /// which puts the cursor where the row goes whatever came before it, and
    /// the cursor is left at the end of the last row, inside the frame.
    pub fn ansi(&self) -> String {
        let mut out = String::from(RESET);
        // Painted before the erase, so the erase fills the screen with it
        // rather than with whatever the terminal was set to.
        out.push_str(BACKGROUND);
        out.push_str("\u{1b}[2J\u{1b}[H");
        for (index, row) in self.rows.iter().enumerate() {
            out.push_str(&position(index + 1));
            // Restated at the start of every row rather than tracked across
            // rows: a frame is written into a terminal whose state nobody
            // here controls, and a row that assumed the last one's attributes
            // would be the row that goes wrong after a resize.
            let mut current: Option<Style> = None;
            for span in row {
                if current != Some(span.style) {
                    out.push_str(enter(span.style));
                    current = Some(span.style);
                }
                out.push_str(&span.text);
            }
        }
        out.push_str(RESET);
        out
    }
}

/// The cursor to the first column of `row`, counted from one.
fn position(row: usize) -> String {
    format!("\u{1b}[{row};1H")
}

/// Whether an escape sequence is one of [`position`]'s.
#[cfg(test)]
fn is_position(sequence: &str) -> bool {
    sequence
        .strip_prefix("\u{1b}[")
        .and_then(|rest| rest.strip_suffix(";1H"))
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
}

const RESET: &str = "\u{1b}[0m";

/// The product's own colour, #136207, as a true-colour background.
///
/// Painted rather than assumed: a terminal Catalyst has never seen is not
/// configured for it, and a front end that only looks right on the machine it
/// was built on is not a front end. It is the one colour in the whole
/// program, and everything on top of it is dim, normal or bold -- the frame
/// has to read with the colour stripped, which is exactly what the headless
/// transcript checks on every run.
pub const BACKGROUND: &str = "\u{1b}[48;2;19;98;7m";

/// The two foregrounds, chosen by contrast against that green rather than by
/// taste.
///
/// #136207 has a relative luminance of about 0.087, which is dark, so the text
/// on it is light. `TEXT` is #F2F7F0 and reaches about 7.1:1 against the
/// background -- comfortably past the 4.5:1 that ordinary body text is held
/// to. `QUIET` is #A8C6A0, about 4.1:1: enough to read a label by, visibly
/// behind the value beside it. The box itself takes `QUIET` with the dim
/// attribute on top, because structure should be present without competing.
///
/// Two tints of one hue and two attributes is the whole palette. Anything a
/// reader could mistake for *meaning* -- a red, a second hue, a fill -- would
/// have to be explained, and none of it would survive the headless transcript,
/// which carries no colour at all and is the copy every oracle reads.
/// Public because the operator chose the palette and it is part of what the
/// front end is, not an implementation detail of how it draws.
pub const TEXT: &str = "\u{1b}[38;2;242;247;240m";
pub const QUIET: &str = "\u{1b}[38;2;168;198;160m";

/// The paint, the ink and the weight for one style, in one sequence.
///
/// Every change re-asserts the background and the foreground, because the
/// reset that clears the last attribute would otherwise clear the paint with
/// it and leave the rest of the row in whatever the terminal defaults to.
fn enter(style: Style) -> &'static str {
    match style {
        Style::Frame => concat!(
            "\u{1b}[0m",
            "\u{1b}[48;2;19;98;7m",
            "\u{1b}[38;2;168;198;160m",
            "\u{1b}[2m"
        ),
        Style::Label => concat!(
            "\u{1b}[0m",
            "\u{1b}[48;2;19;98;7m",
            "\u{1b}[38;2;168;198;160m"
        ),
        Style::Value => concat!(
            "\u{1b}[0m",
            "\u{1b}[48;2;19;98;7m",
            "\u{1b}[38;2;242;247;240m"
        ),
        Style::Strong => concat!(
            "\u{1b}[0m",
            "\u{1b}[48;2;19;98;7m",
            "\u{1b}[38;2;242;247;240m",
            "\u{1b}[1m"
        ),
    }
}

/// The whole flow, in order. Every frame carries it verbatim.
pub const TRAIL: &str = "Goal > Inputs > Run > Results > Export";

/// The bracketed marker line that goes under the trail, aligned with the step
/// it points at.
pub fn marker(title: &str) -> String {
    let at = TRAIL.find(title).unwrap_or(0);
    format!("{}[{title}]", " ".repeat(at.saturating_sub(1)))
}

/// Wrap prose to a width, breaking between words where it can and inside a
/// word only when the word alone is wider than the pane.
///
/// Used for the frame's own sentences. A value the operator typed goes through
/// [`fit_value`] instead, which is a different promise.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            cells(word)
        } else {
            cells(&line) + 1 + cells(word)
        };
        if candidate <= width {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        // A single word wider than the pane is broken, because the alternative
        // is a row that does not close.
        let mut rest: Vec<char> = word.chars().collect();
        while rest.len() > width {
            lines.push(rest[..width].iter().collect());
            rest.drain(..width);
        }
        line = rest.into_iter().collect();
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Lay a value out across as many rows as it needs, whole.
///
/// A value the operator typed is never shortened and never has a marker put
/// through it: it appears exactly as typed. When it is longer than the pane
/// there is nowhere to put it in one piece, so it continues on the next row,
/// indented, and the pieces still read as the one string that was entered.
pub fn fit_value(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if cells(text) <= width {
        return vec![text.to_owned()];
    }
    let indent = if width > 4 { 2 } else { 0 };
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let room = if out.is_empty() {
            width
        } else {
            width - indent
        };
        let take = room.min(chars.len() - at);
        let mut line = " ".repeat(if out.is_empty() { 0 } else { indent });
        line.extend(&chars[at..at + take]);
        out.push(line);
        at += take;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Row> {
        vec![
            vec![frame("┌──┐")],
            vec![
                frame("│ "),
                label("state: "),
                value("proposed"),
                frame(" │"),
            ],
            vec![frame("└──┘")],
        ]
    }

    fn a_frame() -> Frame {
        Frame {
            step: "inputs",
            state: "proposed",
            rows: rows(),
            trailer: Vec::new(),
        }
    }

    #[test]
    fn the_plain_rendering_is_the_spans_and_nothing_else() {
        let plain = a_frame().plain();
        assert!(plain.contains("│ state: proposed │"), "{plain}");
        assert!(!plain.contains('\u{1b}'));
    }

    /// Every escape sequence the terminal renderer writes, in the order the
    /// output uses them.
    fn sequences(ansi: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = ansi;
        while let Some(at) = rest.find('\u{1b}') {
            let tail = &rest[at..];
            let end = tail
                .char_indices()
                .find(|(i, c)| *i > 1 && c.is_ascii_alphabetic())
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(tail.len());
            out.push(tail[..end].to_owned());
            rest = &tail[end..];
        }
        out
    }

    /// The operator's green, two foregrounds picked for contrast against it,
    /// and two attributes. Nothing else reaches the terminal -- no palette
    /// index, no second hue, nothing that cycles.
    #[test]
    fn the_terminal_renderer_paints_one_colour_and_writes_nothing_else() {
        let ansi = a_frame().ansi();
        assert!(
            ansi.contains(BACKGROUND),
            "the operator's #136207 is painted"
        );
        assert!(
            ansi.contains("\u{1b}[2J"),
            "and the erase fills the screen with it"
        );
        assert!(ansi.ends_with(RESET), "the terminal is put back");
        assert!(ansi.contains(TEXT), "values are legible light text");
        assert!(ansi.contains(QUIET), "labels are a quieter tint of it");
        assert!(ansi.contains("\u{1b}[2m"), "the box is dim");

        let allowed = ["\u{1b}[2J", "\u{1b}[H", RESET, BACKGROUND, TEXT, QUIET];
        for sequence in sequences(&ansi) {
            let known = allowed.contains(&sequence.as_str())
                || sequence == "\u{1b}[1m"
                || sequence == "\u{1b}[2m"
                || is_position(&sequence);
            assert!(
                known,
                "unexpected escape sequence {sequence:?} in:\n{ansi:?}"
            );
        }
    }

    /// A frame drawn on a terminal of exactly its own height must not scroll
    /// it: no row ends in a line break, every row is placed by address, and
    /// the last thing written is the reset, inside the frame.
    #[test]
    fn no_row_ends_in_a_line_break_and_every_row_is_placed_by_address() {
        let frame = a_frame();
        let ansi = frame.ansi();
        assert!(
            !ansi.contains('\n'),
            "a line break would scroll the terminal"
        );
        assert!(!ansi.contains('\r'));
        let positions = sequences(&ansi)
            .into_iter()
            .filter(|s| is_position(s))
            .count();
        assert_eq!(positions, frame.rows.len(), "one address per row");
        assert!(ansi.contains(&position(1)) && ansi.contains(&position(frame.rows.len())));
        assert!(ansi.ends_with(RESET));
    }

    #[test]
    fn the_terminal_and_the_transcript_carry_the_same_characters() {
        let frame = a_frame();
        let mut stripped = frame.ansi();
        for sequence in sequences(&stripped.clone()) {
            // A row address stands where the transcript has a line break.
            let replacement = if is_position(&sequence) { "\n" } else { "" };
            stripped = stripped.replacen(&sequence, replacement, 1);
        }
        assert_eq!(
            format!("{}\n", stripped.trim_start_matches('\n')),
            frame.plain()
        );
    }

    #[test]
    fn the_trailer_is_for_the_transcript_only() {
        let mut frame = a_frame();
        frame.trailer = vec!["Next step: type a finite number".to_owned()];
        assert!(frame
            .plain()
            .contains("\nNext step: type a finite number\n"));
        assert!(!frame.ansi().contains("Next step:"));
    }

    #[test]
    fn the_marker_sits_under_the_step_it_points_at() {
        assert_eq!(marker("Inputs"), "      [Inputs]");
        assert_eq!(marker("Goal"), "[Goal]");
    }

    #[test]
    fn prose_wraps_between_words_and_only_breaks_a_word_it_has_to() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap("", 10), vec![""]);
        for line in wrap("a much longer sentence than the pane is wide", 12) {
            assert!(cells(&line) <= 12, "{line:?}");
        }
    }

    #[test]
    fn a_typed_value_is_carried_whole_even_when_it_needs_two_rows() {
        assert_eq!(fit_value("N*s/m", 20), vec!["N*s/m"]);
        let long = "func spring(k, c) = 8 / c";
        let laid = fit_value(long, 12);
        let rejoined: String = laid.iter().map(|line| line.trim_start()).collect();
        assert_eq!(rejoined, long, "every character survives, in order");
        for line in &laid {
            assert!(cells(line) <= 12, "{line:?}");
        }
    }

    #[test]
    fn the_header_names_the_step_and_the_state() {
        assert_eq!(
            a_frame().header(3),
            "--- frame 3 (step: inputs, state: proposed) ---"
        );
    }
}
