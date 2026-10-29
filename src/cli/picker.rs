use std::io::{self, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};

pub(super) const LIGHT_GREEN: &str = "\x1b[38;2;191;217;155m";
pub(super) const OLIVE: &str = "\x1b[38;2;152;159;95m";
pub(super) const AMBER: &str = "\x1b[38;5;179m";
const RESET: &str = "\x1b[0m";
const NOT_DETECTED: &str = " (not detected)";
const MIN_LABEL_WIDTH: usize = 12;
const DEFAULT_COLUMNS: u16 = 80;
const FOOTER: &str = "↑↓ move · space toggle · a all · enter confirm · esc cancel";

/// One choice on screen: a name, whether it was found here, and what picking it writes.
/// Every pick writes outside the repo, so the summary is always drawn as a warning.
pub struct Row {
    pub label: String,
    pub detected: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Toggle,
    All,
    Confirm,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Picking,
    Confirmed,
    Cancelled,
}

/// Plain for tests and dumb terminals; ANSI colours on a real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Plain,
    Ansi,
}

/// A checklist the person moves through with the arrow keys. Rows found on this
/// machine start checked, so enter alone wires what is installed.
pub struct Picker {
    title: String,
    rows: Vec<Row>,
    checked: Vec<bool>,
    cursor: usize,
}

fn paint(text: &str, colour: &str, style: Style) -> String {
    match style {
        Style::Plain => text.to_string(),
        Style::Ansi => format!("{colour}{text}{RESET}"),
    }
}

impl Picker {
    pub fn new(title: impl Into<String>, rows: Vec<Row>) -> Self {
        let checked = rows.iter().map(|r| r.detected).collect();
        Self { title: title.into(), rows, checked, cursor: 0 }
    }

    pub fn apply(&mut self, key: Key) -> Outcome {
        let n = self.rows.len();
        match key {
            Key::Up if n > 0 => self.cursor = (self.cursor + n - 1) % n,
            Key::Down if n > 0 => self.cursor = (self.cursor + 1) % n,
            Key::Toggle if n > 0 => self.checked[self.cursor] = !self.checked[self.cursor],
            Key::All => {
                let all_on = self.checked.iter().all(|c| *c);
                self.checked.iter_mut().for_each(|c| *c = !all_on);
            }
            Key::Confirm => return Outcome::Confirmed,
            Key::Cancel => return Outcome::Cancelled,
            _ => {}
        }
        Outcome::Picking
    }

    /// Indexes of the checked rows, in row order.
    pub fn picked(&self) -> Vec<usize> {
        self.checked.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect()
    }

    pub fn render(&self, style: Style) -> Vec<String> {
        let label = |r: &Row| format!("{}{}", r.label, if r.detected { "" } else { NOT_DETECTED });
        let width = self.rows.iter().map(|r| label(r).chars().count()).max().unwrap_or(0).max(MIN_LABEL_WIDTH);

        let mut lines = vec![self.title.clone(), String::new()];
        for (i, row) in self.rows.iter().enumerate() {
            let here = i == self.cursor;
            let pointer = if here { "›" } else { " " };
            let mark = if self.checked[i] { "[✓]" } else { "[ ]" };
            let name = if here { paint(&row.label, LIGHT_GREEN, style) } else { row.label.clone() };
            let tag = if row.detected { String::new() } else { paint(NOT_DETECTED, OLIVE, style) };
            let gap = " ".repeat(width - label(row).chars().count());
            let summary = paint(&row.summary, AMBER, style);
            lines.push(format!("{pointer} {mark} {name}{tag}{gap}   {summary}"));
        }
        lines.push(String::new());
        lines.push(paint(FOOTER, OLIVE, style));
        lines
    }
}

fn key_of(event: KeyEvent) -> Option<Key> {
    if event.kind != KeyEventKind::Press {
        return None;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('c') {
        return Some(Key::Cancel);
    }
    match event.code {
        KeyCode::Up | KeyCode::Char('k') => Some(Key::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(Key::Down),
        KeyCode::Char(' ') => Some(Key::Toggle),
        KeyCode::Char('a') => Some(Key::All),
        KeyCode::Enter => Some(Key::Confirm),
        KeyCode::Esc | KeyCode::Char('q') => Some(Key::Cancel),
        _ => None,
    }
}

/// Terminal rows `lines` take at `columns` wide: a line wider than the terminal wraps,
/// and the redraw has to climb over every row or the title is left behind. A pty
/// with no size set reports 0 columns; read as one column, the redraw would climb
/// hundreds of rows and erase the scrollback above the picker.
fn rows_on_screen(lines: &[String], columns: u16) -> u16 {
    let columns = usize::from(if columns == 0 { DEFAULT_COLUMNS } else { columns });
    let rows: usize = lines.iter().map(|l| l.chars().count().div_ceil(columns).max(1)).sum();
    u16::try_from(rows).unwrap_or(u16::MAX)
}

/// Raw mode on for as long as it lives, so an error or panic still gives the shell back.
struct RawMode;

impl RawMode {
    fn enter(out: &mut impl Write) -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(out, cursor::Hide)?;
        Ok(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(io::stderr(), cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

fn draw(out: &mut impl Write, picker: &Picker, drawn: u16) -> io::Result<u16> {
    if drawn > 0 {
        execute!(out, cursor::MoveToPreviousLine(drawn), terminal::Clear(terminal::ClearType::FromCursorDown))?;
    }
    // Raw mode turns off the newline's carriage return, so each line ends in \r\n
    for line in picker.render(Style::Ansi) {
        write!(out, "{line}\r\n")?;
    }
    out.flush()?;
    let columns = terminal::size().map(|(c, _)| c).unwrap_or(DEFAULT_COLUMNS);
    Ok(rows_on_screen(&picker.render(Style::Plain), columns))
}

/// Run the picker on stderr until enter or esc: the picked row indexes, or `None` when
/// cancelled. It is erased afterwards; what init prints next reports the choice.
pub fn run(mut picker: Picker) -> io::Result<Option<Vec<usize>>> {
    let mut out = io::stderr();
    let _raw = RawMode::enter(&mut out)?;
    let mut drawn = draw(&mut out, &picker, 0)?;

    let outcome = loop {
        let Event::Key(event) = event::read()? else {
            continue;
        };
        let Some(key) = key_of(event) else {
            continue;
        };
        match picker.apply(key) {
            Outcome::Picking => drawn = draw(&mut out, &picker, drawn)?,
            done => break done,
        }
    };

    execute!(out, cursor::MoveToPreviousLine(drawn), terminal::Clear(terminal::ClearType::FromCursorDown))?;
    Ok((outcome == Outcome::Confirmed).then(|| picker.picked()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &str, detected: bool) -> Row {
        Row { label: label.into(), detected, summary: format!("{label}.md") }
    }

    fn picker() -> Picker {
        Picker::new("knot init · pick the agents you use:", vec![row("claude", true), row("codex", false), row("cursor", true)])
    }

    #[test]
    fn detected_rows_start_checked() {
        assert_eq!(picker().picked(), vec![0, 2]);
    }

    #[test]
    fn keys_move_wrap_and_toggle() {
        let mut p = picker();
        assert_eq!(p.apply(Key::Up), Outcome::Picking);
        p.apply(Key::Toggle);
        assert_eq!(p.picked(), vec![0], "up from the top wraps to the last row");
        p.apply(Key::Down);
        p.apply(Key::Down);
        p.apply(Key::Toggle);
        assert_eq!(p.picked(), vec![0, 1]);
    }

    #[test]
    fn all_checks_everything_then_clears_it() {
        let mut p = picker();
        p.apply(Key::All);
        assert_eq!(p.picked(), vec![0, 1, 2]);
        p.apply(Key::All);
        assert!(p.picked().is_empty());
    }

    #[test]
    fn enter_confirms_and_esc_cancels() {
        assert_eq!(picker().apply(Key::Confirm), Outcome::Confirmed);
        assert_eq!(picker().apply(Key::Cancel), Outcome::Cancelled);
    }

    #[test]
    fn render_aligns_summaries_and_tags_missing_agents() {
        let lines = picker().render(Style::Plain);
        assert_eq!(lines[0], "knot init · pick the agents you use:");
        assert_eq!(lines[2], format!("› [✓] claude{}claude.md", " ".repeat(17)));
        assert_eq!(lines[3], "  [ ] codex (not detected)   codex.md");
        assert_eq!(lines[4], format!("  [✓] cursor{}cursor.md", " ".repeat(17)));
        assert_eq!(lines.last().unwrap(), FOOTER);
    }

    #[test]
    fn wrapped_lines_count_every_row() {
        let lines = vec!["x".repeat(100), String::new()];
        assert_eq!(rows_on_screen(&lines, 80), 3);
    }

    #[test]
    fn unsized_terminal_counts_as_eighty_columns() {
        let lines = picker().render(Style::Plain);
        assert_eq!(rows_on_screen(&lines, 0), rows_on_screen(&lines, 80));
        assert_eq!(rows_on_screen(&lines, 0), u16::try_from(lines.len()).unwrap());
    }

    #[test]
    fn ctrl_c_and_release_events() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_of(ctrl_c), Some(Key::Cancel));
        let mut release = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(key_of(release), None);
    }
}
