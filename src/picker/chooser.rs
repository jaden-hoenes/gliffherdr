//! The screen chooser: which of the remote's screens a new window shows.
//!
//! It is a centred box over the picker (`Ctrl+O`) and, for `connect --choose`, the whole of a
//! terminal of its own. The state is plain data; [`Chooser::draw`] renders it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Clear, Widget};

use super::view::{DIM, put, tone_style};
use crate::launch::{ScreenChoice, valid_output_name};
use crate::status::{RowState, Tone, glyph};
use crate::target::Target;

/// The width of the box.
pub const BOX_WIDTH: u16 = 44;

/// The chooser's state.
#[derive(Debug, Clone)]
pub struct Chooser {
    label: String,
    target: Target,
    outputs: Option<Vec<String>>,
    selected: usize,
}

impl Chooser {
    /// A chooser for a machine whose screens are known.
    #[must_use]
    pub fn ready(label: String, target: Target, outputs: Vec<String>) -> Chooser {
        Chooser {
            label,
            target,
            outputs: Some(outputs),
            selected: 0,
        }
    }

    /// A chooser waiting for a check to say which screens the machine has.
    #[must_use]
    pub fn loading(label: String, target: Target) -> Chooser {
        Chooser {
            label,
            target,
            outputs: None,
            selected: 0,
        }
    }

    /// Whether the check is still running.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        self.outputs.is_none()
    }

    /// The machine the chooser is for.
    #[must_use]
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// The machine's label (its target when it has none).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The check finished: show its screens.
    pub fn set_outputs(&mut self, outputs: Vec<String>) {
        self.outputs = Some(outputs);
        self.selected = 0;
    }

    /// The choices: the focused screen, one per known output, a private screen. Empty while
    /// loading.
    #[must_use]
    pub fn options(&self) -> Vec<(String, ScreenChoice)> {
        let Some(outputs) = &self.outputs else {
            return Vec::new();
        };
        let mut v = vec![(
            "Mirror the focused screen".to_string(),
            ScreenChoice::Mirror,
        )];
        v.extend(
            outputs
                .iter()
                .filter(|o| valid_output_name(o))
                .map(|o| (format!("Mirror {o}"), ScreenChoice::Output(o.clone()))),
        );
        v.push((
            "Private screen sized to the window".to_string(),
            ScreenChoice::Headless,
        ));
        v
    }

    /// Move the highlight, wrapping at the ends.
    pub fn move_by(&mut self, delta: isize) {
        let n = isize::try_from(self.options().len()).unwrap_or(0);
        if n == 0 {
            return;
        }
        let cur = isize::try_from(self.selected).unwrap_or(0);
        self.selected = usize::try_from((cur + delta).rem_euclid(n)).unwrap_or(0);
    }

    /// Highlight the first choice.
    pub fn first(&mut self) {
        self.selected = 0;
    }

    /// Highlight the last choice.
    pub fn last(&mut self) {
        self.selected = self.options().len().saturating_sub(1);
    }

    /// The highlighted choice, `None` while loading.
    #[must_use]
    pub fn pick(&self) -> Option<ScreenChoice> {
        self.options().into_iter().nth(self.selected).map(|o| o.1)
    }

    /// The index of the highlighted choice.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Draw the box centred in `area`; `frame` is the spinner frame.
    pub fn draw(&self, buf: &mut Buffer, area: Rect, frame: usize) {
        let area = area.intersection(buf.area);
        let options = self.options();
        let lines = options.len().max(1);
        let w = BOX_WIDTH.min(area.width);
        let want_h = u16::try_from(lines + 2).unwrap_or(u16::MAX);
        let h = want_h.min(area.height);
        if w < 8 || h < 3 {
            return;
        }
        let rect = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + (area.height - h) / 2,
            w,
            h,
        );
        Clear.render(rect, buf);
        let title = format!(" {} › screen ", self.label);
        Block::bordered()
            .title(ratatui::text::Line::styled(
                crate::text::truncate(&title, usize::from(w.saturating_sub(4))),
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .render(rect, buf);
        let inner = Rect::new(rect.x + 1, rect.y + 1, rect.width - 2, rect.height - 2);
        if self.is_loading() {
            let g = glyph(&RowState::Checking, frame);
            let x = put(buf, inner, inner.x + 1, inner.y, g, tone_style(Tone::Busy));
            put(
                buf,
                inner,
                x + 1,
                inner.y,
                &format!("checking {}…", self.label),
                DIM,
            );
            return;
        }
        let visible = usize::from(inner.height);
        let first = (self.selected + 1).saturating_sub(visible);
        for (row, (text, _)) in options.iter().enumerate().skip(first).take(visible) {
            let y = inner.y + u16::try_from(row - first).unwrap_or(0);
            let cell = crate::text::pad(text, usize::from(inner.width.saturating_sub(2)));
            put(buf, inner, inner.x + 1, y, &cell, Style::new());
            if row == self.selected {
                buf.set_style(
                    Rect::new(inner.x, y, inner.width, 1),
                    Style::new().add_modifier(Modifier::REVERSED),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chooser(outputs: &[&str]) -> Chooser {
        Chooser::ready(
            "j".into(),
            Target::parse("j@h").unwrap(),
            outputs.iter().map(|s| (*s).to_string()).collect(),
        )
    }

    #[test]
    fn options_are_mirror_each_output_private() {
        let c = chooser(&["DP-1", "DP-2"]);
        let o = c.options();
        assert_eq!(
            o.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            [
                "Mirror the focused screen",
                "Mirror DP-1",
                "Mirror DP-2",
                "Private screen sized to the window"
            ]
        );
        assert_eq!(o[2].1, ScreenChoice::Output("DP-2".into()));
        assert_eq!(o[3].1, ScreenChoice::Headless);
    }

    #[test]
    fn output_names_that_gliff_would_refuse_are_not_offered() {
        let c = chooser(&["DP-1", "bad name", "--evil"]);
        let names: Vec<String> = c.options().into_iter().map(|o| o.0).collect();
        assert!(names.contains(&"Mirror DP-1".to_string()));
        assert!(!names.iter().any(|n| n.contains("bad name")), "{names:?}");
    }

    #[test]
    fn moving_wraps_and_first_last_do_not() {
        let mut c = chooser(&["DP-1"]);
        assert_eq!(c.pick(), Some(ScreenChoice::Mirror));
        c.move_by(-1);
        assert_eq!(c.pick(), Some(ScreenChoice::Headless));
        c.move_by(1);
        assert_eq!(c.selected(), 0);
        c.last();
        assert_eq!(c.selected(), 2);
        c.first();
        assert_eq!(c.selected(), 0);
    }

    #[test]
    fn loading_has_no_choices_until_the_check_answers() {
        let mut c = Chooser::loading("j".into(), Target::parse("j@h").unwrap());
        assert!(c.is_loading() && c.pick().is_none() && c.options().is_empty());
        c.move_by(1);
        c.set_outputs(vec!["DP-1".into()]);
        assert!(!c.is_loading());
        assert_eq!(c.options().len(), 3);
    }

    #[test]
    fn drawing_never_panics_and_shows_the_title_and_options() {
        for (w, h) in [(0, 0), (1, 1), (5, 3), (20, 4), (44, 6), (100, 30)] {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            chooser(&["DP-1", "DP-2", "DP-3", "DP-4"]).draw(&mut buf, area, 0);
            Chooser::loading("j".into(), Target::parse("j@h").unwrap()).draw(&mut buf, area, 3);
        }
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        chooser(&["DP-1"]).draw(&mut buf, area, 0);
        let text: String = (0..10)
            .map(|y| {
                (0..60)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("j › screen"), "{text}");
        assert!(text.contains("Mirror DP-1"));
        assert!(text.contains("Private screen sized to the window"));
    }
}
