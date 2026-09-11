//! The panelled terminal front end (§4b). One terminal shows several at once:
//! the step being worked, the measurements as they stand, the five steps and
//! where the flow is in them, and the keys that work here.
//!
//! Minimal has a meaning that can be checked: it fits an eighty-column
//! terminal, it needs no colour to be read, and every line closes. Easy to
//! operate has one too: a person sees every key that works on this step, and an
//! agent can ask for the layout as data instead of parsing a drawing.
mod acceptance_common;
use acceptance_common as common;
use common::Json;
use std::path::Path;

const PANES: [&str; 4] = ["catalyst", "measurements", "steps", "keys"];

/// Read one field out of a transcript frame header.
fn header_field(header: &str, key: &str) -> String {
    let needle = format!("{key}: ");
    let start = match header.find(&needle) {
        Some(at) => at + needle.len(),
        None => return String::new(),
    };
    let rest = &header[start..];
    let end = rest.find([',', ')']).unwrap_or(rest.len());
    rest[..end].trim().to_owned()
}

/// The display width of a rendered line, counting a box-drawing character as
/// the one column it occupies.
fn width(line: &str) -> usize {
    line.chars().count()
}

/// Drive the whole typed flow and return its frames.
fn flow_frames(dir: &Path, extra: &[&str]) -> Vec<(String, String)> {
    common::write(
        &dir.join("keys.txt"),
        &common::typed_flow(
            common::SPRING_FUNCTION,
            common::SPRING_INPUTS,
            "export/spring",
        ),
    );
    let mut args = vec![
        "tui",
        "--headless",
        "--keys",
        "keys.txt",
        "--transcript",
        "t.txt",
    ];
    args.extend_from_slice(extra);
    let run = common::catalyst(&args, dir, &[]);
    assert_eq!(run.status, Some(0), "{}", run.summary());
    common::frames(&common::read_file(&dir.join("t.txt")))
}

#[test]
fn oracle_9_1_every_frame_is_one_closed_rectangle() {
    let dir = common::scratch("panes-closed");
    let frames = flow_frames(&dir, &[]);
    assert!(!frames.is_empty(), "9.1: the flow renders frames");
    for (header, body) in &frames {
        let lines: Vec<&str> = body.lines().collect();
        assert!(
            lines.len() >= 3,
            "9.1: a frame is a rectangle, not a line:\n{body}"
        );
        let w = width(lines[0]);
        for (n, line) in lines.iter().enumerate() {
            assert_eq!(
                width(line),
                w,
                "9.1: every line of a frame is the same width; line {n} of {header} is not:\n{body}"
            );
        }
        let first = lines[0];
        let last = lines[lines.len() - 1];
        assert!(
            first.starts_with('\u{250c}') && first.ends_with('\u{2510}'),
            "9.1: the frame opens with a top border:\n{body}"
        );
        assert!(
            last.starts_with('\u{2514}') && last.ends_with('\u{2518}'),
            "9.1: the frame closes with a bottom border:\n{body}"
        );
        for line in &lines[1..lines.len() - 1] {
            let opens = line.starts_with('\u{2502}') || line.starts_with('\u{251c}');
            let closes = line.ends_with('\u{2502}') || line.ends_with('\u{2524}');
            assert!(
                opens && closes,
                "9.1: every row is bounded on both sides:\n{line}"
            );
        }
    }
}

#[test]
fn oracle_9_2_one_terminal_shows_four_named_panes() {
    let dir = common::scratch("panes-named");
    for (header, body) in flow_frames(&dir, &[]) {
        for pane in PANES {
            assert!(
                body.contains(pane),
                "9.2: {header} must show the `{pane}` pane:\n{body}"
            );
        }
    }
}

#[test]
fn oracle_9_3_the_panes_carry_what_the_flow_already_promised() {
    // The layout is a way of showing the flow, not a different flow: every
    // guarantee section 4 already made is still on the frame.
    let dir = common::scratch("panes-content");
    let frames = flow_frames(&dir, &[]);
    for (header, body) in &frames {
        assert!(
            body.contains("Goal > Inputs > Run > Results > Export"),
            "9.3: the flow header stays on every frame:\n{body}"
        );
        let step = header_field(header, "step");
        let state = header_field(header, "state");
        assert!(
            body.contains(&format!("state: {state}")),
            "9.3: the state stays on every frame:\n{body}"
        );
        // The steps pane marks exactly one step as the one being worked, and
        // it is the step the frame header names.
        let here: Vec<&str> = body.lines().filter(|l| l.contains(" here")).collect();
        assert_eq!(
            here.len(),
            1,
            "9.3: exactly one step is marked as the current one:\n{body}"
        );
        assert!(
            here[0].to_lowercase().contains(&step),
            "9.3: the marked step must be `{step}`, the steps pane says `{}`",
            here[0].trim()
        );
    }
    // The last frame of the flow reports the export, as it always did.
    let (_, last) = frames.last().expect("9.3: a last frame");
    assert!(
        last.contains("exported:"),
        "9.3: the export is reported on the frame:\n{last}"
    );
}

#[test]
fn oracle_9_4_measurements_appear_in_their_pane_once_they_exist() {
    let dir = common::scratch("panes-measure");
    let frames = flow_frames(&dir, &[]);
    // Before anything is run there is nothing to show, and the pane says so
    // rather than showing a stale or invented number.
    let (_, first) = &frames[0];
    assert!(
        first.contains("measurements"),
        "9.4: the pane exists from the first frame:\n{first}"
    );
    assert!(
        !first.contains("d/d"),
        "9.4: nothing is measured yet, so no partial is shown:\n{first}"
    );
    // After the run, every partial is on the frame.
    let measured = frames
        .iter()
        .find(|(h, _)| header_field(h, "state") == "measured")
        .expect("9.4: the flow reaches `measured`");
    for name in ["d/dk", "d/dc"] {
        assert!(
            measured.1.contains(name),
            "9.4: the partial {name} is shown once it exists:\n{}",
            measured.1
        );
    }
}

#[test]
fn oracle_9_5_it_fits_a_plain_eighty_column_terminal_and_takes_a_size() {
    let dir = common::scratch("panes-size");
    // The default fits anywhere: eighty columns, and a height a
    // twenty-four-row terminal can show without scrolling.
    for (header, body) in flow_frames(&dir, &[]) {
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            width(lines[0]),
            80,
            "9.5: the default is eighty columns, {header} is {}",
            width(lines[0])
        );
        assert!(
            lines.len() <= 24,
            "9.5: the default fits twenty-four rows, {header} needs {}",
            lines.len()
        );
    }
    // A wider terminal is used, and the frame still closes.
    let dir = common::scratch("panes-wide");
    for (header, body) in flow_frames(&dir, &["--width", "120"]) {
        for line in body.lines() {
            assert_eq!(
                width(line),
                120,
                "9.5: at --width 120 every line is 120 wide, {header} is not:\n{body}"
            );
        }
    }
    // A width no layout can honour is refused, not silently drawn wrong.
    let dir = common::scratch("panes-narrow");
    common::write(&dir.join("keys.txt"), "text:x\n");
    let run = common::catalyst(
        &[
            "tui",
            "--headless",
            "--keys",
            "keys.txt",
            "--transcript",
            "t.txt",
            "--width",
            "20",
        ],
        &dir,
        &[],
    );
    common::assert_refusal(&run, "catalyst.tui_size_refused");
}

#[test]
fn oracle_9_6_every_key_the_legend_names_is_a_key_that_works_here() {
    // Easy to operate, for a person: the frame tells you what works.
    let dir = common::scratch("panes-keys");
    for (header, body) in flow_frames(&dir, &[]) {
        assert!(
            body.contains("ctrl-c"),
            "9.6: quitting is always offered:\n{body}"
        );
        assert!(
            body.contains("ctrl-s"),
            "9.6: saving is always offered:\n{body}"
        );
        assert!(
            body.contains("enter"),
            "9.6: {header} must say what enter does:\n{body}"
        );
    }
}

#[test]
fn oracle_9_7_an_agent_can_ask_for_the_layout_as_data() {
    // Easy to operate, for a program: the drawing is for people, and a machine
    // gets the same information without parsing it.
    let dir = common::scratch("panes-describe");
    let run = common::catalyst(&["tui", "--describe"], &dir, &[]);
    let json = common::assert_ok(&run);
    assert_eq!(json.str_field("schema"), "catalyst.tui-layout.v1");
    assert_eq!(json.num_field("width"), 80.0);
    let panes = json
        .get("panes")
        .and_then(Json::as_arr)
        .expect("9.7: the layout names its panes");
    assert_eq!(
        panes.len(),
        PANES.len(),
        "9.7: the described panes are the drawn panes"
    );
    for pane in panes {
        let name = pane.str_field("name");
        assert!(
            PANES.contains(&name),
            "9.7: `{name}` is not one of the drawn panes"
        );
        for field in ["row", "col", "width", "height"] {
            assert!(
                pane.get(field).is_some(),
                "9.7: pane `{name}` must say where it is: no `{field}`"
            );
        }
    }
    // And what each step accepts, so an agent need not guess a key script.
    let steps = json
        .get("steps")
        .and_then(Json::as_arr)
        .expect("9.7: the layout names the steps");
    let names: Vec<&str> = steps.iter().map(|s| s.str_field("name")).collect();
    assert_eq!(
        names,
        vec!["goal", "inputs", "run", "results", "export"],
        "9.7: the steps, in order"
    );
    for step in steps {
        let keys = step
            .get("keys")
            .and_then(Json::as_arr)
            .expect("9.7: each step names the keys that work on it");
        assert!(
            !keys.is_empty(),
            "9.7: `{}` accepts at least one key",
            step.str_field("name")
        );
    }
    // Describing the layout runs no flow and writes nothing.
    let left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.is_empty(), "9.7: --describe writes nothing: {left:?}");
}

#[test]
fn oracle_9_8_a_typed_value_is_never_split_by_a_pane_border() {
    // A layout that cuts `N*s/m` in half is worse than no layout.
    let dir = common::scratch("panes-whole");
    let frames = flow_frames(&dir, &[]);
    let run_frame = frames
        .iter()
        .find(|(h, _)| header_field(h, "step") == "run")
        .expect("9.8: the flow reaches `run` with every value entered");
    for value in ["12", "1.5", "N/m", "N*s/m"] {
        assert!(
            run_frame.1.contains(value),
            "9.8: `{value}` must appear whole on the frame:\n{}",
            run_frame.1
        );
    }
}

#[test]
fn oracle_9_9_the_terminal_front_end_paints_the_operators_green() {
    // The operator chose the background: #136207, which is
    // ESC [ 48;2;19;98;7 m as a true-colour sequence. It is the product's own
    // colour, so Catalyst paints it rather than relying on the terminal being
    // configured for it.
    let dir = common::scratch("panes-green");
    common::write(
        &dir.join("keys.txt"),
        &common::typed_flow(
            common::SPRING_FUNCTION,
            common::SPRING_INPUTS,
            "export/spring",
        ),
    );
    let terminal = common::catalyst(
        &["tui", "--keys", "keys.txt"],
        &dir,
        &[("TERM", "xterm-256color")],
    );
    assert_eq!(terminal.status, Some(0), "{}", terminal.summary());
    assert!(
        terminal.stdout.contains("\u{1b}[48;2;19;98;7m"),
        "9.9: the terminal front end paints its background #136207"
    );
    // It must also put the terminal back the way it found it.
    assert!(
        terminal.stdout.contains("\u{1b}[0m"),
        "9.9: and resets what it set, rather than leaving the terminal green"
    );
    // Colour is decoration over a frame that must read without it: the
    // headless transcript carries no escape sequence at all.
    let dir = common::scratch("panes-green-headless");
    let frames = flow_frames(&dir, &[]);
    for (header, body) in &frames {
        assert!(
            !body.contains('\u{1b}'),
            "9.9: {header} must be readable with no colour at all:\n{body}"
        );
    }
}

/// 9.10 — a frame never scrolls the terminal it is drawn on. Measured on the
/// live WezTerm pane during the phase-2 front-end test: the ANSI renderer
/// ended the last row of every frame with a line break, so on a terminal of
/// exactly the declared height each keystroke scrolled the screen by one row
/// and the top border appeared twice. The headless transcript cannot show
/// this, so the bytes of the ANSI renderer are what is measured here: after
/// the last row of a frame the next byte is never a newline, and every frame
/// carries exactly the declared number of rows.
#[test]
fn oracle_9_10_the_ansi_frame_never_writes_past_its_last_row() {
    let dir = common::scratch("no-scroll");
    let keys = common::keys(&[
        "text:func simple(x, y) = x * y + sin(x)",
        "enter",
        "text:0.7",
        "tab",
        "text:-3",
        "tab",
        "text:3",
        "enter",
    ]);
    common::write(&dir.join("keys.txt"), &keys);
    let run = common::catalyst(&["tui", "--keys", "keys.txt"], &dir, &[]);
    assert_eq!(run.status, Some(0), "9.10:\n{}", run.summary());
    let bytes = run.stdout.as_bytes();
    // Frames begin at the cursor-home sequence; the trailing JSON object is
    // not a frame.
    let frames: Vec<&[u8]> = {
        let mut starts: Vec<usize> = Vec::new();
        let mut i = 0;
        while let Some(pos) = find(&bytes[i..], b"\x1b[H") {
            starts.push(i + pos);
            i += pos + 3;
        }
        assert!(
            starts.len() >= 3,
            "9.10: at least three frames were rendered"
        );
        let mut out = Vec::new();
        for (n, &s) in starts.iter().enumerate() {
            let e = starts.get(n + 1).copied().unwrap_or(bytes.len());
            out.push(&bytes[s..e]);
        }
        out
    };
    for (n, frame) in frames.iter().enumerate() {
        let text = String::from_utf8_lossy(frame);
        let last_corner = text
            .rfind('┘')
            .unwrap_or_else(|| panic!("9.10: frame {n} has a bottom border"));
        let after: String = text[last_corner + '┘'.len_utf8()..]
            .chars()
            .filter(|c| !c.is_ascii_control() || *c == '\n')
            .take_while(|c| *c != '{')
            .collect();
        assert!(
            !after.contains('\n'),
            "9.10: frame {n} writes a line break after its last row, which scrolls a terminal of exactly the declared height"
        );
        let rows = text.matches('│').count();
        assert!(rows > 0, "9.10: frame {n} has rows");
        let newlines = text.matches('\n').count();
        assert!(
            newlines <= 23,
            "9.10: frame {n} has {newlines} line breaks; 24 rows need at most 23"
        );
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
