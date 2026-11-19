//! `gliffherdr list`: the machines, best first, as a table or JSON.

use std::sync::Arc;

use serde_json::{Value, json};

use super::check::run_checks;
use super::{CliError, Io, ListArgs, load_config};
use crate::check::{CheckResult, is_fresh, load_cached};
use crate::hypr::open_targets;
use crate::paths::Paths;
use crate::roster::{Machine, OsHint, collect};
use crate::status::{
    RowState, Tone, glyph, os_name, rank, row_state, short_text, status_detail, tone,
};
use crate::sys::Sys;
use crate::text::{pad, truncate, width};

/// Column caps, as in the picker's list.
const LABEL_MAX: usize = 14;
const TARGET_MAX: usize = 30;
const STATUS_MAX: usize = 22;

/// Run `list`.
pub fn run(args: &ListArgs, sys: &Arc<dyn Sys>, io: &mut Io<'_>) -> Result<(), CliError> {
    let paths = Paths::from_sys(sys.as_ref());
    let config = load_config(&paths)?;
    let roster = collect(sys.as_ref(), &paths, &config);
    for w in &roster.warnings {
        let _ = writeln!(io.err, "gliffherdr: {w}");
    }

    if args.check || args.check_stale {
        let now = sys.now_unix();
        let todo: Vec<Machine> = roster
            .machines
            .iter()
            .filter(|m| m.checkable())
            .filter(|m| {
                args.check
                    || load_cached(&paths, &m.target)
                        .is_none_or(|r| !is_fresh(&r, now, config.check.fresh_for_secs))
            })
            .cloned()
            .collect();
        run_checks(sys, &paths, &config, &todo);
    }

    let open = open_targets(sys.as_ref());
    let mut rows: Vec<Row> = roster
        .machines
        .iter()
        .map(|m| {
            let cached = load_cached(&paths, &m.target);
            let is_open = m.is_open(&open);
            let state = row_state(m, cached.as_ref(), false, is_open);
            Row {
                machine: m,
                cached,
                state,
                open: is_open,
            }
        })
        .collect();
    rows.sort_by_key(|r| rank(&r.state)); // stable: ties keep roster order

    if args.json {
        let values: Vec<Value> = rows.iter().map(Row::json).collect();
        let text =
            serde_json::to_string_pretty(&values).map_err(|e| CliError::failed(e.to_string()))?;
        let _ = writeln!(io.out, "{text}");
    } else {
        let table: Vec<TableRow> = rows.iter().map(Row::table_row).collect();
        let _ = write!(io.out, "{}", render_table(&table, io.color));
        if table.is_empty() {
            let _ = writeln!(
                io.err,
                "gliffherdr: no machines found; add some with `gliffherdr config init`"
            );
        }
    }
    Ok(())
}

struct Row<'a> {
    machine: &'a Machine,
    cached: Option<CheckResult>,
    state: RowState,
    open: bool,
}

/// The state name of `list --json`.
fn state_name(s: &RowState) -> &'static str {
    match s {
        RowState::Open => "open",
        RowState::Ready => "ready",
        RowState::Checking => "checking",
        RowState::NotChecked => "not_checked",
        RowState::NeedsSetup(_) => "needs_setup",
        RowState::Unreachable(_) => "unreachable",
        RowState::Unsupported(_) => "unsupported",
    }
}

impl Row<'_> {
    fn json(&self) -> Value {
        let m = self.machine;
        let c = self.cached.as_ref();
        let detail = c
            .map(|c| status_detail(&c.status))
            .filter(|d| !d.is_empty());
        let os = c.and_then(|c| c.os.clone()).or_else(|| match &m.os_hint {
            Some(OsHint::MacOs) => Some("macOS".to_string()),
            Some(OsHint::Windows) => Some("Windows".to_string()),
            Some(OsHint::Linux) => Some("Linux".to_string()),
            Some(OsHint::Other(s)) => Some(os_name(s)),
            None => None,
        });
        json!({
            "target": m.target.as_str(),
            "label": m.label,
            "sources": m.sources.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "state": state_name(&self.state),
            "status": c.map(|c| &c.status),
            "detail": detail,
            "os": os,
            "outputs": c.map_or_else(Vec::new, |c| c.outputs.clone()),
            "server_version": c.and_then(|c| c.server_version.clone()),
            "checked_at": c.map(|c| c.checked_at),
            "window_open": self.open,
        })
    }

    fn table_row(&self) -> TableRow {
        TableRow {
            state: self.state.clone(),
            label: self.machine.label.clone().unwrap_or_default(),
            target: self.machine.target.to_string(),
            sources: self
                .machine
                .sources
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// One line of the `list` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRow {
    /// The row state (glyph, tone and status text come from it).
    pub state: RowState,
    /// The label column.
    pub label: String,
    /// The target column.
    pub target: String,
    /// The sources, space separated.
    pub sources: String,
}

fn paint(tone: Tone, text: &str, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let code = match tone {
        Tone::Good => "32",
        Tone::Accent => "36",
        Tone::Busy | Tone::Warn => "33",
        Tone::Bad => "31",
        Tone::Muted => "2",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// Render the table: glyph, label (up to 14 columns), target (30), status (22), sources.
/// Columns are as wide as their widest cell up to the cap; colour only when `color`.
pub fn render_table(rows: &[TableRow], color: bool) -> String {
    let col = |f: fn(&TableRow) -> String, cap: usize| {
        rows.iter()
            .map(|r| width(&f(r)))
            .max()
            .unwrap_or(0)
            .min(cap)
    };
    let label_w = col(|r| r.label.clone(), LABEL_MAX);
    let target_w = col(|r| r.target.clone(), TARGET_MAX);
    let status_w = col(|r| short_text(&r.state), STATUS_MAX);
    let mut out = String::new();
    for r in rows {
        let t = tone(&r.state);
        let status = pad(&truncate(&short_text(&r.state), STATUS_MAX), status_w);
        let line = format!(
            "  {} {}  {}  {}  {}",
            paint(t, glyph(&r.state, 0), color),
            pad(&r.label, label_w),
            pad(&r.target, target_w),
            paint(t, &status, color),
            paint(Tone::Muted, &r.sources, color && !r.sources.is_empty()),
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::Status;

    fn row(state: RowState, label: &str, target: &str, sources: &str) -> TableRow {
        TableRow {
            state,
            label: label.into(),
            target: target.into(),
            sources: sources.into(),
        }
    }

    #[test]
    fn table_matches_the_spec_example() {
        let text = render_table(
            &[
                row(RowState::Ready, "j", "j@10.0.0.2", "herdr"),
                row(
                    RowState::Unsupported("macOS".into()),
                    "air",
                    "air.tail.ts.net",
                    "tailscale",
                ),
            ],
            false,
        );
        assert_eq!(
            text,
            "  ● j    j@10.0.0.2       ready                  herdr\n  – air  air.tail.ts.net  macOS: needs Hyprland  tailscale\n"
        );
    }

    #[test]
    fn a_machine_without_a_label_leaves_the_label_column_blank() {
        let m = crate::roster::bare_machine(
            &crate::sys::fake::FakeSys::new(),
            &crate::config::Config::default(),
            crate::target::Target::parse("lab@10.0.0.9").unwrap(),
        );
        let r = Row {
            machine: &m,
            cached: None,
            state: RowState::NotChecked,
            open: false,
        };
        assert_eq!(r.table_row().label, "");
        let text = render_table(
            &[
                row(RowState::NotChecked, "", "lab@10.0.0.9", "ssh"),
                row(RowState::NotChecked, "j", "j@10.0.0.2", "herdr"),
            ],
            false,
        );
        assert_eq!(
            text,
            "  ○    lab@10.0.0.9  not checked  ssh\n  ○ j  j@10.0.0.2    not checked  herdr\n"
        );
    }

    #[test]
    fn columns_are_capped_and_truncated() {
        let long = "a-very-long-label-indeed";
        let text = render_table(
            &[row(
                RowState::Unreachable(Status::Error {
                    detail: "x".repeat(40),
                }),
                long,
                &"t".repeat(50),
                "ssh",
            )],
            false,
        );
        let line = text.lines().next().unwrap();
        assert!(line.contains(&format!("{}…", &long[..13])), "{line}");
        assert!(line.contains(&format!("{}…", "t".repeat(29))), "{line}");
        assert!(line.contains(&format!("{}…", "x".repeat(21))), "{line}");
        assert!(line.ends_with("  ssh"));
    }

    #[test]
    fn colour_wraps_glyph_and_status_in_the_tone() {
        let text = render_table(&[row(RowState::Ready, "j", "j@h", "herdr")], true);
        assert!(text.contains("\x1b[32m●\x1b[0m"), "{text:?}");
        assert!(text.contains("\x1b[32mready\x1b[0m"));
        assert!(text.contains("\x1b[2mherdr\x1b[0m"));
        let plain = render_table(&[row(RowState::Ready, "j", "j@h", "herdr")], false);
        assert!(!plain.contains('\x1b'));
    }

    #[test]
    fn empty_table_is_empty() {
        assert_eq!(render_table(&[], false), "");
    }

    #[test]
    fn state_names_are_the_documented_snake_case() {
        let all = [
            (RowState::Open, "open"),
            (RowState::Ready, "ready"),
            (RowState::Checking, "checking"),
            (RowState::NotChecked, "not_checked"),
            (RowState::NeedsSetup(Status::Auth), "needs_setup"),
            (RowState::Unreachable(Status::Auth), "unreachable"),
            (RowState::Unsupported("x".into()), "unsupported"),
        ];
        for (s, n) in all {
            assert_eq!(state_name(&s), n);
        }
    }
}
