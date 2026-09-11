//! The guided flow: `Goal > Inputs > Run > Results > Export`.
//!
//! # One state machine, three front ends
//!
//! `docs/interface.md` §4 asks for an interactive terminal, the same terminal
//! renderer driven by a key script, and a plain-text renderer writing a
//! transcript. They are three ways of *displaying and driving* one
//! [`Session`], not three programs: [`Session::apply`] never learns which one
//! it is talking to, and the acceptance oracle proves it by driving the flow
//! twice, once each way, and comparing the two exports byte for byte. If the
//! step logic lived in a front end, that comparison would have nothing to
//! measure.
//!
//! # Why the flow validates through the same reader as the file front door
//!
//! The Inputs step could check its four fields itself. It does not: it builds
//! a `catalyst.problem.v1` document out of what was typed and hands it to
//! [`crate::problem::parse`], the same reader `catalyst eval --problem` uses.
//! So "min below max", "inside the declared domain" and every message about
//! them are one implementation. A flow with its own opinion about a valid
//! problem would eventually accept something the file front door rejects, and
//! the user would have found the difference by being told two things.
//!
//! For the same reason the Export step calls [`crate::export::write_go_export`]
//! rather than writing files of its own. The export a person clicks through to
//! is the export a script produces, down to the byte.
//!
//! # What a frame is for
//!
//! Every frame says where the flow is (the trail, and the step in brackets),
//! what the run is (`state:`), what is on the screen, and what the keys do.
//! The legend is on every frame on purpose: a guided flow whose keys are
//! documented elsewhere is not guided.

pub mod keys;
pub mod layout;
pub mod render;

use crate::api::Answer;
use crate::cli::{Args, Refused};
use crate::export;
use crate::json::{self, obj, s, Json};
use crate::paths;
use crate::problem::{self, Problem};
use crate::text;
use keys::Event;
use layout::Geometry;
use render::{cells, Frame, Row};
use std::io::Write as _;

/// The five state labels, in the order `catalyst tui --states` prints them.
pub const STATES: [&str; 5] = ["proposed", "running", "measured", "failed", "inconclusive"];

/// Where `ctrl-s` writes when `--save` was not given.
pub const DEFAULT_SAVE: &str = "catalyst-tui-state.json";

/// The five steps of the flow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Goal,
    Inputs,
    Run,
    Results,
    Export,
}

/// The five steps in the order the trail names them.
pub const ORDER: [Step; 5] = [
    Step::Goal,
    Step::Inputs,
    Step::Run,
    Step::Results,
    Step::Export,
];

impl Step {
    /// The name in the frame header and the state file: lower case, because
    /// it is an identifier rather than a caption.
    pub fn label(self) -> &'static str {
        match self {
            Step::Goal => "goal",
            Step::Inputs => "inputs",
            Step::Run => "run",
            Step::Results => "results",
            Step::Export => "export",
        }
    }

    /// The name in the trail, which a person reads.
    pub fn title(self) -> &'static str {
        match self {
            Step::Goal => "Goal",
            Step::Inputs => "Inputs",
            Step::Run => "Run",
            Step::Results => "Results",
            Step::Export => "Export",
        }
    }

    fn from_label(label: &str) -> Option<Step> {
        ORDER.into_iter().find(|step| step.label() == label)
    }

    /// Every key this step acts on, and nothing else.
    ///
    /// The legend on the frame and the list `--describe` prints are generated
    /// from this one table, so a step cannot advertise a key it ignores in one
    /// place and not the other. `esc` is absent from Goal because Goal is the
    /// first step and `esc` there does nothing; `tab` is absent wherever there
    /// is at most one field to move between.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Step::Goal => &["enter", "backspace", "ctrl-s", "ctrl-c"],
            Step::Inputs => &[
                "enter",
                "tab",
                "backtab",
                "up",
                "down",
                "backspace",
                "esc",
                "ctrl-s",
                "ctrl-c",
            ],
            Step::Run | Step::Results => &["enter", "esc", "ctrl-s", "ctrl-c"],
            Step::Export => &["enter", "backspace", "esc", "ctrl-s", "ctrl-c"],
        }
    }

    /// The legend, as it is drawn: the key as a person presses it, what it
    /// does, and a shorter way of saying so for a narrow terminal.
    fn legend(self) -> Vec<(&'static str, &'static str, &'static str)> {
        let (accept, accept_short) = match self {
            Step::Goal => ("parse it", "parse"),
            Step::Inputs => ("accept fields", "accept"),
            Step::Run => ("run it", "run"),
            Step::Results => ("on to Export", "export"),
            Step::Export => ("write export", "write"),
        };
        let back = match self {
            Step::Inputs => ("esc", "back to Goal", "back"),
            Step::Run | Step::Results => ("esc", "back to Inputs", "back"),
            Step::Export => ("esc", "back to Results", "back"),
            Step::Goal => ("", "", ""),
        };
        let mut out = vec![("enter", accept, accept_short)];
        if self == Step::Inputs {
            out.push(("tab", "next field", "next"));
            out.push(("backtab", "prev field", "prev"));
            out.push(("up/down", "move", "move"));
        }
        if matches!(self, Step::Goal | Step::Inputs | Step::Export) {
            out.push(("backspace", "rub out", "rub out"));
        }
        if !back.0.is_empty() {
            out.push(back);
        }
        out.push(("ctrl-s", "save", "save"));
        out.push(("ctrl-c", "quit", "quit"));
        out
    }

    /// Where `esc` goes.
    ///
    /// Results goes back to Inputs rather than to Run, which is the one place
    /// this is not simply the previous step: Run holds nothing to correct, and
    /// somebody backing out of a result wants the numbers that produced it.
    fn back(self) -> Step {
        match self {
            Step::Goal => Step::Goal,
            Step::Inputs => Step::Goal,
            Step::Run | Step::Results => Step::Inputs,
            Step::Export => Step::Results,
        }
    }
}

/// What the run is, as far as anyone can honestly say.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Proposed,
    Running,
    Measured,
    Failed,
    Inconclusive,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Proposed => STATES[0],
            State::Running => STATES[1],
            State::Measured => STATES[2],
            State::Failed => STATES[3],
            State::Inconclusive => STATES[4],
        }
    }

    fn from_label(label: &str) -> Option<State> {
        [
            State::Proposed,
            State::Running,
            State::Measured,
            State::Failed,
            State::Inconclusive,
        ]
        .into_iter()
        .find(|state| state.label() == label)
    }
}

/// The four fields every parameter has, in the order they are shown and
/// tabbed through.
const SLOTS: [&str; 4] = ["value", "min", "max", "unit"];

/// What one key did: the frames to show, and whether the flow is over.
pub struct Applied {
    pub frames: Vec<Frame>,
    pub quit: bool,
}

/// The whole flow.
pub struct Session {
    step: Step,
    state: State,
    /// The Goal step's one field: the function source, as typed.
    function: String,
    /// The function's own name, once it has parsed. The problem is named
    /// after it, and so is the default export directory.
    name: String,
    params: Vec<String>,
    /// Four entries per parameter, as typed. Kept as text rather than as
    /// numbers so a frame can show exactly what was entered, including the
    /// half-typed number that is about to be corrected.
    fields: Vec<String>,
    export_path: String,
    default_export: Option<String>,
    focus: usize,
    save_path: String,
    problem: Option<Problem>,
    answer: Option<Answer>,
    message: Option<(String, String)>,
    notes: Vec<String>,
    exported: Option<String>,
    /// The terminal the frames are drawn for.
    geometry: Geometry,
}

impl Session {
    pub fn new(save_path: &str, default_export: Option<&str>) -> Session {
        Session {
            step: Step::Goal,
            state: State::Proposed,
            function: String::new(),
            name: String::new(),
            params: Vec::new(),
            fields: Vec::new(),
            export_path: String::new(),
            default_export: default_export.map(str::to_owned),
            focus: 0,
            save_path: save_path.to_owned(),
            problem: None,
            answer: None,
            message: None,
            notes: Vec::new(),
            exported: None,
            geometry: Geometry::default(),
        }
    }

    /// Draw for a terminal of this size from now on.
    pub fn set_geometry(&mut self, geometry: Geometry) {
        self.geometry = geometry;
    }

    pub fn step(&self) -> Step {
        self.step
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn exported(&self) -> Option<&str> {
        self.exported.as_deref()
    }

    // -----------------------------------------------------------------
    // Keys
    // -----------------------------------------------------------------

    /// Apply one key and say what to show.
    pub fn apply(&mut self, event: Event) -> Applied {
        // A message and a note belong to the key that produced them. Clearing
        // them here means a `Problem:` never outlives the input that caused
        // it, which is the difference between a warning and a stuck screen.
        self.message = None;
        self.notes.clear();
        match event {
            Event::CtrlC => {
                return Applied {
                    frames: Vec::new(),
                    quit: true,
                }
            }
            Event::CtrlS => self.save(),
            Event::Enter => return self.accept(),
            Event::Text(text) => self.type_text(&text),
            Event::Backspace => self.rub_out(),
            Event::Tab | Event::Down => self.move_focus(1),
            Event::BackTab | Event::Up => self.move_focus(-1),
            Event::Esc => {
                self.step = self.step.back();
                self.focus = 0;
                // Going back to the goal or the fields means the fields are
                // about to change, so the measurement stops being true. The
                // measurements pane may not show a number that is not
                // currently true, and the cheapest way to keep that promise
                // is to have no stale number to show.
                if matches!(self.step, Step::Goal | Step::Inputs) {
                    self.forget_measurement();
                }
            }
        }
        Applied {
            frames: vec![self.frame()],
            quit: false,
        }
    }

    /// `enter`, which is the only key that means something different on every
    /// step.
    fn accept(&mut self) -> Applied {
        match self.step {
            Step::Goal => self.parse_goal(),
            Step::Inputs => {
                if self.validate_inputs() {
                    self.step = Step::Run;
                }
            }
            Step::Run => {
                // The one key that produces two frames: the run has to be
                // visible while it happens, not only after it has finished.
                self.state = State::Running;
                let running = self.frame_inner(true);
                self.evaluate();
                self.step = Step::Results;
                return Applied {
                    frames: vec![running, self.frame()],
                    quit: false,
                };
            }
            Step::Results => {
                self.step = Step::Export;
                self.focus = 0;
            }
            Step::Export => self.write_export(),
        }
        Applied {
            frames: vec![self.frame()],
            quit: false,
        }
    }

    fn parse_goal(&mut self) {
        match text::parse(&self.function) {
            Ok(parsed) => {
                // Re-typing the same signature keeps what was already entered;
                // a different one cannot, because the fields are the
                // parameters.
                if parsed.params != self.params {
                    self.fields = vec![String::new(); parsed.params.len() * SLOTS.len()];
                }
                self.name = parsed.name;
                self.params = parsed.params;
                self.step = Step::Inputs;
                self.state = State::Proposed;
                self.focus = 0;
            }
            Err(refusal) => self.complain(Refused::from(refusal)),
        }
    }

    fn type_text(&mut self, text: &str) {
        match self.step {
            Step::Goal => {
                self.function.push_str(text);
                self.forget_measurement();
            }
            Step::Inputs => {
                if let Some(field) = self.fields.get_mut(self.focus) {
                    field.push_str(text);
                    self.forget_measurement();
                }
            }
            Step::Export => self.export_path.push_str(text),
            Step::Run | Step::Results => {}
        }
    }

    fn rub_out(&mut self) {
        let edited = matches!(self.step, Step::Goal | Step::Inputs);
        let field = match self.step {
            Step::Goal => Some(&mut self.function),
            Step::Inputs => self.fields.get_mut(self.focus),
            Step::Export => Some(&mut self.export_path),
            Step::Run | Step::Results => None,
        };
        if let Some(field) = field {
            field.pop();
            if edited {
                self.forget_measurement();
            }
        }
    }

    /// Drop a measurement the inputs no longer support.
    ///
    /// The state goes back with it. A frame saying `state: measured` beside a
    /// pane with nothing in it would be two halves of the screen disagreeing
    /// about whether a run had happened.
    fn forget_measurement(&mut self) {
        if self.answer.is_some() || self.state != State::Proposed {
            self.answer = None;
            self.state = State::Proposed;
        }
    }

    fn move_focus(&mut self, by: isize) {
        let count = self.field_count();
        if count == 0 {
            self.focus = 0;
            return;
        }
        let count = count as isize;
        self.focus = (((self.focus as isize + by) % count + count) % count) as usize;
    }

    fn field_count(&self) -> usize {
        match self.step {
            Step::Goal | Step::Export => 1,
            Step::Inputs => self.fields.len(),
            Step::Run | Step::Results => 0,
        }
    }

    fn complain(&mut self, refused: Refused) {
        self.message = Some((refused.detail, refused.remedy));
    }

    // -----------------------------------------------------------------
    // The problem, the run and the export
    // -----------------------------------------------------------------

    /// Build the typed fields into a problem document and read it back with
    /// the ordinary reader. `true` when the flow may go on.
    fn validate_inputs(&mut self) -> bool {
        match self.build_problem() {
            Ok(problem) => {
                self.problem = Some(problem);
                true
            }
            Err(refused) => {
                self.complain(refused);
                false
            }
        }
    }

    fn build_problem(&self) -> Result<Problem, Refused> {
        let mut inputs = Vec::new();
        let mut domains = Vec::new();
        for (index, name) in self.params.iter().enumerate() {
            let value = self.number(index, 0)?;
            let min = self.number(index, 1)?;
            let max = self.number(index, 2)?;
            let unit = self.field(index, 3).trim().to_owned();
            inputs.push((name.clone(), Json::Num(value)));
            domains.push((
                name.clone(),
                obj(vec![
                    ("min", Json::Num(min)),
                    ("max", Json::Num(max)),
                    ("unit", s(&unit)),
                ]),
            ));
        }
        // The goal text is empty for a typed flow: nobody was asked for one,
        // and inventing a sentence here would put words in the export that
        // the user never wrote.
        let document = obj(vec![
            ("schema", s("catalyst.problem.v1")),
            ("name", s(&self.name)),
            ("goal", s("")),
            ("function", s(&self.function)),
            ("inputs", Json::Obj(inputs)),
            ("domains", Json::Obj(domains)),
        ]);
        problem::parse(&document.render())
    }

    fn field(&self, param: usize, slot: usize) -> &str {
        self.fields
            .get(param * SLOTS.len() + slot)
            .map(String::as_str)
            .unwrap_or("")
    }

    fn number(&self, param: usize, slot: usize) -> Result<f64, Refused> {
        let name = &self.params[param];
        let what = SLOTS[slot];
        let text = self.field(param, slot).trim();
        let value: f64 = text.parse().map_err(|_| self.not_a_number(name, what))?;
        if !value.is_finite() {
            return Err(self.not_a_number(name, what));
        }
        Ok(value)
    }

    fn not_a_number(&self, name: &str, what: &str) -> Refused {
        Refused::new(
            "catalyst.syntax",
            format!("the {what} field of `{name}` does not hold a finite number"),
            format!(
                "tab to the {what} field of `{name}` and type a finite number, \
                 such as 12 or 1.5e-3"
            ),
        )
    }

    fn evaluate(&mut self) {
        let Some(problem) = &self.problem else {
            self.state = State::Failed;
            self.complain(Refused::new(
                "catalyst.problem_incomplete",
                "there is no validated problem to run",
                "go back to Inputs with esc and accept the fields with enter",
            ));
            return;
        };
        match crate::api::gradient(&problem.function, problem.at()) {
            Ok(answer) => {
                let partials_finite = answer.gradient.iter().all(|g| g.is_finite());
                self.state = if !answer.value.is_finite() {
                    State::Failed
                } else if partials_finite {
                    State::Measured
                } else {
                    State::Inconclusive
                };
                self.answer = Some(answer);
            }
            // An engine refusal is a failed run rather than a broken flow: the
            // user is told what was refused and can correct it and run again.
            Err(refusal) => {
                self.answer = None;
                self.state = State::Failed;
                self.complain(Refused::from(refusal));
            }
        }
    }

    /// Where the export goes: what was typed, or the default when nothing was.
    fn export_target(&self) -> String {
        let typed = self.export_path.trim();
        if !typed.is_empty() {
            return typed.to_owned();
        }
        self.default_export
            .clone()
            .unwrap_or_else(|| format!("export/{}", self.name))
    }

    fn write_export(&mut self) {
        let named = self.export_target();
        let Some(problem) = self.problem.clone() else {
            self.complain(Refused::new(
                "catalyst.problem_incomplete",
                "there is no validated problem to export",
                "go back with esc and accept the inputs with enter first",
            ));
            return;
        };
        // The path rule first, exactly as `catalyst export go` applies it: a
        // directory outside the current one is refused before it is created.
        let out = match paths::prepare_out_dir(&named) {
            Ok(out) => out,
            Err(refused) => return self.complain(refused),
        };
        match export::write_go_export(&problem, &out) {
            Ok(cases) => {
                self.exported = Some(named.clone());
                self.notes.push(format!("exported: {named}"));
                self.notes
                    .push(format!("files: {}", export::FILES.join(", ")));
                self.notes.push(format!("fixture cases: {cases}"));
            }
            Err(refused) => self.complain(refused),
        }
    }

    // -----------------------------------------------------------------
    // Saving and resuming
    // -----------------------------------------------------------------

    fn save(&mut self) {
        match self.write_state() {
            Ok(()) => {
                let path = self.save_path.clone();
                self.notes.push(format!("saved: {path}"));
            }
            Err(refused) => self.complain(refused),
        }
    }

    fn write_state(&self) -> Result<(), Refused> {
        let resolved = paths::resolve(&self.save_path)?;
        if let Some(parent) = resolved.full.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&resolved.full, self.state_document().render_pretty()).map_err(|_| {
            Refused::new(
                "catalyst.path_refused",
                format!("the state file `{}` could not be written", self.save_path),
                "name a file inside the current directory that this user may write",
            )
        })
    }

    fn state_document(&self) -> Json {
        let mut fields: Vec<(String, Json)> = Vec::new();
        for (index, name) in self.params.iter().enumerate() {
            for (slot, what) in SLOTS.iter().enumerate() {
                fields.push((format!("{name}.{what}"), s(self.field(index, slot))));
            }
        }
        fields.push(("export.path".to_owned(), s(&self.export_path)));
        obj(vec![
            ("schema", s("catalyst.tui-state.v1")),
            ("step", s(self.step.label())),
            ("state", s(self.state.label())),
            ("function", s(&self.function)),
            ("fields", Json::Obj(fields)),
        ])
    }

    /// Restore a saved session.
    ///
    /// The parameters are not in the file: they come back from re-parsing the
    /// function, which is the only place they were ever defined, and that also
    /// means a state file cannot claim a set of fields the function does not
    /// have.
    pub fn restore(&mut self, text_of_state: &str) -> Result<(), Refused> {
        let document = Json::parse(text_of_state).map_err(|error| {
            Refused::new(
                "catalyst.syntax",
                format!(
                    "the state file is not readable JSON: at byte {}, expected {}",
                    error.at, error.what
                ),
                "resume from a file written by ctrl-s in the guided flow",
            )
        })?;
        if document.get("schema").and_then(Json::as_str) != Some("catalyst.tui-state.v1") {
            return Err(Refused::new(
                "catalyst.syntax",
                "the state file is not a catalyst.tui-state.v1 document",
                "resume from a file written by ctrl-s in the guided flow",
            ));
        }
        self.function = document
            .get("function")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_owned();
        let mut step = document
            .get("step")
            .and_then(Json::as_str)
            .and_then(Step::from_label)
            .unwrap_or(Step::Goal);
        self.state = document
            .get("state")
            .and_then(Json::as_str)
            .and_then(State::from_label)
            .unwrap_or(State::Proposed);

        let Ok(parsed) = text::parse(&self.function) else {
            // A function that no longer parses cannot have fields restored
            // against it, so the flow starts where it can: at the Goal, with
            // the text still there to be corrected.
            self.step = Step::Goal;
            return Ok(());
        };
        self.name = parsed.name;
        self.params = parsed.params;
        self.fields = vec![String::new(); self.params.len() * SLOTS.len()];
        if let Some(saved) = document.get("fields") {
            for (index, name) in self.params.clone().iter().enumerate() {
                for (slot, what) in SLOTS.iter().enumerate() {
                    if let Some(text) = saved.get(&format!("{name}.{what}")).and_then(Json::as_str)
                    {
                        self.fields[index * SLOTS.len() + slot] = text.to_owned();
                    }
                }
            }
            if let Some(path) = saved.get("export.path").and_then(Json::as_str) {
                self.export_path = path.to_owned();
            }
        }

        // A step past Inputs needs the problem those inputs make. Rebuilding
        // it here rather than trusting the file means a state saved with
        // fields that no longer validate comes back at the step that can fix
        // them instead of at one that would act on them.
        if !matches!(step, Step::Goal | Step::Inputs) {
            match self.build_problem() {
                Ok(problem) => self.problem = Some(problem),
                Err(_) => step = Step::Inputs,
            }
        }
        // Results without a measurement would be a frame stating a result
        // nobody produced. The run is a measurement, not a field, so it is
        // not restored: the flow comes back to the step that makes one.
        if step == Step::Results && self.answer.is_none() {
            step = Step::Run;
        }
        self.step = step;
        self.focus = 0;
        Ok(())
    }

    // -----------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------

    pub fn frame(&self) -> Frame {
        self.frame_inner(false)
    }

    fn frame_inner(&self, progress: bool) -> Frame {
        let g = self.geometry;
        let panes = layout::Panes {
            catalyst: clip(self.catalyst_pane(progress), g.catalyst_rows()),
            measurements: clip(self.measurements_pane(), g.measure_rows),
            steps: self.steps_pane(),
            keys: self.keys_pane(),
        };
        Frame {
            step: self.step.label(),
            state: self.state.label(),
            rows: layout::draw(&g, &panes),
            trailer: match &self.message {
                Some((problem, next)) => {
                    vec![format!("Problem: {problem}"), format!("Next step: {next}")]
                }
                None => Vec::new(),
            },
        }
    }

    // -- the `catalyst` pane: the step being worked ---------------------

    fn catalyst_pane(&self, progress: bool) -> Vec<Row> {
        let w = Geometry::text(self.geometry.left);
        let mut rows = vec![
            trail_row(self.step),
            vec![render::label(render::marker(self.step.title()))],
            vec![render::label("state: "), render::value(self.state.label())],
            Vec::new(),
        ];
        match self.step {
            Step::Goal => self.goal_rows(&mut rows, w),
            Step::Inputs => self.input_rows(&mut rows, w),
            Step::Run => self.run_rows(&mut rows, w, progress),
            Step::Results => self.result_rows(&mut rows, w),
            Step::Export => self.export_rows(&mut rows, w),
        }
        if let Some((problem, next)) = &self.message {
            rows.push(Vec::new());
            prose(&mut rows, &format!("Problem: {problem}"), w);
            prose(&mut rows, &format!("Next step: {next}"), w);
        }
        if !self.notes.is_empty() {
            rows.push(Vec::new());
        }
        for note in &self.notes {
            prose(&mut rows, note, w);
        }
        rows
    }

    fn goal_rows(&self, rows: &mut Vec<Row>, w: usize) {
        rows.push(vec![render::label("function")]);
        field_rows(rows, "", &self.function, true, w);
        rows.push(Vec::new());
        prose(rows, "the shape is `func name(a, b) = expression`", w);
    }

    fn input_rows(&self, rows: &mut Vec<Row>, w: usize) {
        rows.push(vec![render::label("value, range and unit, per parameter")]);
        for (index, name) in self.params.iter().enumerate() {
            rows.push(vec![render::label(name.clone())]);
            for (slot, what) in SLOTS.iter().enumerate() {
                let at = index * SLOTS.len() + slot;
                field_rows(
                    rows,
                    &format!("{what:<5}  "),
                    self.field(index, slot),
                    at == self.focus,
                    w,
                );
            }
        }
    }

    fn run_rows(&self, rows: &mut Vec<Row>, w: usize, progress: bool) {
        rows.push(vec![render::label("ready to run")]);
        rows.push(Vec::new());
        let table: Vec<Vec<String>> = self
            .params
            .iter()
            .enumerate()
            .map(|(index, name)| {
                vec![
                    name.clone(),
                    self.field(index, 0).trim().to_owned(),
                    format!(
                        "[{}, {}]",
                        self.field(index, 1).trim(),
                        self.field(index, 2).trim()
                    ),
                    self.field(index, 3).trim().to_owned(),
                ]
            })
            .collect();
        for cells in columns(
            &[Align::Left, Align::Decimal, Align::Left, Align::Left],
            &table,
        ) {
            rows.push(vec![
                render::value("  "),
                render::label(cells[0].clone()),
                render::value("  "),
                render::value(cells[1].clone()),
                render::value("  "),
                render::value(cells[2].clone()),
                render::value("  "),
                render::label(cells[3].clone()),
            ]);
        }
        rows.push(Vec::new());
        rows.push(vec![render::label("function")]);
        for piece in render::fit_value(&self.function, w.saturating_sub(2)) {
            rows.push(vec![render::value("  "), render::value(piece)]);
        }
        if progress {
            rows.push(Vec::new());
            prose(
                rows,
                "progress: evaluating the function and its gradient in one reverse sweep",
                w,
            );
        }
    }

    fn result_rows(&self, rows: &mut Vec<Row>, w: usize) {
        rows.push(vec![render::label("results")]);
        rows.push(Vec::new());
        let Some(answer) = &self.answer else {
            prose(rows, "the engine produced no result at this point.", w);
            return;
        };
        labelled(rows, "value", &shown(answer.value), w);
        // Which partials are trustworthy is the whole difference between
        // `measured` and `inconclusive`, so it is said in words and not left
        // to the reader to spot a phrase where a number should be.
        let unusable: Vec<String> = self
            .params
            .iter()
            .zip(&answer.gradient)
            .filter(|(_, partial)| !partial.is_finite())
            .map(|(name, _)| format!("d/d{name}"))
            .collect();
        let verdict = if unusable.is_empty() {
            "every one is finite".to_owned()
        } else {
            format!("not a number: {}", unusable.join(", "))
        };
        labelled(rows, "partials", &verdict, w);
        rows.push(Vec::new());
        labelled(
            rows,
            "cost",
            &format!(
                "{} primal instructions, {} adjoint",
                answer.primal_insts, answer.adjoint_insts
            ),
            w,
        );
    }

    fn export_rows(&self, rows: &mut Vec<Row>, w: usize) {
        rows.push(vec![render::label("where the Go export is written")]);
        field_rows(rows, "", &self.export_path, true, w);
        if self.export_path.trim().is_empty() {
            rows.push(vec![
                render::value("  "),
                render::label("empty means "),
                render::value(self.export_target()),
            ]);
        }
    }

    // -- the `measurements` pane ----------------------------------------

    /// The value and every partial once a run has produced them, and a dash
    /// before that. Never a number that is not currently true: the pane is
    /// driven by [`Session::answer`], which is dropped the moment an input
    /// that produced it changes.
    fn measurements_pane(&self) -> Vec<Row> {
        let w = Geometry::text(self.geometry.right);
        let Some(answer) = &self.answer else {
            return vec![vec![
                render::label(format!("{:<7}", "value")),
                render::value("-"),
            ]];
        };
        let mut table = vec![vec!["value".to_owned(), shown(answer.value)]];
        for (name, partial) in self.params.iter().zip(&answer.gradient) {
            table.push(vec![format!("d/d{name}"), shown(*partial)]);
        }
        let laid = columns(&[Align::Left, Align::Decimal], &table);
        let fits = laid
            .iter()
            .all(|cells| cells[0].chars().count() + 2 + cells[1].chars().count() <= w);
        let mut rows: Vec<Row> = Vec::new();
        if fits {
            for (n, cells) in laid.iter().enumerate() {
                // A blank line between the value and its partials: they are
                // two different things and the eye should not have to count
                // rows to tell them apart.
                if n == 1 {
                    rows.push(Vec::new());
                }
                rows.push(vec![
                    render::label(cells[0].clone()),
                    render::value("  "),
                    render::value(cells[1].clone()),
                ]);
            }
        } else {
            // Too narrow for a name and a number side by side. The number
            // goes under its name rather than losing its last digits: a
            // truncated number is not the number, and this pane may only
            // show what is true.
            let stacked = columns(
                &[Align::Decimal],
                &table
                    .iter()
                    .map(|entry| vec![entry[1].clone()])
                    .collect::<Vec<_>>(),
            );
            let room = self.geometry.measure_rows;
            for (n, entry) in table.iter().enumerate() {
                // One row held back for the count of what did not fit, so a
                // number never appears without the name of what it measures.
                let reserve = usize::from(n + 1 < table.len());
                if rows.len() + 2 + reserve > room {
                    break;
                }
                rows.push(vec![render::label(entry[0].clone())]);
                rows.push(vec![
                    render::value(" "),
                    render::value(stacked[n][0].clone()),
                ]);
            }
            let hidden = table.len() - rows.len() / 2;
            if hidden > 0 {
                rows.push(vec![render::label(format!("\u{2026} {hidden} more"))]);
            }
            return rows;
        }
        // The point they were measured at, because a gradient without one is
        // not a fact about anything.
        let at: Vec<Vec<String>> = self
            .params
            .iter()
            .enumerate()
            .map(|(index, name)| {
                vec![
                    name.clone(),
                    self.field(index, 0).trim().to_owned(),
                    self.field(index, 3).trim().to_owned(),
                ]
            })
            .collect();
        // Only when there is room for all of it: half a point is no point.
        if !at.is_empty() && w >= 16 && rows.len() + 2 + at.len() <= self.geometry.measure_rows {
            rows.push(Vec::new());
            rows.push(vec![render::label("at")]);
            for cells in columns(&[Align::Left, Align::Decimal, Align::Left], &at) {
                rows.push(vec![
                    render::label(cells[0].clone()),
                    render::value("  "),
                    render::value(cells[1].clone()),
                    render::value("  "),
                    render::label(cells[2].clone()),
                ]);
            }
        }
        rows
    }

    // -- the `steps` pane: where the flow is ----------------------------

    /// Five rows, one per step, each `done`, `here` or `-`, with the marks in
    /// a column of their own against the right wall. Two seconds of looking
    /// answers "where am I and what is left".
    fn steps_pane(&self) -> Vec<Row> {
        let w = Geometry::text(self.geometry.right);
        let current = ORDER.iter().position(|s| *s == self.step).unwrap_or(0);
        ORDER
            .iter()
            .enumerate()
            .map(|(n, step)| {
                let mark = match n.cmp(&current) {
                    std::cmp::Ordering::Less => "done",
                    std::cmp::Ordering::Equal => "here",
                    std::cmp::Ordering::Greater => "-",
                };
                let head = format!("{} {}", n + 1, step.title());
                let gap = w.saturating_sub(cells(&head) + cells(mark)).max(1);
                let (head, mark) = if n == current {
                    (render::strong(head), render::strong(mark))
                } else {
                    (render::label(head), render::value(mark))
                };
                vec![head, render::value(" ".repeat(gap)), mark]
            })
            .collect()
    }

    // -- the `keys` pane: what works right now --------------------------

    /// Every key this step acts on, and not one it ignores.
    ///
    /// What gives way as the terminal narrows is the *wording*, never a key:
    /// a legend missing `ctrl-s` is worse than a legend that says `save`
    /// instead of `save the session`. So the pane tries, in order, a column
    /// grid with the full wording, the same grid with the short wording, a
    /// packed line with each, and finally the key names alone -- and every
    /// one of those still names all of them.
    fn keys_pane(&self) -> Vec<Row> {
        let w = self.geometry.width - 4;
        let rows = self.geometry.key_rows;
        let legend = self.step.legend();
        let wording = |short: bool| -> Vec<(&'static str, &'static str)> {
            legend
                .iter()
                .map(|(key, full, brief)| (*key, if short { *brief } else { *full }))
                .collect()
        };
        let bare: Vec<(&str, &str)> = legend.iter().map(|(key, _, _)| (*key, "")).collect();
        for short in [false, true] {
            if let Some(drawn) = grid(&wording(short), w, rows) {
                return drawn;
            }
        }
        for short in [false, true] {
            if let Some(drawn) = packed(&wording(short), w, rows) {
                return drawn;
            }
        }
        packed(&bare, w, usize::MAX).unwrap_or_default()
    }
}

/// The flow trail, with the step being worked picked out. The trail itself is
/// verbatim on every frame: a reader (and a checker) finds the whole flow in
/// one string, and the bracketed marker underneath says where in it this is.
fn trail_row(step: Step) -> Row {
    let title = step.title();
    let at = render::TRAIL.find(title).unwrap_or(0);
    vec![
        render::label(&render::TRAIL[..at]),
        render::strong(title),
        render::label(&render::TRAIL[at + title.len()..]),
    ]
}

/// The width of the label column wherever a frame puts a name beside a thing.
/// One column for the names, one for the things, so the things line up.
const LABEL_COLUMN: usize = 10;

/// A named thing: the name in the label column, the thing beside it, and any
/// continuation hanging under the thing rather than under the name.
fn labelled(rows: &mut Vec<Row>, name: &str, text: &str, width: usize) {
    let room = width.saturating_sub(LABEL_COLUMN).max(1);
    for (n, line) in render::wrap(text, room).into_iter().enumerate() {
        let margin = if n == 0 {
            render::label(format!("{name:<LABEL_COLUMN$}"))
        } else {
            render::value(" ".repeat(LABEL_COLUMN))
        };
        rows.push(vec![margin, render::value(line)]);
    }
}

/// A sentence of the frame's own, wrapped into the pane.
fn prose(rows: &mut Vec<Row>, text: &str, width: usize) {
    for line in render::wrap(text, width) {
        rows.push(vec![render::value(line)]);
    }
}

/// An editable field: a marker column, an optional label, and the text
/// exactly as it was typed.
fn field_rows(rows: &mut Vec<Row>, prefix: &str, text: &str, focused: bool, width: usize) {
    let head = cells(prefix) + 2;
    let room = width.saturating_sub(head).max(1);
    for (n, piece) in render::fit_value(text, room).into_iter().enumerate() {
        let margin = if n > 0 {
            render::value(" ".repeat(head))
        } else if focused {
            render::strong(format!("\u{25b8} {prefix}"))
        } else {
            render::label(format!("  {prefix}"))
        };
        let body = if focused {
            render::strong(piece)
        } else {
            render::value(piece)
        };
        rows.push(vec![margin, body]);
    }
}

/// Keep a pane's rows inside it, saying so rather than silently losing the
/// end. The first rows are the ones that say where the flow is and the last
/// are the ones that just changed, so a pane too short drops from the middle.
fn clip(mut rows: Vec<Row>, height: usize) -> Vec<Row> {
    // Breathing space is the first thing to give up when there is none: a
    // blank row separates, and a row of content says something.
    while rows.len() > height {
        match rows.iter().rposition(Vec::is_empty) {
            Some(at) => {
                rows.remove(at);
            }
            None => break,
        }
    }
    if rows.len() <= height || height < 3 {
        return rows;
    }
    let head = 3.min(height - 2);
    let tail = height - head - 1;
    let hidden = rows.len() - head - tail;
    let mut out: Vec<Row> = rows[..head].to_vec();
    out.push(vec![render::label(format!("… {hidden} more rows"))]);
    out.extend_from_slice(&rows[rows.len() - tail..]);
    out
}

/// How a column of a table is lined up.
enum Align {
    Left,
    /// On the decimal point, which is the only way a column of numbers reads
    /// as a column.
    Decimal,
}

/// Pad every cell of a table so each column lines up.
fn columns(aligns: &[Align], rows: &[Vec<String>]) -> Vec<Vec<String>> {
    let width = aligns.len();
    let mut before = vec![0usize; width];
    let mut after = vec![0usize; width];
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(width) {
            let (head, tail) = split_decimal(cell, matches!(aligns[i], Align::Decimal));
            before[i] = before[i].max(cells(&head));
            after[i] = after[i].max(cells(&tail));
        }
    }
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .take(width)
                .map(|(i, cell)| match aligns[i] {
                    // A word starts where the column starts.
                    Align::Left => format!("{cell}{}", " ".repeat(before[i] - cells(cell))),
                    // A number hangs off its decimal point: the integer part
                    // grows leftwards and the fraction rightwards, so the
                    // points stack however long either side is.
                    Align::Decimal => {
                        let (head, tail) = split_decimal(cell, true);
                        format!(
                            "{}{head}{tail}{}",
                            " ".repeat(before[i] - cells(&head)),
                            " ".repeat(after[i] - cells(&tail))
                        )
                    }
                })
                .collect()
        })
        .collect()
}

/// A number split at its decimal point, so the points can be stacked.
fn split_decimal(text: &str, decimal: bool) -> (String, String) {
    match text.find('.') {
        Some(at) if decimal => (text[..at].to_owned(), text[at..].to_owned()),
        _ => (text.to_owned(), String::new()),
    }
}

/// Key and action pairs in equal columns, filled across. `None` when they do
/// not all fit, because a legend that fits by leaving a key out is wrong.
fn grid(entries: &[(&str, &str)], width: usize, rows: usize) -> Option<Vec<Row>> {
    let key_width = entries.iter().map(|(k, _)| cells(k)).max().unwrap_or(0);
    let act_width = entries.iter().map(|(_, a)| cells(a)).max().unwrap_or(0);
    let cell = key_width + 2 + act_width;
    if cell > width {
        return None;
    }
    let count = ((width + 2) / (cell + 2)).max(1);
    if entries.len().div_ceil(count) > rows {
        return None;
    }
    let mut out: Vec<Row> = Vec::new();
    for chunk in entries.chunks(count) {
        let mut row: Row = Vec::new();
        for (n, (key, action)) in chunk.iter().enumerate() {
            if n > 0 {
                row.push(render::value("  "));
            }
            row.push(render::label(format!("{key:<key_width$}  ")));
            row.push(render::value(*action));
            let used = key_width + 2 + cells(action);
            if n + 1 < chunk.len() && used < cell {
                row.push(render::value(" ".repeat(cell - used)));
            }
        }
        out.push(row);
    }
    Some(out)
}

/// The same pairs packed along each line instead of into columns: less tidy,
/// and it holds a legend a grid cannot.
fn packed(entries: &[(&str, &str)], width: usize, rows: usize) -> Option<Vec<Row>> {
    let mut out: Vec<Row> = Vec::new();
    let mut row: Row = Vec::new();
    let mut used = 0;
    for (key, action) in entries {
        let piece = cells(key) + usize::from(!action.is_empty()) + cells(action);
        let gap = if used == 0 { 0 } else { 3 };
        if used > 0 && used + gap + piece > width {
            out.push(std::mem::take(&mut row));
            used = 0;
        }
        if used > 0 {
            row.push(render::value("   "));
            used += 3;
        }
        row.push(render::label(*key));
        if !action.is_empty() {
            row.push(render::value(format!(" {action}")));
        }
        used += piece;
    }
    if !row.is_empty() {
        out.push(row);
    }
    if out.len() > rows {
        return None;
    }
    Some(out)
}

/// A number as a frame shows it: the shortest form that reads back, or a
/// phrase for the ones that are not numbers at all. `null` would be right in
/// JSON and wrong on a screen.
fn shown(x: f64) -> String {
    if x.is_finite() {
        json::number(x)
    } else if x.is_nan() {
        "not a number".to_owned()
    } else if x > 0.0 {
        "beyond the largest number".to_owned()
    } else {
        "beyond the smallest number".to_owned()
    }
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Where frames go: a transcript, a terminal, or both.
struct Sink {
    transcript: Option<std::fs::File>,
    ansi: bool,
    count: usize,
}

impl Sink {
    fn emit(&mut self, frame: &Frame) -> Result<(), Refused> {
        self.count += 1;
        if let Some(file) = &mut self.transcript {
            let entry = format!("{}\n{}", frame.header(self.count), frame.plain());
            file.write_all(entry.as_bytes()).map_err(|_| {
                Refused::new(
                    "catalyst.path_refused",
                    "the transcript could not be written",
                    "name a transcript file inside the current directory that this user may write",
                )
            })?;
        }
        if self.ansi {
            print!("{}", frame.ansi());
            let _ = std::io::stdout().flush();
        }
        Ok(())
    }
}

/// `catalyst tui …`. Returns the one JSON object the command prints.
pub fn run(args: &Args) -> Result<String, Refused> {
    if args.has("states") {
        return Ok(Json::Arr(STATES.iter().map(|state| s(state)).collect()).render());
    }

    // The size is settled before anything else, because a size that cannot be
    // drawn is a refusal and not a flow that runs and then cannot be shown.
    // Only a frame bound for a real terminal asks that terminal how big it is:
    // `--describe` runs nothing, and a headless or scripted run has no window.
    let interactive = !args.has("describe") && !args.has("headless") && !args.has("keys");
    let terminal = if interactive {
        keys::terminal_size()
    } else {
        None
    };
    let geometry = layout::read(args, terminal)?;

    // `--describe` is the front end answering a question about itself. It runs
    // no flow and writes nothing -- not a transcript, not a save file --
    // because an agent asking what the keys are has not asked for a session.
    if args.has("describe") {
        let steps = ORDER
            .iter()
            .map(|step| (step.label(), step.keys().to_vec()))
            .collect();
        return Ok(layout::describe(&geometry, steps).render());
    }

    let headless = args.has("headless");
    let script = args.value("keys");
    let transcript = args.value("transcript");
    if headless && script.is_none() {
        return Err(Refused::usage(
            "`catalyst tui --headless` has no keyboard, so it needs --keys FILE",
        ));
    }
    if headless && transcript.is_none() {
        return Err(Refused::usage(
            "`catalyst tui --headless` writes its frames to --transcript FILE, which was not given",
        ));
    }

    let mut session = Session::new(
        args.value("save").unwrap_or(DEFAULT_SAVE),
        args.value("out"),
    );
    session.set_geometry(geometry);
    if let Some(named) = args.value("resume") {
        session.restore(&paths::read_input(named)?)?;
    }

    // The whole script is read before the first frame: a script with an
    // unknown event is refused as a script, rather than half-run and then
    // stopped in the middle of somebody's flow.
    let events = match script {
        Some(named) => Some(keys::parse_script(&paths::read_input(named)?)?),
        None => None,
    };

    let mut sink = Sink {
        transcript: match transcript {
            Some(named) => Some(open_transcript(named)?),
            None => None,
        },
        ansi: !headless,
        count: 0,
    };

    sink.emit(&session.frame())?;
    match events {
        Some(events) => {
            for event in events {
                let applied = session.apply(event);
                for frame in &applied.frames {
                    sink.emit(frame)?;
                }
                if applied.quit {
                    break;
                }
            }
        }
        None => drive_from_keyboard(&mut session, &mut sink)?,
    }

    let mut answer = vec![
        ("ok", Json::Bool(true)),
        ("step", s(session.step().label())),
        ("state", s(session.state().label())),
        ("frames", Json::Num(sink.count as f64)),
    ];
    if let Some(named) = transcript {
        answer.push(("transcript", s(named)));
    }
    answer.push((
        "exported",
        match session.exported() {
            Some(path) => s(path),
            None => Json::Null,
        },
    ));
    Ok(obj(answer).render())
}

/// The interactive front end: raw mode through `stty`, keys decoded from
/// bytes, and the terminal put back however the run ends.
fn drive_from_keyboard(session: &mut Session, sink: &mut Sink) -> Result<(), Refused> {
    let terminal = keys::RawTerminal::enter()?;
    let bytes = keys::keyboard();
    let mut outcome = Ok(());
    while let Some(event) = keys::next_event(&bytes) {
        let applied = session.apply(event);
        for frame in &applied.frames {
            if let Err(refused) = sink.emit(frame) {
                outcome = Err(refused);
                break;
            }
        }
        if applied.quit || outcome.is_err() {
            break;
        }
    }
    terminal.restore();
    outcome
}

fn open_transcript(named: &str) -> Result<std::fs::File, Refused> {
    let resolved = paths::resolve(named)?;
    if let Some(parent) = resolved.full.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::File::create(&resolved.full).map_err(|_| {
        Refused::new(
            "catalyst.path_refused",
            format!("the transcript `{named}` could not be created"),
            "name a transcript file inside the current directory that this user may write",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(session: &mut Session, events: &[Event]) {
        for event in events {
            session.apply(event.clone());
        }
    }

    fn text(what: &str) -> Event {
        Event::Text(what.to_owned())
    }

    fn spring(session: &mut Session) {
        typed(
            session,
            &[
                text("func spring(k, c) = 8 / c + 100 * exp(0 - 3.141592653589793 * c / sqrt(4 * k - c * c))"),
                Event::Enter,
                text("12"),
                Event::Tab,
                text("1"),
                Event::Tab,
                text("100"),
                Event::Tab,
                text("N/m"),
                Event::Tab,
                text("1.5"),
                Event::Tab,
                text("0.1"),
                Event::Tab,
                text("1.9"),
                Event::Tab,
                text("N*s/m"),
            ],
        );
    }

    #[test]
    fn the_five_states_are_the_only_states_and_keep_their_order() {
        assert_eq!(
            STATES,
            ["proposed", "running", "measured", "failed", "inconclusive"]
        );
    }

    #[test]
    fn a_typed_flow_reaches_a_measured_result() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        spring(&mut session);
        assert_eq!(session.step(), Step::Inputs);
        session.apply(Event::Enter);
        assert_eq!(session.step(), Step::Run);
        let applied = session.apply(Event::Enter);
        assert_eq!(applied.frames.len(), 2, "the run is shown while it happens");
        assert_eq!(applied.frames[0].state, "running");
        assert_eq!(session.step(), Step::Results);
        assert_eq!(session.state(), State::Measured);
    }

    #[test]
    fn a_non_finite_value_fails_and_a_non_finite_partial_is_inconclusive() {
        for (function, value, min, max, want) in [
            ("func f(x) = sqrt(x)", "-1", "-2", "2", State::Failed),
            ("func f(x) = sqrt(x)", "0", "0", "1", State::Inconclusive),
        ] {
            let mut session = Session::new(DEFAULT_SAVE, None);
            typed(
                &mut session,
                &[
                    text(function),
                    Event::Enter,
                    text(value),
                    Event::Tab,
                    text(min),
                    Event::Tab,
                    text(max),
                    Event::Tab,
                    text(""),
                    Event::Enter,
                    Event::Enter,
                ],
            );
            assert_eq!(session.state(), want, "{function} at {value}");
        }
    }

    #[test]
    fn every_invalid_input_stays_on_its_step_with_a_problem_and_a_next_step() {
        for (label, events) in [
            (
                "a function that does not parse",
                vec![text("func f(x) = +"), Event::Enter],
            ),
            (
                "a name that is not a parameter",
                vec![text("func f(x) = x + q"), Event::Enter],
            ),
        ] {
            let mut session = Session::new(DEFAULT_SAVE, None);
            typed(&mut session, &events);
            assert_eq!(session.step(), Step::Goal, "{label}");
            let body = session.frame().plain();
            assert!(body.contains("Problem: "), "{label}: {body}");
            assert!(body.contains("Next step: "), "{label}: {body}");
        }

        for (label, value, min, max) in [
            ("not a number", "abc", "0", "2"),
            ("min not below max", "1", "3", "1"),
            ("outside the domain", "5", "0", "2"),
        ] {
            let mut session = Session::new(DEFAULT_SAVE, None);
            typed(
                &mut session,
                &[
                    text("func f(x) = x"),
                    Event::Enter,
                    text(value),
                    Event::Tab,
                    text(min),
                    Event::Tab,
                    text(max),
                    Event::Tab,
                    text(""),
                    Event::Enter,
                ],
            );
            assert_eq!(session.step(), Step::Inputs, "{label}");
            let body = session.frame().plain();
            assert!(body.contains("Problem: "), "{label}: {body}");
            assert!(body.contains("Next step: "), "{label}: {body}");
        }
    }

    #[test]
    fn esc_from_results_returns_to_inputs_with_every_value_kept() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        spring(&mut session);
        typed(&mut session, &[Event::Enter, Event::Enter]);
        assert_eq!(session.step(), Step::Results);
        session.apply(Event::Esc);
        assert_eq!(session.step(), Step::Inputs);
        let body = session.frame().plain();
        for value in ["12", "1.5", "N/m", "N*s/m"] {
            assert!(body.contains(value), "{value} missing from:\n{body}");
        }
    }

    #[test]
    fn a_saved_session_restores_its_step_and_every_field() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        spring(&mut session);
        let saved = session.state_document().render_pretty();

        let mut resumed = Session::new(DEFAULT_SAVE, None);
        resumed.restore(&saved).expect("the state restores");
        assert_eq!(resumed.step(), Step::Inputs);
        let body = resumed.frame().plain();
        for value in ["12", "1.5", "N/m", "N*s/m"] {
            assert!(body.contains(value), "{value} missing from:\n{body}");
        }
        // And the restored session can still be run, and carries everything
        // the Export step needs. The write itself is the same call
        // `catalyst export go` makes, into a directory the path rule has
        // already accepted.
        resumed.apply(Event::Enter);
        assert_eq!(resumed.step(), Step::Run);
        resumed.apply(Event::Enter);
        assert_eq!(resumed.state(), State::Measured);
        resumed.apply(Event::Enter);
        assert_eq!(resumed.step(), Step::Export);
        assert!(
            resumed.problem.is_some(),
            "the resumed flow reaches Export with a validated problem"
        );
        assert_eq!(resumed.export_target(), "export/spring");
        typed(&mut resumed, &[text("export/resumed")]);
        assert_eq!(resumed.export_target(), "export/resumed");
    }

    #[test]
    fn a_state_file_of_another_shape_is_refused() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        assert_eq!(
            session
                .restore("{\"schema\":\"catalyst.problem.v1\"}")
                .expect_err("refused")
                .code,
            "catalyst.syntax"
        );
    }

    // -- the panelled layout (section 4b) -------------------------------

    /// Walk the whole flow at a size and hand every frame to a checker, so a
    /// property is measured on every step rather than on the one that was
    /// convenient.
    fn every_frame(width: usize, height: usize, mut check: impl FnMut(&Frame)) {
        let mut session = Session::new(DEFAULT_SAVE, None);
        session.set_geometry(Geometry::new(width, height).expect("a valid size"));
        check(&session.frame());
        let script = [
            text("func spring(k, c) = 8 / c + 100 * exp(0 - 3.141592653589793 * c / sqrt(4 * k - c * c))"),
            Event::Enter,
            text("12"), Event::Tab, text("1"), Event::Tab, text("100"), Event::Tab, text("N/m"),
            Event::Tab,
            text("1.5"), Event::Tab, text("0.1"), Event::Tab, text("1.9"), Event::Tab, text("N*s/m"),
            Event::Enter,
            Event::Enter,
            Event::Enter,
        ];
        for event in script {
            for frame in session.apply(event).frames {
                check(&frame);
            }
        }
    }

    #[test]
    fn every_frame_of_the_flow_is_one_closed_rectangle_at_every_size() {
        for (width, height) in [(80, 24), (70, 16), (120, 40), (100, 24)] {
            every_frame(width, height, |frame| {
                let drawn = frame.plain();
                let lines: Vec<&str> = drawn.lines().collect();
                assert_eq!(lines.len(), height, "{drawn}");
                for line in &lines {
                    assert_eq!(cells(line), width, "{drawn}");
                }
                assert!(lines[0].starts_with('┌') && lines[0].ends_with('┐'));
                let last = lines[lines.len() - 1];
                assert!(last.starts_with('└') && last.ends_with('┘'));
                for line in &lines[1..lines.len() - 1] {
                    assert!(line.starts_with('│') || line.starts_with('├'), "{line}");
                    assert!(line.ends_with('│') || line.ends_with('┤'), "{line}");
                }
            });
        }
    }

    /// A person must see every key that works, at any size the layout
    /// accepts. Wording gives way; a key never does.
    #[test]
    fn no_size_drops_a_key_the_step_acts_on() {
        for (width, height) in [(80, 24), (70, 16), (120, 40)] {
            every_frame(width, height, |frame| {
                let drawn = frame.plain();
                let step = Step::from_label(frame.step).expect("a known step");
                for key in step.keys() {
                    assert!(
                        drawn.contains(key),
                        "{width}x{height}: {} must name `{key}`:\n{drawn}",
                        frame.step
                    );
                }
            });
        }
    }

    /// Exactly one step is `here`, and it is the step the header names. The
    /// whole steps pane is worthless if two rows can claim it.
    #[test]
    fn exactly_one_step_is_marked_here_and_it_is_the_one_the_header_names() {
        every_frame(80, 24, |frame| {
            let drawn = frame.plain();
            let marked: Vec<&str> = drawn.lines().filter(|l| l.contains(" here")).collect();
            assert_eq!(marked.len(), 1, "{drawn}");
            assert!(
                marked[0].to_lowercase().contains(frame.step),
                "the marked step must be `{}`: {}",
                frame.step,
                marked[0].trim()
            );
        });
    }

    /// Nothing resembling a measurement before one exists, and nothing stale
    /// after the inputs that produced it change.
    #[test]
    fn the_measurements_pane_shows_nothing_that_is_not_currently_true() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        assert!(!session.frame().plain().contains("d/d"));
        spring(&mut session);
        assert!(!session.frame().plain().contains("d/d"), "still no run");
        typed(&mut session, &[Event::Enter, Event::Enter]);
        assert_eq!(session.state(), State::Measured);
        let measured = session.frame().plain();
        for partial in ["d/dk", "d/dc"] {
            assert!(measured.contains(partial), "{measured}");
        }
        // Back to the fields, change one, and the number that described the
        // old one is gone rather than left on the screen.
        typed(&mut session, &[Event::Esc, text("0")]);
        let after = session.frame().plain();
        assert!(
            !after.contains("d/dk"),
            "a stale partial survived:\n{after}"
        );
        assert_eq!(session.state(), State::Proposed);
    }

    /// Neither a typed value nor a measured number is ever cut short: the
    /// layout marks any clip, so a frame with no mark has lost nothing.
    #[test]
    fn nothing_is_quietly_cut_short() {
        for (width, height) in [(80, 24), (120, 40)] {
            every_frame(width, height, |frame| {
                let drawn = frame.plain();
                assert!(!drawn.contains('\u{2026}'), "{width}x{height}:\n{drawn}");
            });
        }
        // And the four values of the spring problem survive whole.
        let mut session = Session::new(DEFAULT_SAVE, None);
        spring(&mut session);
        session.apply(Event::Enter);
        let run = session.frame().plain();
        for value in ["12", "1.5", "N/m", "N*s/m"] {
            assert!(run.contains(value), "`{value}` missing from:\n{run}");
        }
    }

    #[test]
    fn a_diagnostic_reaches_the_transcript_on_a_line_a_script_can_read() {
        let mut session = Session::new(DEFAULT_SAVE, None);
        typed(&mut session, &[text("func f(x) = +"), Event::Enter]);
        let frame = session.frame();
        let drawn = frame.plain();
        // In the pane, wrapped to the pane.
        assert!(drawn.contains("Problem: "), "{drawn}");
        // And under it, whole, for a reader that greps rather than looks.
        let plain = drawn
            .lines()
            .find_map(|l| l.strip_prefix("Next step:"))
            .expect("an unwrapped remedy under the frame");
        assert!(plain.trim().len() >= 8, "{drawn}");
    }

    #[test]
    fn the_described_layout_names_the_four_panes_and_the_five_steps() {
        let geometry = Geometry::default();
        let steps = ORDER
            .iter()
            .map(|step| (step.label(), step.keys().to_vec()))
            .collect();
        let described = layout::describe(&geometry, steps);
        assert_eq!(
            described.path_string("schema").as_deref(),
            Some("catalyst.tui-layout.v1")
        );
        let panes = described
            .get("panes")
            .and_then(Json::as_arr)
            .expect("panes");
        assert_eq!(panes.len(), 4);
        let listed: Vec<&str> = described
            .get("steps")
            .and_then(Json::as_arr)
            .expect("steps")
            .iter()
            .filter_map(|step| step.get("name").and_then(Json::as_str))
            .collect();
        assert_eq!(listed, ["goal", "inputs", "run", "results", "export"]);
    }

    /// A step may not advertise a key it ignores. `esc` on Goal is the case
    /// that matters: it is the first step, so going back does nothing.
    #[test]
    fn no_step_names_a_key_it_ignores() {
        assert!(!Step::Goal.keys().contains(&"esc"));
        assert!(!Step::Run.keys().contains(&"tab"));
        assert!(!Step::Results.keys().contains(&"backspace"));
        for step in ORDER {
            for always in ["enter", "ctrl-s", "ctrl-c"] {
                assert!(step.keys().contains(&always), "{}", step.label());
            }
            // The legend draws exactly the keys the table names.
            let drawn: Vec<&str> = step.legend().iter().map(|(key, _, _)| *key).collect();
            for key in step.keys() {
                assert!(
                    drawn.iter().any(|shown| shown.contains(key)),
                    "{} draws no `{key}`: {drawn:?}",
                    step.label()
                );
            }
        }
    }

    #[test]
    fn a_column_of_numbers_lines_up_on_the_decimal_point() {
        let laid = columns(
            &[Align::Left, Align::Decimal],
            &[
                vec!["value".to_owned(), "55.156".to_owned()],
                vec!["d/dk".to_owned(), "1.51744".to_owned()],
                vec!["d/dc".to_owned(), "-27.8".to_owned()],
            ],
        );
        let points: Vec<usize> = laid
            .iter()
            .map(|row| row[1].find('.').expect("a decimal point"))
            .collect();
        assert_eq!(points, vec![points[0]; 3], "{laid:?}");
        for row in &laid {
            assert_eq!(cells(&row[0]), 5, "the label column is one width");
            assert_eq!(cells(&row[1]), cells(&laid[0][1]), "so is the number");
        }
    }
}
