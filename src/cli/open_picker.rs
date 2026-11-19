//! `gliffherdr open-picker`: ask herdr to open the picker popup.

use super::{CliError, Io};
use crate::herdr::Herdr;
use crate::sys::Sys;

/// The title of every notification gliffherdr shows.
pub const NOTIFY_TITLE: &str = "Remote desktop";

/// Ask herdr to open the plugin's `picker` pane. A refusal (`ui_busy` while a modal is open, a
/// plugin that is not linked) is shown as a herdr notification, because an action has no
/// terminal, and fails the command.
pub fn run(sys: &dyn Sys, _io: &mut Io<'_>) -> Result<(), CliError> {
    let herdr = Herdr::from_sys(sys);
    herdr
        .open_pane(sys, "picker", None, &[], false)
        .map_err(|e| {
            let _ = herdr.notify(sys, NOTIFY_TITLE, &e.to_string());
            CliError::failed(e.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::fake::FakeSys;

    fn run_it(fake: &FakeSys) -> (Result<(), CliError>, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let r = run(
            fake,
            &mut Io {
                out: &mut out,
                err: &mut err,
                color: false,
            },
        );
        (r, String::from_utf8(err).unwrap())
    }

    #[test]
    fn opens_the_picker_pane() {
        let fake = FakeSys::new();
        fake.set_env("HERDR_BIN_PATH", "/h");
        fake.set_env("HERDR_PLUGIN_ID", "me.test");
        fake.on("/h", &[]).stdout("{}");
        let (r, _) = run_it(&fake);
        assert_eq!(r, Ok(()));
        assert_eq!(
            fake.call_lines(),
            ["/h plugin pane open --plugin me.test --entrypoint picker"]
        );
    }

    #[test]
    fn a_refusal_becomes_a_notification_and_exit_1() {
        let fake = FakeSys::new();
        fake.set_env("HERDR_BIN_PATH", "/h");
        fake.on("/h", &["plugin"])
            .code(1)
            .stderr(r#"{"error":{"code":"ui_busy","message":"a popup pane is already open"}}"#);
        fake.on("/h", &["notification"]).stdout("");
        let (r, _) = run_it(&fake);
        let e = r.unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (1, "a popup pane is already open")
        );
        assert_eq!(
            fake.calls_to("/h")[1].args,
            [
                "notification",
                "show",
                "Remote desktop",
                "--body",
                "a popup pane is already open"
            ]
        );
    }
}
