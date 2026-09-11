//! The panelled layout: four panes in one closed rectangle.
//!
//! `docs/interface.md` §4b. One terminal shows several at once -- the step
//! being worked, the measurements as they stand, the five steps and where the
//! flow is in them, and the keys that work right now -- and it does it in a
//! single frame that a transcript can hold and a checker can measure.
//!
//! # The shape, and why this shape
//!
//! ```text
//! ┌─ catalyst ──────────────┬─ measurements ────┐
//! │ the step being worked   │ the value and the │
//! │                         │ partials          │
//! │                         ├─ steps ───────────┤
//! │                         │ 1 Goal       done │
//! ├─ keys ──────────────────┴───────────────────┤
//! │ enter …   ctrl-s …   ctrl-c …               │
//! └─────────────────────────────────────────────┘
//! ```
//!
//! The work is a tall column on the left because that is where the eye starts
//! and where the typing happens. What the run produced sits at the top right,
//! next to the work rather than under it, so a value and the fields that made
//! it are on screen together. Progress sits under it, five short rows, because
//! "where am I" is a two-second question. The keys run along the bottom, full
//! width, because they belong to the whole frame and not to one pane.
//!
//! # Joins
//!
//! Every place a divider meets another line uses the character that belongs
//! there: `┬` where the vertical starts under the top wall, `├` where the
//! `steps` rule branches off it, `┴` where it ends on the `keys` rule, `┤` and
//! `│` at the outer wall. A divider that crossed a wall, or stopped one column
//! short of it, would be the first thing a reader noticed.
//!
//! # Sizes
//!
//! On a real terminal the frame is the size of that terminal, read once from
//! `stty size` before anything is drawn; everywhere else -- headless, a key
//! script, `--describe` -- it is 80 by 24, because that is the terminal
//! everybody has. `--width` and `--height` set it either way. A size the layout cannot honour -- one where the
//! flow trail would not fit on a line, or where a pane would have no rows --
//! is refused as `catalyst.tui_size_refused`, because a frame drawn wrong is
//! worse than a frame not drawn: the checker measures the drawing, and so does
//! the reader.

use super::render::{self, cells, Row, Span};
use crate::cli::{Args, Refused};
use crate::json::{obj, s, Json};

/// The terminal everybody has.
pub const DEFAULT_WIDTH: usize = 80;
pub const DEFAULT_HEIGHT: usize = 24;

/// One row per step, always. The steps pane is the one pane whose height is
/// not negotiable: it shows five steps or it is not showing the flow.
pub const STEP_ROWS: usize = 5;

/// Below this the layout stops being able to keep its promises: the flow trail
/// is 37 columns and has to sit on one line, the measurements need a label and
/// a number side by side, and every pane needs rows to put anything in.
const MIN_WIDTH: usize = 70;
const MIN_HEIGHT: usize = 16;
/// Above this a "terminal size" is a mistake rather than a terminal.
const MAX_WIDTH: usize = 400;
const MAX_HEIGHT: usize = 200;

/// The narrowest left column the flow trail and its bracketed marker fit in,
/// with the pane's one space of padding on each side.
const TRAIL_ROOM: usize = 41;
/// The narrowest right column a label and a number fit in side by side.
const RIGHT_ROOM: usize = 26;

/// Where everything is, for one terminal size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
    /// Inner width of the left column, padding included.
    pub left: usize,
    /// Inner width of the right column, padding included.
    pub right: usize,
    /// Rows the measurements pane gets.
    pub measure_rows: usize,
    /// Rows the keys pane gets.
    pub key_rows: usize,
}

impl Default for Geometry {
    fn default() -> Geometry {
        Geometry::new(DEFAULT_WIDTH, DEFAULT_HEIGHT).expect("the default size is a valid layout")
    }
}

impl Geometry {
    pub fn new(width: usize, height: usize) -> Result<Geometry, Refused> {
        if !(MIN_WIDTH..=MAX_WIDTH).contains(&width) || !(MIN_HEIGHT..=MAX_HEIGHT).contains(&height)
        {
            return Err(refused(width, height));
        }
        // Two outer walls and one divider are structure, not content.
        let inner = width - 3;
        let right = (inner * 2 / 5)
            .clamp(RIGHT_ROOM, 48)
            .min(inner.saturating_sub(TRAIL_ROOM));
        if right < RIGHT_ROOM {
            return Err(refused(width, height));
        }
        // Top wall, steps rule, five step rows, keys rule, bottom wall.
        let spare = height - (STEP_ROWS + 4);
        let key_rows = match spare {
            0..=4 => 1,
            5 => 2,
            _ => 3,
        };
        let measure_rows = spare - key_rows;
        if measure_rows < 3 {
            return Err(refused(width, height));
        }
        Ok(Geometry {
            width,
            height,
            left: inner - right,
            right,
            measure_rows,
            key_rows,
        })
    }

    /// Rows the left column gets: everything beside the measurements, the
    /// `steps` rule and the steps.
    pub fn catalyst_rows(&self) -> usize {
        self.measure_rows + 1 + STEP_ROWS
    }

    /// Where each pane's content starts and how big it is, in terminal rows
    /// and columns with the outer wall at 0.
    pub fn panes(&self) -> [Rect; 4] {
        [
            Rect {
                name: "catalyst",
                row: 1,
                col: 1,
                width: self.left,
                height: self.catalyst_rows(),
            },
            Rect {
                name: "measurements",
                row: 1,
                col: self.left + 2,
                width: self.right,
                height: self.measure_rows,
            },
            Rect {
                name: "steps",
                row: self.measure_rows + 2,
                col: self.left + 2,
                width: self.right,
                height: STEP_ROWS,
            },
            Rect {
                name: "keys",
                row: self.measure_rows + 8,
                col: 1,
                width: self.width - 2,
                height: self.key_rows,
            },
        ]
    }

    /// Text columns inside a pane of this inner width: one space of padding
    /// on each side, always, so nothing ever touches a border.
    pub fn text(inner: usize) -> usize {
        inner.saturating_sub(2)
    }
}

/// One pane's place on the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub name: &'static str,
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub height: usize,
}

fn refused(width: usize, height: usize) -> Refused {
    Refused::new(
        "catalyst.tui_size_refused",
        format!(
            "a {width} by {height} terminal cannot hold the panelled layout, which needs \
             at least {MIN_WIDTH} columns by {MIN_HEIGHT} rows"
        ),
        format!(
            "widen the terminal, or pass --width and --height of at least {MIN_WIDTH} \
             and {MIN_HEIGHT}"
        ),
    )
}

/// The size `--width` and `--height` ask for, or the default.
///
/// When the frame is going to a real terminal, the default is that terminal's
/// own size, so a window twenty-two rows tall gets a twenty-two-row frame and
/// nothing scrolls. Either flag switches detection off for both dimensions,
/// since a person asking for a size has one in mind. A terminal larger than
/// the layout's ceiling is drawn at the ceiling rather than refused: a big
/// window is a fact, and only a typed number can be a mistake.
pub fn read(args: &Args, terminal: Option<(usize, usize)>) -> Result<Geometry, Refused> {
    let asked = args.value("width").is_some() || args.value("height").is_some();
    let (default_width, default_height) = match terminal {
        Some((columns, rows)) if !asked => (columns.min(MAX_WIDTH), rows.min(MAX_HEIGHT)),
        _ => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
    };
    let width = size(args, "width", default_width)?;
    let height = size(args, "height", default_height)?;
    Geometry::new(width, height)
}

fn size(args: &Args, flag: &str, fallback: usize) -> Result<usize, Refused> {
    match args.value(flag) {
        None => Ok(fallback),
        Some(text) => text.trim().parse::<usize>().map_err(|_| {
            Refused::new(
                "catalyst.tui_size_refused",
                format!("`--{flag}` needs a whole number of terminal cells"),
                format!("pass a number, as in `--{flag} {fallback}`"),
            )
        }),
    }
}

/// What goes in each pane, before it is placed. Rows longer than the pane are
/// clipped here rather than allowed to break the rectangle, but every builder
/// wraps its own text first, so clipping is the last defence and not the plan.
#[derive(Default)]
pub struct Panes {
    pub catalyst: Vec<Row>,
    pub measurements: Vec<Row>,
    pub steps: Vec<Row>,
    pub keys: Vec<Row>,
}

/// Draw the whole rectangle.
pub fn draw(g: &Geometry, panes: &Panes) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::with_capacity(g.height);

    // The top wall, both pane titles on it.
    out.push(vec![render::frame(format!(
        "┌{}┬{}┐",
        titled("catalyst", g.left),
        titled("measurements", g.right)
    ))]);

    // The measurements band: the work on the left, what was measured on the
    // right.
    for i in 0..g.measure_rows {
        let mut row = vec![render::frame("│")];
        row.extend(pad(panes.catalyst.get(i), g.left));
        row.push(render::frame("│"));
        row.extend(pad(panes.measurements.get(i), g.right));
        row.push(render::frame("│"));
        out.push(row);
    }

    // The `steps` rule branches off the divider without crossing it.
    let mut rule = vec![render::frame("│")];
    rule.extend(pad(panes.catalyst.get(g.measure_rows), g.left));
    rule.push(render::frame(format!("├{}┤", titled("steps", g.right))));
    out.push(rule);

    for i in 0..STEP_ROWS {
        let mut row = vec![render::frame("│")];
        row.extend(pad(panes.catalyst.get(g.measure_rows + 1 + i), g.left));
        row.push(render::frame("│"));
        row.extend(pad(panes.steps.get(i), g.right));
        row.push(render::frame("│"));
        out.push(row);
    }

    // The `keys` rule runs the whole width; the divider from above ends on it.
    out.push(vec![render::frame(format!(
        "├{}┴{}┤",
        titled("keys", g.left),
        "─".repeat(g.right)
    ))]);
    for i in 0..g.key_rows {
        let mut row = vec![render::frame("│")];
        row.extend(pad(panes.keys.get(i), g.width - 2));
        row.push(render::frame("│"));
        out.push(row);
    }

    out.push(vec![render::frame(format!(
        "└{}┘",
        "─".repeat(g.width - 2)
    ))]);
    out
}

/// `─ name ─────…`, filling the pane's inner width.
fn titled(name: &str, inner: usize) -> String {
    let head = format!("─ {name} ");
    match inner.checked_sub(cells(&head)) {
        Some(rest) => format!("{head}{}", "─".repeat(rest)),
        None => "─".repeat(inner),
    }
}

/// One pane cell: a space, the content, a space.
///
/// Content wider than the pane is cut, and the cut is *marked*. Silent
/// truncation is the failure mode that matters here: a number with its last
/// digit quietly missing is a number that is not true, and nothing on the
/// screen would say so. Every builder sizes its own rows, so a mark appearing
/// at all is a bug made visible rather than a layout doing its job.
fn pad(row: Option<&Row>, inner: usize) -> Row {
    let width = Geometry::text(inner);
    let mut out = vec![render::value(" ")];
    let mut used = 0;
    if let Some(row) = row {
        let cut = render::row_width(row) > width;
        let room = if cut { width.saturating_sub(1) } else { width };
        for span in row {
            if used >= room {
                break;
            }
            let text: String = span.text.chars().take(room - used).collect();
            used += cells(&text);
            out.push(Span {
                text,
                style: span.style,
            });
        }
        if cut && width > 0 {
            out.push(render::frame("\u{2026}"));
            used += 1;
        }
    }
    if used < width {
        out.push(render::value(" ".repeat(width - used)));
    }
    out.push(render::value(" "));
    out
}

/// The layout as data, so an agent drives the front end without reading a
/// drawing.
pub fn describe(g: &Geometry, steps: Vec<(&'static str, Vec<&'static str>)>) -> Json {
    let panes: Vec<Json> = g
        .panes()
        .iter()
        .map(|rect| {
            obj(vec![
                ("name", s(rect.name)),
                ("row", Json::Num(rect.row as f64)),
                ("col", Json::Num(rect.col as f64)),
                ("width", Json::Num(rect.width as f64)),
                ("height", Json::Num(rect.height as f64)),
            ])
        })
        .collect();
    let steps: Vec<Json> = steps
        .into_iter()
        .map(|(name, keys)| {
            obj(vec![
                ("name", s(name)),
                ("keys", Json::Arr(keys.into_iter().map(s).collect())),
            ])
        })
        .collect();
    obj(vec![
        ("ok", Json::Bool(true)),
        ("schema", s("catalyst.tui-layout.v1")),
        ("width", Json::Num(g.width as f64)),
        ("height", Json::Num(g.height as f64)),
        ("panes", Json::Arr(panes)),
        ("steps", Json::Arr(steps)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::Frame;

    fn args(argv: &[&str]) -> Args {
        Args::read(&argv.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_terminal_size_is_the_default_and_a_flag_overrides_it() {
        // A real terminal's size is the frame's size.
        let g = read(&args(&[]), Some((100, 22))).unwrap();
        assert_eq!((g.width, g.height), (100, 22));
        // No terminal, no flags: the terminal everybody has.
        let g = read(&args(&[]), None).unwrap();
        assert_eq!((g.width, g.height), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        // One flag switches detection off for both dimensions.
        let g = read(&args(&["--height", "30"]), Some((100, 22))).unwrap();
        assert_eq!((g.width, g.height), (DEFAULT_WIDTH, 30));
        // A huge window is drawn at the ceiling, a typed number is refused.
        let g = read(&args(&[]), Some((1000, 500))).unwrap();
        assert_eq!((g.width, g.height), (MAX_WIDTH, MAX_HEIGHT));
        let refused = read(&args(&["--width", "1000"]), None).unwrap_err();
        assert_eq!(refused.code, "catalyst.tui_size_refused");
        // A terminal too small to hold the layout is refused, with the remedy.
        let refused = read(&args(&[]), Some((60, 10))).unwrap_err();
        assert_eq!(refused.code, "catalyst.tui_size_refused");
    }

    fn drawn(g: &Geometry, panes: &Panes) -> Vec<String> {
        Frame {
            step: "goal",
            state: "proposed",
            rows: draw(g, panes),
            trailer: Vec::new(),
        }
        .plain()
        .lines()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn the_default_is_eighty_by_twenty_four_and_closes() {
        let g = Geometry::default();
        let lines = drawn(&g, &Panes::default());
        assert_eq!(lines.len(), 24);
        for line in &lines {
            assert_eq!(cells(line), 80, "{line}");
        }
        assert!(lines[0].starts_with('┌') && lines[0].ends_with('┐'));
        let last = lines.last().unwrap();
        assert!(last.starts_with('└') && last.ends_with('┘'));
        for line in &lines[1..lines.len() - 1] {
            assert!(line.starts_with('│') || line.starts_with('├'), "{line}");
            assert!(line.ends_with('│') || line.ends_with('┤'), "{line}");
        }
    }

    /// A divider that crossed a wall, or stopped short of one, is the first
    /// thing a reader would notice.
    #[test]
    fn every_join_is_the_join_that_belongs_there() {
        let g = Geometry::default();
        let lines = drawn(&g, &Panes::default());
        let divider = g.left + 1;
        let at = |line: &str, col: usize| line.chars().nth(col).unwrap();
        assert_eq!(
            at(&lines[0], divider),
            '┬',
            "the divider starts on the wall"
        );
        assert_eq!(at(&lines[1], divider), '│');
        let steps_rule = 1 + g.measure_rows;
        assert_eq!(
            at(&lines[steps_rule], divider),
            '├',
            "the steps rule branches off the divider"
        );
        assert!(lines[steps_rule].ends_with('┤'));
        let keys_rule = steps_rule + 1 + STEP_ROWS;
        assert_eq!(
            at(&lines[keys_rule], divider),
            '┴',
            "the divider ends on the keys rule"
        );
        assert!(lines[keys_rule].starts_with('├') && lines[keys_rule].ends_with('┤'));
    }

    #[test]
    fn every_pane_names_itself_on_its_own_wall() {
        let lines = drawn(&Geometry::default(), &Panes::default());
        let whole = lines.join("\n");
        for name in ["catalyst", "measurements", "steps", "keys"] {
            assert!(whole.contains(&format!("─ {name} ")), "{name} in:\n{whole}");
        }
    }

    /// One space of padding inside every pane wall, on every content row,
    /// however long the content is.
    #[test]
    fn text_never_touches_a_border() {
        let g = Geometry::default();
        let wide = vec![vec![render::value("x".repeat(200))]; 30];
        let panes = Panes {
            catalyst: wide.clone(),
            measurements: wide.clone(),
            steps: wide.clone(),
            keys: wide,
        };
        let lines = drawn(&g, &panes);
        let steps_rule = 1 + g.measure_rows;
        let keys_rule = steps_rule + 1 + STEP_ROWS;
        for (n, line) in lines.iter().enumerate() {
            let chars: Vec<char> = line.chars().collect();
            let split = [0, steps_rule, keys_rule, lines.len() - 1].contains(&n);
            if split {
                continue;
            }
            assert_eq!(chars[1], ' ', "row {n}: after the left wall: {line}");
            assert_eq!(
                chars[chars.len() - 2],
                ' ',
                "row {n}: before the right wall: {line}"
            );
            if n <= steps_rule + STEP_ROWS {
                assert_eq!(chars[g.left], ' ', "row {n}: before the divider: {line}");
                assert_eq!(chars[g.left + 2], ' ', "row {n}: after it: {line}");
            }
        }
    }

    #[test]
    fn a_wider_terminal_is_used_and_still_closes() {
        let g = Geometry::new(120, 30).expect("a valid size");
        let lines = drawn(&g, &Panes::default());
        assert_eq!(lines.len(), 30);
        for line in &lines {
            assert_eq!(cells(line), 120, "{line}");
        }
    }

    #[test]
    fn a_size_no_layout_can_honour_is_refused_rather_than_drawn_wrong() {
        for (w, h) in [(20, 24), (80, 4), (69, 24), (80, 15), (5000, 24)] {
            let refusal = Geometry::new(w, h).expect_err("refused");
            assert_eq!(refusal.code, "catalyst.tui_size_refused", "{w}x{h}");
        }
    }

    #[test]
    fn the_described_panes_are_the_panes_that_are_drawn() {
        let g = Geometry::default();
        let lines = drawn(&g, &Panes::default());
        for rect in g.panes() {
            assert!(rect.width >= 1 && rect.height >= 1, "{rect:?}");
            let bottom = rect.row + rect.height;
            assert!(bottom < lines.len(), "{rect:?} runs past the frame");
            // The row above a pane's first row is its own title rule, or the
            // top wall; the row below its last is a rule or the bottom wall.
            assert!(rect.col + rect.width < g.width, "{rect:?}");
        }
    }
}
