//! The SCAN TO SOLID tab's actions as command dialogs (scan-to-solid fork, ADR 0019 there).
//!
//! A tab button (`scan.<action>`) opens a command dialog like any other command's
//! ([`crate::dialogs`]): the selection inputs and the fields `solidcraft_run.py describe` gives that
//! action, so a new action is a change in the scripts, not here. OK runs the action's script with
//! the fields and the picks; what it says goes to the status line and Messages. The actions are the
//! scan-to-solid scripts (`scripts/solidcraft_run.py` in the folder `SCAN_TO_SOLID_HOME`, default
//! [`DEFAULT_HOME`]). A run is a child process read on its own threads: the script changes the open
//! design through the control channel, which the window thread serves, so the window never waits on
//! it. Each line the script writes is JSON: `say` while it works, then `answer`, `refused` or `error`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value, json};

use crate::SolveApp;
use crate::dialogs::{Dialog, Kind};
use crate::selection::{self, Accept, SelInput};
use crate::ui_kit;

/// Where the scan-to-solid folder is when `SCAN_TO_SOLID_HOME` does not say.
pub const DEFAULT_HOME: &str = r"C:\Repo\hardware\scan-to-solid";
const HOME_VAR: &str = "SCAN_TO_SOLID_HOME";
const RUNNER: &str = "scripts/solidcraft_run.py";
/// The field the script reads the dialog's picks from (`solidcraft_run.py` `SELECTION`).
const SELECTION: &str = "selection";
/// Lines of a script's error output kept to show when it fails.
const STDERR_LINES: usize = 20;
/// Windows: start the script without a console window of its own.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One line from a running script.
enum Line {
    Out(String),
    Err(String),
}

/// A script running off the window thread.
struct Job {
    child: Child,
    lines: Receiver<Line>,
    /// What the status line says it is running ("Load Test").
    label: String,
    /// The latest thing it said, in this frame or an earlier one, was a last word (`answer`,
    /// `refused` or `error`).
    answered: bool,
}

impl Job {
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// The lines written since the last frame, and whether the script is done: both its streams
    /// read to the end. A process that has exited may still have lines on their way, so its exit
    /// alone does not end the job (its last word would be lost).
    fn read(&mut self) -> (Vec<String>, Vec<String>, bool) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        loop {
            match self.lines.try_recv() {
                Ok(Line::Out(text)) => out.push(text),
                Ok(Line::Err(text)) => err.push(text),
                Err(TryRecvError::Empty) => return (out, err, false),
                Err(TryRecvError::Disconnected) => {
                    let _ = self.child.wait();
                    return (out, err, true);
                }
            }
        }
    }
}

/// A field's value as the person left it.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Text(String),
    Choice(usize),
    Check(bool),
}

/// The tab's state: the scripts' panel, the fields as the person left them (kept from one dialog to
/// the next), and the scripts running now.
#[derive(Default)]
pub struct ScanToSolid {
    /// `describe`'s panel, once read.
    spec: Option<Value>,
    pub fields: HashMap<String, Field>,
    /// A command asked for before the panel was read; its dialog opens once it is.
    pending: Option<String>,
    describing: Option<Job>,
    /// Why `describe` could not run or answer, said when a button is pressed.
    unavailable: Option<String>,
    running: Option<Job>,
    /// A watch check's job: its action again after every design change.
    watching: Option<Job>,
    stderr: Vec<String>,
    /// The window's context, which a script's lines wake (kept by [`poll`] for OK, which has none).
    ctx: Option<egui::Context>,
}

/// The scan-to-solid folder.
fn home() -> PathBuf {
    std::env::var_os(HOME_VAR).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_HOME))
}

fn python(home: &Path) -> PathBuf {
    if cfg!(windows) { home.join(".venv").join("Scripts").join("python.exe") } else { home.join(".venv").join("bin").join("python") }
}

/// Start the runner with `args`; its lines arrive on the job's channel and wake the window.
fn spawn(args: &[String], label: &str, ctx: &egui::Context) -> Result<Job, String> {
    let home = home();
    let python = python(&home);
    if !python.is_file() || !home.join(RUNNER).is_file() {
        return Err(format!(
            "Scan To Solid needs the scan-to-solid folder with its .venv, not found at {}. Set {HOME_VAR} to where it is.",
            home.display()
        ));
    }
    let mut command = Command::new(&python);
    command.arg("-u").arg(RUNNER).args(args).current_dir(&home).env("PYTHONIOENCODING", "utf-8");
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| format!("Could not start {}: {e}", python.display()))?;
    let (tx, rx) = channel();
    if let Some(out) = child.stdout.take() {
        read_lines(out, tx.clone(), Line::Out, ctx.clone());
    }
    if let Some(err) = child.stderr.take() {
        read_lines(err, tx, Line::Err, ctx.clone());
    }
    Ok(Job { child, lines: rx, label: label.to_owned(), answered: false })
}

fn read_lines<R: std::io::Read + Send + 'static>(stream: R, tx: Sender<Line>, wrap: fn(String) -> Line, ctx: egui::Context) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { return };
            if tx.send(wrap(line)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    });
}

/// What a script's line says: its text, whether it is the run's last word, and whether that word
/// is a refusal or an error.
#[derive(Debug, PartialEq)]
struct Said {
    text: String,
    last: bool,
    failed: bool,
}

fn said(line: &str) -> Option<Said> {
    let value: Value = serde_json::from_str(line).ok()?;
    if let Some(text) = value.get("say").and_then(Value::as_str) {
        return Some(Said { text: text.to_owned(), last: false, failed: false });
    }
    for key in ["answer", "refused", "error"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            let text = if key == "error" { format!("Error: {text}") } else { text.to_owned() };
            return Some(Said { text, last: true, failed: key != "answer" });
        }
    }
    None
}

/// Every control of the spec, in order.
fn controls(spec: &Value) -> impl Iterator<Item = &Value> {
    spec.as_array().into_iter().flatten().flat_map(|g| g["rows"].as_array().into_iter().flatten()).flat_map(|r| r.as_array().into_iter().flatten())
}

/// The button of `action`, and the fields its dialog shows: the other controls of its row, then
/// those of its group's rows without a button (Force and Pulls belong to both Load Test and Stronger).
fn action_spec<'a>(spec: &'a Value, action: &str) -> Option<(&'a Value, Vec<&'a Value>)> {
    for group in spec.as_array().into_iter().flatten() {
        let rows: Vec<&Vec<Value>> = group["rows"].as_array().into_iter().flatten().filter_map(Value::as_array).collect();
        let is_button = |c: &Value| c["kind"].as_str() == Some("button");
        let Some(row) = rows.iter().find(|row| row.iter().any(|c| is_button(c) && c["action"].as_str() == Some(action))) else { continue };
        let button = row.iter().find(|c| is_button(c) && c["action"].as_str() == Some(action))?;
        let mut fields: Vec<&Value> = row.iter().filter(|c| !is_button(c)).collect();
        fields.extend(rows.iter().filter(|r| !r.iter().any(is_button)).flat_map(|r| r.iter()));
        return Some((button, fields));
    }
    None
}

/// A control's starting value; a button has none.
fn default_field(control: &Value) -> Option<Field> {
    match control["kind"].as_str()? {
        "choice" => {
            let default = control["default"].as_str().unwrap_or_default();
            let index = control["options"].as_array()?.iter().position(|o| o[0].as_str() == Some(default)).unwrap_or(0);
            Some(Field::Choice(index))
        }
        "number" => Some(Field::Text(match &control["default"] {
            Value::Number(n) => n.as_f64().map(|v| format!("{v}")).unwrap_or_default(),
            Value::String(s) => s.clone(),
            _ => String::new(),
        })),
        "check" => Some(Field::Check(control["default"].as_bool().unwrap_or(false))),
        _ => None,
    }
}

/// What a selection input of the spec takes.
fn accept_of(names: &Value) -> Accept {
    names.as_array().into_iter().flatten().filter_map(Value::as_str).fold(0, |accept, name| {
        accept
            | match name {
                "faces" => selection::FACES,
                "features" => selection::FEATURES,
                "bodies" => selection::BODIES,
                "edges" => selection::EDGES,
                "vertices" => selection::VERTICES,
                _ => 0,
            }
    })
}

/// A label from the scripts as the `&'static str` a [`SelInput`] holds: each distinct label is
/// kept once for the life of the app (a few dozen words at most).
fn kept(label: &str) -> &'static str {
    static KEPT: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut kept = KEPT.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    kept.entry(label.to_owned()).or_insert_with(|| Box::leak(label.to_owned().into_boxed_str()))
}

/// The row label of a field: its label, with its unit.
fn row_label(control: &Value) -> String {
    let label = control["label"].as_str().filter(|l| !l.is_empty()).or_else(|| control["name"].as_str()).unwrap_or_default();
    match control["unit"].as_str().filter(|u| !u.is_empty()) {
        Some(unit) => format!("{label} ({unit})"),
        None => label.to_owned(),
    }
}

impl ScanToSolid {
    /// The fields `action` takes, as the script reads them: a choice by its key, a number as typed.
    fn values(&self, action: &str) -> Map<String, Value> {
        let mut out = Map::new();
        let Some((_, fields)) = self.spec.as_ref().and_then(|spec| action_spec(spec, action)) else { return out };
        for control in fields {
            let Some(name) = control["name"].as_str() else { continue };
            let value = match self.fields.get(name) {
                Some(Field::Text(t)) => json!(t),
                Some(Field::Check(b)) => json!(b),
                Some(Field::Choice(i)) => control["options"].get(*i).and_then(|o| o.get(0)).cloned().unwrap_or(Value::Null),
                None => continue,
            };
            out.insert(name.to_owned(), value);
        }
        out
    }

    fn label_of(&self, action: &str) -> String {
        self.spec
            .as_ref()
            .and_then(|spec| action_spec(spec, action))
            .and_then(|(button, _)| button["label"].as_str().map(str::to_owned))
            .unwrap_or_else(|| action.to_owned())
    }

    /// What OK on `action` does to the watch: `Some(true)` starts it (Check with Live on),
    /// `Some(false)` stops it (Check with Live off), `None` leaves it running (any other action: a
    /// change it makes is what the watch is there to check).
    fn watch_change(&self, action: &str) -> Option<bool> {
        let (_, fields) = self.spec.as_ref().and_then(|spec| action_spec(spec, action))?;
        let live = fields.iter().find(|c| c["watch"].as_str() == Some(action))?;
        Some(live["name"].as_str().and_then(|name| self.fields.get(name)) == Some(&Field::Check(true)))
    }

    /// The selection inputs of `action`'s dialog, in the order the script reads the picks.
    fn inputs(&self, action: &str) -> Vec<SelInput> {
        let Some((button, _)) = self.spec.as_ref().and_then(|spec| action_spec(spec, action)) else { return Vec::new() };
        button["select"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|pick| {
                let many = pick["many"].as_bool().unwrap_or(false);
                let input = SelInput::new(kept(pick["label"].as_str().unwrap_or_default()), accept_of(&pick["accept"]), many);
                // several places on one face add up: points drawn on the scan or on a body's face
                if many { input.places() } else { input }
            })
            .collect()
    }

    /// Take the panel from `describe`'s line.
    fn described(&mut self, line: &str) {
        let Ok(value) = serde_json::from_str::<Value>(line) else { return };
        let Some(panel) = value.get("panel") else { return };
        for control in controls(panel) {
            if let (Some(name), Some(field)) = (control["name"].as_str(), default_field(control)) {
                self.fields.entry(name.to_owned()).or_insert(field);
            }
        }
        self.spec = Some(panel.clone());
    }

    fn keep_stderr(&mut self, lines: Vec<String>) {
        self.stderr.extend(lines);
        let extra = self.stderr.len().saturating_sub(STDERR_LINES);
        self.stderr.drain(..extra);
    }

    /// Read the panel once, in the background, so a button's dialog opens at once.
    fn describe(&mut self, ctx: &egui::Context) {
        if self.spec.is_some() || self.describing.is_some() || self.unavailable.is_some() {
            return;
        }
        match spawn(&["describe".to_owned()], "Scan To Solid", ctx) {
            Ok(job) => self.describing = Some(job),
            Err(why) => self.unavailable = Some(why),
        }
    }

    /// Read what the jobs wrote since the last frame; what a run says, in the order said.
    fn poll(&mut self) -> Vec<(String, bool)> {
        if let Some(job) = &mut self.describing {
            let (out, err, done) = job.read();
            for line in out {
                self.described(&line);
            }
            self.keep_stderr(err);
            if done {
                self.describing = None;
                if self.spec.is_none() {
                    self.unavailable = Some(format!("Could not read the Scan To Solid actions: {}", self.stderr.join("\n")));
                }
            }
        }
        let mut status_lines = Vec::new();
        for watch in [false, true] {
            let slot = if watch { &mut self.watching } else { &mut self.running };
            let Some(job) = slot else { continue };
            let (out, err, done) = job.read();
            let label = job.label.clone();
            let lines: Vec<Said> = out.iter().filter_map(|text| said(text)).collect();
            // the last word and the end of the streams may be read in different frames; a watch's
            // next run says more, and is unanswered again until it ends
            if let Some(line) = lines.last() {
                job.answered = line.last;
            }
            let answered = job.answered;
            if done {
                *slot = None;
            }
            self.keep_stderr(err);
            for line in lines {
                status_lines.push((if line.last { format!("{label}: {}", line.text) } else { format!("{label}… {}", line.text) }, line.failed));
            }
            // a run that ended without its last word crashed before it could say why; a watch ends
            // only when turned off, which drops it before this, so its end is always said
            if done && (watch || !answered) {
                let what = if watch { "stopped" } else { "stopped without an answer" };
                let why = self.stderr.join("\n");
                status_lines.push((if why.is_empty() { format!("{label} {what}") } else { format!("{label} {what}: {why}") }, true));
            }
        }
        status_lines
    }

    /// Turn `action`'s watch on (the action again after every design change) or off.
    fn set_watch(&mut self, action: Option<&str>, fields: &Map<String, Value>, ctx: &egui::Context) -> Result<(), String> {
        if let Some(job) = &mut self.watching {
            job.stop();
        }
        self.watching = None;
        let Some(action) = action else { return Ok(()) };
        let label = format!("Live {}", self.label_of(action));
        self.watching = Some(spawn(&["watch".to_owned(), action.to_owned(), Value::Object(fields.clone()).to_string()], &label, ctx)?);
        Ok(())
    }

    /// Stop every script: the app is closing.
    pub fn shutdown(&mut self) {
        for job in [&mut self.describing, &mut self.running, &mut self.watching].into_iter().flatten() {
            job.stop();
        }
    }
}

/// The dialog kind and selection inputs of a tab command (`for_command`), once the actions are read.
pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let action = solidcraft_engine::scan_to_solid_action(id)?;
    app.scan_to_solid.spec.as_ref()?;
    let title = solidcraft_engine::find_command(id).map_or("Scan To Solid", |c| c.label);
    Some((Kind::Scan { action: action.to_owned(), title }, app.scan_to_solid.inputs(action)))
}

/// A tab button pressed before the actions are read: the dialog opens once they are, and the
/// status line says why it has not yet. False when the dialog can open now.
pub fn waits(app: &mut SolveApp, id: &str) -> bool {
    if solidcraft_engine::scan_to_solid_action(id).is_none() || app.scan_to_solid.spec.is_some() {
        return false;
    }
    match app.scan_to_solid.unavailable.take() {
        Some(why) => app.set_status(why, true),
        None => {
            app.scan_to_solid.pending = Some(id.to_owned());
            app.set_status("Scan To Solid is reading its actions…", false);
        }
    }
    true
}

/// Each frame: read the actions once, open a dialog that waited for them, and say what the
/// scripts say on the status line.
pub fn poll(app: &mut SolveApp, ctx: &egui::Context) {
    app.scan_to_solid.ctx = Some(ctx.clone());
    app.scan_to_solid.describe(ctx);
    for (text, error) in app.scan_to_solid.poll() {
        app.set_status(text, error);
    }
    if app.scan_to_solid.spec.is_some()
        && let Some(id) = app.scan_to_solid.pending.take()
    {
        app.start(&id);
    } else if app.scan_to_solid.pending.is_some()
        && let Some(why) = app.scan_to_solid.unavailable.clone()
    {
        app.scan_to_solid.pending = None;
        app.set_status(why, true);
    }
}

/// The dialog's fields; true when Enter was pressed in one.
pub fn rows(ui: &mut egui::Ui, panel: &mut ScanToSolid, action: &str) -> bool {
    let mut enter = false;
    let Some(spec) = panel.spec.clone() else { return false };
    let Some((_, fields)) = action_spec(&spec, action) else { return false };
    for control in fields {
        let name = control["name"].as_str().unwrap_or_default();
        let label = row_label(control);
        match (control["kind"].as_str().unwrap_or_default(), panel.fields.get_mut(name)) {
            ("choice", Some(Field::Choice(index))) => {
                let options: Vec<&str> = control["options"].as_array().into_iter().flatten().filter_map(|o| o[1].as_str()).collect();
                ui_kit::choice_row(ui, &label, &format!("scan_to_solid_{name}"), index, &options);
            }
            // (enter, changed): only Enter is OK
            ("number", Some(Field::Text(text))) => enter |= ui_kit::text_row(ui, &label, text, "").0,
            ("check", Some(Field::Check(on))) => {
                ui_kit::checkbox(ui, &label, on);
            }
            _ => {}
        }
    }
    enter
}

/// OK: run the dialog's action with its fields and picks (a watched action starts its watch,
/// which runs it once). The dialog closes; the status line follows the run.
pub fn run(app: &mut SolveApp, d: &Dialog) -> Result<(), String> {
    let Kind::Scan { action, .. } = &d.kind else { return Err("not a Scan To Solid dialog".into()) };
    let panel = &mut app.scan_to_solid;
    let label = panel.label_of(action);
    if let Some(job) = &panel.running {
        return Err(format!("{} is still running: press OK again when it answers", job.label));
    }
    let mut fields = panel.values(action);
    let picks: Vec<&solidcraft_engine::Sel> = d.inputs.iter().flat_map(|input| input.items.iter()).collect();
    fields.insert(SELECTION.to_owned(), json!(picks));
    panel.stderr.clear();
    let ctx = panel.ctx.clone().ok_or("Scan To Solid is not ready yet: press OK again")?;
    match panel.watch_change(action) {
        Some(true) => panel.set_watch(Some(action), &fields, &ctx)?,
        change => {
            if change == Some(false) {
                panel.set_watch(None, &fields, &ctx)?;
            }
            panel.running = Some(spawn(&["run".to_owned(), action.clone(), Value::Object(fields).to_string()], &label, &ctx)?);
        }
    }
    app.set_status(format!("{label}…"), false);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIBED: &str = r#"{"panel": [{"title": "Load", "rows": [[{"kind": "number", "name": "force", "default": 100.0, "unit": "N", "label": "Force"}, {"kind": "check", "name": "pull", "label": "Pulls", "default": false}], [{"kind": "button", "action": "load_test", "label": "Load Test", "select": [{"label": "Holds", "accept": ["faces"], "many": false}, {"label": "Force on", "accept": ["faces"], "many": false}]}]]}, {"title": "Trace and check", "rows": [[{"kind": "button", "action": "section", "label": "Section", "select": [{"label": "Point", "accept": ["faces"], "many": false}]}, {"kind": "choice", "name": "axis", "options": [["X", "Plane X"], ["Y", "Plane Y"]], "default": "Y", "label": "Plane"}], [{"kind": "button", "action": "check", "label": "Check", "select": []}, {"kind": "number", "name": "tolerance", "default": 0.5, "unit": "mm", "label": "Tolerance"}, {"kind": "check", "name": "live", "label": "Live", "default": false, "watch": "check"}]]}, {"title": "Place on scan", "rows": [[{"kind": "button", "action": "repair", "label": "Repair", "select": [{"label": "Place", "accept": ["faces", "features"], "many": true}]}]]}], "hint": "Select, then press a button"}"#;

    fn described() -> ScanToSolid {
        let mut panel = ScanToSolid::default();
        panel.described(DESCRIBED);
        panel
    }

    #[test]
    fn scan_to_solid_dialog_takes_its_row_and_the_group_rows_without_a_button() {
        let panel = described();
        assert_eq!(panel.values("load_test"), json!({"force": "100", "pull": false}).as_object().cloned().unwrap());
        assert_eq!(panel.values("section"), json!({"axis": "Y"}).as_object().cloned().unwrap());
        assert_eq!(panel.values("check"), json!({"tolerance": "0.5", "live": false}).as_object().cloned().unwrap());
        assert!(panel.values("nothing").is_empty());
        assert_eq!(panel.label_of("load_test"), "Load Test");
        assert_eq!(panel.label_of("nothing"), "nothing");
    }

    #[test]
    fn scan_to_solid_dialog_asks_for_the_picks_in_the_scripts_order() {
        let panel = described();
        let load: Vec<_> = panel.inputs("load_test").into_iter().map(|i| (i.label, i.accept, i.multi, i.places)).collect();
        assert_eq!(load, [("Holds", selection::FACES, false, false), ("Force on", selection::FACES, false, false)]);
        let repair = panel.inputs("repair");
        assert_eq!((repair[0].label, repair[0].accept, repair[0].multi, repair[0].places), ("Place", selection::FACES | selection::FEATURES, true, true));
        assert!(panel.inputs("check").is_empty());
    }

    #[test]
    fn scan_to_solid_sends_a_choice_by_its_key_and_a_number_as_typed() {
        let mut panel = described();
        panel.fields.insert("axis".into(), Field::Choice(0));
        panel.fields.insert("force".into(), Field::Text("250,5".into()));
        assert_eq!(panel.values("section")["axis"], "X");
        assert_eq!(panel.values("load_test")["force"], "250,5");
    }

    /// Check with Live on starts the watch, with Live off stops it; any other action leaves it
    /// running (Section stopped it, so Live never checked a change made from the tab).
    #[test]
    fn scan_to_solid_only_check_turns_its_watch_on_or_off() {
        let mut panel = described();
        assert_eq!(panel.watch_change("check"), Some(false));
        panel.fields.insert("live".into(), Field::Check(true));
        assert_eq!(panel.watch_change("check"), Some(true));
        assert_eq!(panel.watch_change("section"), None);
        assert_eq!(panel.watch_change("load_test"), None);
    }

    /// A watch ends only when it is turned off, which drops it first: one that ends by itself says
    /// so, even after its answers (it went quiet, and Live looked on).
    #[test]
    fn scan_to_solid_a_watch_that_ends_by_itself_says_so() {
        let mut panel = ScanToSolid { watching: Some(answering_job(false)), ..Default::default() };
        let mut status_lines = Vec::new();
        for _ in 0..200 {
            status_lines.extend(panel.poll());
            if panel.watching.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(status_lines, [("Build: Built".to_owned(), false), ("Build stopped".to_owned(), true)]);
    }

    /// A job whose script writes `{"answer": "Built"}` and exits, after about a second when `lingers`.
    fn answering_job(lingers: bool) -> Job {
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;
            let mut command = Command::new("cmd");
            let wait = if lingers { " & ping -n 2 127.0.0.1 >nul" } else { "" };
            command.raw_arg(format!("/C echo {{\"answer\": \"Built\"}}{wait}"));
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            let wait = if lingers { "; sleep 1" } else { "" };
            command.args(["-c", &format!("echo '{{\"answer\": \"Built\"}}'{wait}")]);
            command
        };
        let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("a shell starts");
        let (tx, rx) = channel();
        read_lines(child.stdout.take().expect("piped"), tx.clone(), Line::Out, egui::Context::default());
        read_lines(child.stderr.take().expect("piped"), tx, Line::Err, egui::Context::default());
        Job { child, lines: rx, label: "Build".into(), answered: false }
    }

    /// A script that wrote its last word and exited at once: the word is read, then the job is done
    /// (an exit seen before the reader passed the line on lost it, and the tab said the actions
    /// could not be read).
    #[test]
    fn scan_to_solid_keeps_the_last_word_of_a_script_that_already_exited() {
        let mut job = answering_job(false);
        let (mut out, mut done) = (Vec::new(), false);
        for _ in 0..200 {
            let (lines, _, finished) = job.read();
            out.extend(lines);
            if finished {
                done = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(done, "the job ends once both streams are read");
        assert_eq!(out.iter().filter_map(|l| said(l)).map(|s| s.text).collect::<Vec<_>>(), ["Built"]);
    }

    /// The last word read one frame and the end of the script a later one: the run answered, it
    /// did not stop without an answer (every run said both before).
    #[test]
    fn scan_to_solid_a_run_that_answered_before_it_ended_did_not_fail() {
        let mut panel = ScanToSolid { running: Some(answering_job(true)), ..Default::default() };
        let mut status_lines = Vec::new();
        for _ in 0..200 {
            status_lines.extend(panel.poll());
            if panel.running.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(panel.running.is_none(), "the run ends");
        assert_eq!(status_lines, [("Build: Built".to_owned(), false)]);
    }

    /// Typing in a number field is not OK: the action runs on OK or Enter (each digit typed ran it,
    /// with the number so far).
    #[test]
    fn scan_to_solid_typing_a_number_does_not_run_the_action() {
        let mut h = crate::scenario::Harness::new();
        h.app.scan_to_solid.described(DESCRIBED);
        h.app.start("scan.load_test");
        h.frames(2);
        let at = h.call("ui.at", json!({"handle": "dialog:Force (N)"}))["result"].clone();
        h.call("ui.click", json!({"x": at[0], "y": at[1]}));
        h.call("ui.text", json!({"text": "2"}));
        assert!(matches!(h.app.scan_to_solid.fields.get("force"), Some(Field::Text(t)) if t.contains('2')), "{:?}", h.app.scan_to_solid.fields);
        let dialog = h.app.dialog.as_ref().expect("the dialog stays open while typing");
        assert_eq!(dialog.error, None, "nothing ran");
        assert!(h.app.scan_to_solid.running.is_none(), "nothing runs while typing");
    }

    #[test]
    fn scan_to_solid_tells_the_last_word_from_progress() {
        assert_eq!(said(r#"{"say": "Building…"}"#), Some(Said { text: "Building…".into(), last: false, failed: false }));
        assert_eq!(said(r#"{"answer": "Built"}"#), Some(Said { text: "Built".into(), last: true, failed: false }));
        assert_eq!(said(r#"{"refused": "Pick two faces"}"#), Some(Said { text: "Pick two faces".into(), last: true, failed: true }));
        assert_eq!(said(r#"{"error": "boom"}"#), Some(Said { text: "Error: boom".into(), last: true, failed: true }));
        assert_eq!(said("not json"), None);
    }

    #[test]
    fn scan_to_solid_ignores_a_line_without_a_panel() {
        let mut panel = ScanToSolid::default();
        panel.described(r#"{"error": "usage"}"#);
        panel.described("garbage");
        assert!(panel.spec.is_none() && panel.fields.is_empty());
    }
}
