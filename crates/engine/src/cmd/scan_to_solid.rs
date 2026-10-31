//! The Scan To Solid tab (scan-to-solid fork): a scan turned into a parametric solid by the
//! scan-to-solid scripts (github.com/stanislav-hladyshko, `scripts/solidcraft_run.py`). Each command
//! is one of their actions; the engine only names it. Its command dialog (the UI's `scan_to_solid`)
//! runs the script off the window thread, because the script works on the open design through the
//! control channel itself.

use serde_json::{Value, json};

use super::CommandSpec;
use crate::{Result, Session};

/// The ribbon tab, and its panels in the order the work goes on any part (ADR 0027 of the
/// scan-to-solid repository): prepare the scan, recognise it, model the body, review it, output it;
/// the commands that need a part's build script last.
pub const TAB: &str = "SCAN TO SOLID";
pub const PANEL_PREPARE: &str = "PREPARE";
pub const PANEL_RECOGNISE: &str = "RECOGNISE";
pub const PANEL_MODEL: &str = "MODEL";
pub const PANEL_REVIEW: &str = "REVIEW";
pub const PANEL_OUTPUT: &str = "OUTPUT";
pub const PANEL_KNOWN: &str = "KNOWN PART";

/// Every command id starts with this; the rest is the script's action name.
pub const PREFIX: &str = "scan.";

const PARAMS: &str = "no parameters → action; the command dialog runs the action with its fields and picks";

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.lay_scan", "Lay Scan", action).at(TAB, PANEL_PREPARE).icon("ground").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.recognise", "Recognise", action).at(TAB, PANEL_RECOGNISE).icon("inspect").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.panel", "Panel", action).at(TAB, PANEL_MODEL).icon("extrude").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.stands", "Walls & Clips", action).at(TAB, PANEL_MODEL).icon("rib").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.round", "Round", action).at(TAB, PANEL_MODEL).icon("fillet").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.section", "Section", action).at(TAB, PANEL_MODEL).icon("fit_section").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.layers", "Layer Copy", action).at(TAB, PANEL_MODEL).icon("update").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.check", "Check", action).at(TAB, PANEL_REVIEW).icon("compare").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.accept", "Accept", action).at(TAB, PANEL_REVIEW).icon("finish").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.reject", "Reject", action).at(TAB, PANEL_REVIEW).icon("delete").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.measured", "As Measured", action).at(TAB, PANEL_REVIEW).icon("revert").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.feature", "Is Feature", action).at(TAB, PANEL_REVIEW).icon("select").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.print", "To Print", action).at(TAB, PANEL_OUTPUT).icon("export").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.load_test", "Load Test", action).at(TAB, PANEL_OUTPUT).icon("compute").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.animate", "Animate", action).at(TAB, PANEL_OUTPUT).icon("play").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.intend", "Intent", action).at(TAB, PANEL_KNOWN).icon("edit").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.repair", "Repair", action).at(TAB, PANEL_KNOWN).icon("mirror").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.modify", "Modify", action).at(TAB, PANEL_KNOWN).icon("params").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.ignore", "Scan Gap", action).at(TAB, PANEL_KNOWN).icon("suppress").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.keep", "As Intended", action).at(TAB, PANEL_KNOWN).icon("checkpoint").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.build", "Build", action).at(TAB, PANEL_KNOWN).icon("update").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.remeasure", "Remeasure", action).at(TAB, PANEL_KNOWN).icon("inspect").noundo().doc_only().params(PARAMS),
    CommandSpec::new("scan.stronger", "Stronger", action).at(TAB, PANEL_KNOWN).icon("rib").noundo().doc_only().params(PARAMS),
];

/// The script's action a command id names, if it is one of ours.
pub fn action_of(id: &str) -> Option<&str> {
    id.strip_prefix(PREFIX).filter(|_| COMMANDS.iter().any(|c| c.id == id))
}

/// Run from a script, MCP or the palette the command only names its action: its command dialog,
/// which holds the fields and the picks, is what runs it.
fn action(_s: &mut Session, _p: &Value) -> Result<Value> {
    Ok(json!({"dialog": "scan_to_solid"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_to_solid_every_command_names_an_action_on_the_tab() {
        for command in COMMANDS {
            assert_eq!(command.tab, TAB, "{}", command.id);
            assert!(action_of(command.id).is_some_and(|a| !a.is_empty() && !a.contains('.')), "{}", command.id);
        }
        assert_eq!(action_of("scan.nothing"), None);
        assert_eq!(action_of("solid.extrude"), None);
    }
}
