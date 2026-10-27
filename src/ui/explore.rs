use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Terminal;
use std::path::Path;
use std::time::Duration;

use crate::ast::resolve::CodeGraph;
use crate::ast::types::SymbolNode;

const TICK_RATE_MS: u64 = 250;
const LEFT_PANEL_PERCENT: u16 = 38;
const RIGHT_PANEL_PERCENT: u16 = 62;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Callers,
    Callees,
    BlastRadius,
    Crux,
}

impl DetailTab {
    pub fn index(&self) -> usize {
        match self {
            DetailTab::Callers => 0,
            DetailTab::Callees => 1,
            DetailTab::BlastRadius => 2,
            DetailTab::Crux => 3,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            DetailTab::Callers => DetailTab::Callees,
            DetailTab::Callees => DetailTab::BlastRadius,
            DetailTab::BlastRadius => DetailTab::Crux,
            DetailTab::Crux => DetailTab::Callers,
        }
    }

    pub fn titles() -> Vec<&'static str> {
        vec!["1: Callers", "2: Callees", "3: Blast Radius", "4: Crux Code"]
    }
}

pub struct ExplorerState {
    pub graph: CodeGraph,
    pub all_symbols: Vec<SymbolNode>,
    pub filtered_indices: Vec<usize>,
    pub selected_index: usize,
    pub active_tab: DetailTab,
    pub search_query: String,
    pub is_searching: bool,
    pub detail_list_selected: usize,
}

impl ExplorerState {
    pub fn new(graph: CodeGraph, initial_query: Option<&str>) -> Self {
        let mut all_symbols: Vec<SymbolNode> = graph.symbols.values().cloned().collect();
        all_symbols.sort_by(|a, b| a.id.cmp(&b.id));

        let mut state = Self {
            graph,
            all_symbols,
            filtered_indices: Vec::new(),
            selected_index: 0,
            active_tab: DetailTab::Callers,
            search_query: initial_query.unwrap_or("").to_string(),
            is_searching: false,
            detail_list_selected: 0,
        };
        state.refilter();
        state
    }

    pub fn refilter(&mut self) {
        let q = self.search_query.to_lowercase();
        self.filtered_indices = self
            .all_symbols
            .iter()
            .enumerate()
            .filter(|(_, sym)| {
                if q.is_empty() {
                    return true;
                }
                sym.name.to_lowercase().contains(&q)
                    || sym.file.to_lowercase().contains(&q)
                    || sym.id.to_lowercase().contains(&q)
            })
            .map(|(idx, _)| idx)
            .collect();

        if self.selected_index >= self.filtered_indices.len() {
            self.selected_index = self.filtered_indices.len().saturating_sub(1);
        }
        self.detail_list_selected = 0;
    }

    pub fn selected_symbol(&self) -> Option<&SymbolNode> {
        let global_idx = *self.filtered_indices.get(self.selected_index)?;
        self.all_symbols.get(global_idx)
    }

    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.detail_list_selected = 0;
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered_indices.is_empty() && self.selected_index + 1 < self.filtered_indices.len() {
            self.selected_index += 1;
            self.detail_list_selected = 0;
        }
    }

    pub fn cycle_tab(&mut self) {
        self.active_tab = self.active_tab.next();
        self.detail_list_selected = 0;
    }

    pub fn select_tab(&mut self, tab: DetailTab) {
        self.active_tab = tab;
        self.detail_list_selected = 0;
    }

    pub fn jump_to_symbol(&mut self, symbol_id: &str) -> bool {
        if let Some(pos) = self.filtered_indices.iter().position(|&idx| {
            self.all_symbols.get(idx).map(|s| s.id == symbol_id).unwrap_or(false)
        }) {
            self.selected_index = pos;
            self.detail_list_selected = 0;
            return true;
        }

        self.search_query.clear();
        self.refilter();
        if let Some(pos) = self.filtered_indices.iter().position(|&idx| {
            self.all_symbols.get(idx).map(|s| s.id == symbol_id).unwrap_or(false)
        }) {
            self.selected_index = pos;
            self.detail_list_selected = 0;
            return true;
        }

        false
    }
}

fn symbol_kind_color(kind: &crate::ast::types::SymbolKind) -> Color {
    match kind {
        crate::ast::types::SymbolKind::Function => Color::Cyan,
        crate::ast::types::SymbolKind::Method => Color::LightBlue,
        crate::ast::types::SymbolKind::Struct => Color::Yellow,
        crate::ast::types::SymbolKind::Class => Color::Green,
        crate::ast::types::SymbolKind::Trait => Color::Magenta,
        crate::ast::types::SymbolKind::Enum => Color::LightYellow,
        crate::ast::types::SymbolKind::Module => Color::Blue,
        crate::ast::types::SymbolKind::Interface => Color::LightCyan,
        _ => Color::White,
    }
}

fn draw_symbols_list(f: &mut ratatui::Frame, area: Rect, state: &ExplorerState) {
    let items: Vec<ListItem> = state
        .filtered_indices
        .iter()
        .enumerate()
        .map(|(local_idx, &global_idx)| {
            let sym = &state.all_symbols[global_idx];
            let is_selected = local_idx == state.selected_index;

            let tag_color = symbol_kind_color(&sym.kind);
            let tag = format!("{:<6}", sym.kind.as_str());

            let mut spans = vec![
                Span::styled(format!("[{tag}] "), Style::default().fg(tag_color)),
                Span::styled(&sym.name, Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("  ({})", sym.file), Style::default().fg(Color::DarkGray)),
            ];

            if is_selected {
                spans.insert(0, Span::styled("▶ ", Style::default().fg(Color::Yellow)));
            } else {
                spans.insert(0, Span::raw("  "));
            }

            let style = if is_selected {
                Style::default().bg(Color::Rgb(30, 40, 60))
            } else {
                Style::default()
            };

            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();

    let title = format!(
        " Symbols ({} / {}) ",
        state.filtered_indices.len(),
        state.all_symbols.len()
    );

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

fn draw_detail_panel(f: &mut ratatui::Frame, area: Rect, state: &ExplorerState) {
    let Some(sym) = state.selected_symbol() else {
        let p = Paragraph::new("No symbol selected")
            .block(Block::default().borders(Borders::ALL).title(" Details "));
        f.render_widget(p, area);
        return;
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(5),
        ])
        .split(area);

    let meta_lines = vec![
        Line::from(vec![
            Span::styled("Symbol: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&sym.id, Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Location: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}:L{}-L{}", sym.file, sym.span.0, sym.span.1)),
            Span::styled("   Kind: ", Style::default().fg(Color::DarkGray)),
            Span::styled(sym.kind.as_str(), Style::default().fg(symbol_kind_color(&sym.kind))),
        ]),
        Line::from(vec![
            Span::styled("Signature: ", Style::default().fg(Color::DarkGray)),
            Span::raw(&sym.signature),
        ]),
    ];

    let meta_widget = Paragraph::new(meta_lines)
        .block(Block::default().borders(Borders::ALL).title(" Definition "));
    f.render_widget(meta_widget, chunks[0]);

    let tabs = Tabs::new(DetailTab::titles())
        .block(Block::default().borders(Borders::ALL).title(" Navigation "))
        .select(state.active_tab.index())
        .style(Style::default().fg(Color::White))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
    f.render_widget(tabs, chunks[1]);

    match state.active_tab {
        DetailTab::Callers => {
            let callers = state.graph.get_callers(&sym.name);
            let items: Vec<ListItem> = if callers.is_empty() {
                vec![ListItem::new(Line::from(Span::styled(
                    "No callers found in indexed graph",
                    Style::default().fg(Color::DarkGray),
                )))]
            } else {
                callers
                    .iter()
                    .map(|c| {
                        ListItem::new(Line::from(vec![
                            Span::styled("← ", Style::default().fg(Color::Green)),
                            Span::styled(&c.name, Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(format!("  ({}:L{})", c.file, c.span.0), Style::default().fg(Color::DarkGray)),
                        ]))
                    })
                    .collect()
            };
            let title = format!(" Callers ({}) ", callers.len());
            let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
            f.render_widget(list, chunks[2]);
        }
        DetailTab::Callees => {
            let callees = state.graph.get_callees(&sym.name);
            let items: Vec<ListItem> = if callees.is_empty() {
                vec![ListItem::new(Line::from(Span::styled(
                    "No callees found in indexed graph",
                    Style::default().fg(Color::DarkGray),
                )))]
            } else {
                callees
                    .iter()
                    .map(|c| {
                        ListItem::new(Line::from(vec![
                            Span::styled("→ ", Style::default().fg(Color::Blue)),
                            Span::styled(&c.name, Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(format!("  ({}:L{})", c.file, c.span.0), Style::default().fg(Color::DarkGray)),
                        ]))
                    })
                    .collect()
            };
            let title = format!(" Callees ({}) ", callees.len());
            let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
            f.render_widget(list, chunks[2]);
        }
        DetailTab::BlastRadius => {
            let blast = state.graph.blast_radius(&sym.name, Some(2));
            let items: Vec<ListItem> = if blast.impacted_symbols.is_empty() {
                vec![ListItem::new(Line::from(Span::styled(
                    "No transitive blast radius dependents found",
                    Style::default().fg(Color::DarkGray),
                )))]
            } else {
                blast
                    .impacted_symbols
                    .iter()
                    .map(|b| {
                        ListItem::new(Line::from(vec![
                            Span::styled("✦ ", Style::default().fg(Color::Magenta)),
                            Span::styled(&b.name, Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(format!("  ({}:L{})", b.file, b.span.0), Style::default().fg(Color::DarkGray)),
                        ]))
                    })
                    .collect()
            };
            let title = format!(
                " Transitive Blast Radius (depth 2: {} symbols) ",
                blast.impacted_symbols.len()
            );
            let list = List::new(items).block(Block::default().borders(Borders::ALL).title(title));
            f.render_widget(list, chunks[2]);
        }
        DetailTab::Crux => {
            let text = if sym.crux.is_empty() {
                sym.signature.clone()
            } else {
                sym.crux.clone()
            };
            let p = Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(" Crux Definition "));
            f.render_widget(p, chunks[2]);
        }
    }
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, state: &ExplorerState) {
    let content = if state.is_searching {
        Line::from(vec![
            Span::styled("Search: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(&state.search_query),
            Span::styled("█", Style::default().fg(Color::White)),
            Span::styled("  (Enter: apply, Esc: cancel)", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled("q", Style::default().fg(Color::Yellow)),
            Span::raw(": Quit  "),
            Span::styled("↑/↓", Style::default().fg(Color::Yellow)),
            Span::raw(": Move  "),
            Span::styled("Tab/1-4", Style::default().fg(Color::Yellow)),
            Span::raw(": View  "),
            Span::styled("/", Style::default().fg(Color::Yellow)),
            Span::raw(": Filter  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(": Follow reference"),
        ])
    };

    let p = Paragraph::new(content).block(Block::default().borders(Borders::ALL));
    f.render_widget(p, area);
}

fn render_tui(f: &mut ratatui::Frame, state: &ExplorerState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(f.area());

    let header_text = format!(
        " Knot Codebase Explorer | {} indexed files, {} total symbols",
        state.graph.file_to_ids.len(),
        state.all_symbols.len()
    );
    let header = Paragraph::new(Line::from(vec![
        Span::styled("KNOT ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(header_text),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(LEFT_PANEL_PERCENT),
            Constraint::Percentage(RIGHT_PANEL_PERCENT),
        ])
        .split(chunks[1]);

    draw_symbols_list(f, body_chunks[0], state);
    draw_detail_panel(f, body_chunks[1], state);
    draw_footer(f, chunks[2], state);
}

fn handle_input(state: &mut ExplorerState, key: KeyCode) -> bool {
    if state.is_searching {
        match key {
            KeyCode::Esc => {
                state.is_searching = false;
            }
            KeyCode::Enter => {
                state.is_searching = false;
            }
            KeyCode::Backspace => {
                state.search_query.pop();
                state.refilter();
            }
            KeyCode::Char(c) => {
                state.search_query.push(c);
                state.refilter();
            }
            _ => {}
        }
        return false;
    }

    match key {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_up();
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_down();
            false
        }
        KeyCode::Tab => {
            state.cycle_tab();
            false
        }
        KeyCode::Char('1') => {
            state.select_tab(DetailTab::Callers);
            false
        }
        KeyCode::Char('2') => {
            state.select_tab(DetailTab::Callees);
            false
        }
        KeyCode::Char('3') => {
            state.select_tab(DetailTab::BlastRadius);
            false
        }
        KeyCode::Char('4') => {
            state.select_tab(DetailTab::Crux);
            false
        }
        KeyCode::Char('/') => {
            state.is_searching = true;
            false
        }
        KeyCode::Enter => {
            if let Some(sym) = state.selected_symbol() {
                let sym_name = sym.name.clone();
                match state.active_tab {
                    DetailTab::Callers => {
                        let callers = state.graph.get_callers(&sym_name);
                        if let Some(first) = callers.first() {
                            state.jump_to_symbol(&first.id);
                        }
                    }
                    DetailTab::Callees => {
                        let callees = state.graph.get_callees(&sym_name);
                        if let Some(first) = callees.first() {
                            state.jump_to_symbol(&first.id);
                        }
                    }
                    DetailTab::BlastRadius => {
                        let blast = state.graph.blast_radius(&sym_name, Some(2));
                        if let Some(first) = blast.impacted_symbols.first() {
                            state.jump_to_symbol(&first.id);
                        }
                    }
                    DetailTab::Crux => {}
                }
            }
            false
        }
        _ => false,
    }
}

/// Run the interactive terminal code explorer.
pub fn run_explorer(repo_root: &Path, initial_query: Option<&str>) -> Result<()> {
    let graph = crate::ask::load_graph(repo_root)?;
    if graph.symbols.is_empty() {
        println!("[KNOT] No symbols found in repository.");
        return Ok(());
    }

    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = ExplorerState::new(graph, initial_query);
    let tick_rate = Duration::from_millis(TICK_RATE_MS);

    loop {
        terminal.draw(|f| render_tui(f, &state))?;

        if event::poll(tick_rate)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && handle_input(&mut state, key.code) {
                    break;
                }
            }
        }
    }

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::types::SymbolKind;

    fn make_test_graph() -> CodeGraph {
        let mut graph = CodeGraph::default();
        let sym_a = SymbolNode {
            id: "src/a.rs#alpha".into(),
            name: "alpha".into(),
            kind: SymbolKind::Function,
            file: "src/a.rs".into(),
            span: (1, 10),
            signature: "pub fn alpha()".into(),
            outgoing_calls: vec!["beta".into()],
            crux: "pub fn alpha() {\n    beta();\n}".into(),
            hash: "hash_a".into(),
        };
        let sym_b = SymbolNode {
            id: "src/b.rs#beta".into(),
            name: "beta".into(),
            kind: SymbolKind::Function,
            file: "src/b.rs".into(),
            span: (1, 5),
            signature: "pub fn beta()".into(),
            outgoing_calls: vec![],
            crux: "pub fn beta() {}".into(),
            hash: "hash_b".into(),
        };

        graph.symbols.insert(sym_a.id.clone(), sym_a);
        graph.symbols.insert(sym_b.id.clone(), sym_b);
        graph.name_to_ids.insert("alpha".into(), vec!["src/a.rs#alpha".into()]);
        graph.name_to_ids.insert("beta".into(), vec!["src/b.rs#beta".into()]);
        graph.callers.insert("src/b.rs#beta".into(), vec!["src/a.rs#alpha".into()]);
        graph.callees.insert("src/a.rs#alpha".into(), vec!["src/b.rs#beta".into()]);
        graph
    }

    #[test]
    fn test_explorer_state_filtering() {
        let graph = make_test_graph();
        let mut state = ExplorerState::new(graph, None);

        assert_eq!(state.filtered_indices.len(), 2);

        state.search_query = "alpha".into();
        state.refilter();
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.selected_symbol().unwrap().name, "alpha");

        state.search_query = "nonexistent".into();
        state.refilter();
        assert_eq!(state.filtered_indices.len(), 0);
        assert!(state.selected_symbol().is_none());
    }

    #[test]
    fn test_explorer_state_tab_cycling() {
        let graph = make_test_graph();
        let mut state = ExplorerState::new(graph, None);

        assert_eq!(state.active_tab, DetailTab::Callers);
        state.cycle_tab();
        assert_eq!(state.active_tab, DetailTab::Callees);
        state.cycle_tab();
        assert_eq!(state.active_tab, DetailTab::BlastRadius);
        state.cycle_tab();
        assert_eq!(state.active_tab, DetailTab::Crux);
        state.cycle_tab();
        assert_eq!(state.active_tab, DetailTab::Callers);
    }

    #[test]
    fn test_explorer_state_navigation_and_jump() {
        let graph = make_test_graph();
        let mut state = ExplorerState::new(graph, None);

        assert_eq!(state.selected_index, 0);
        state.move_down();
        assert_eq!(state.selected_index, 1);
        state.move_down();
        assert_eq!(state.selected_index, 1);
        state.move_up();
        assert_eq!(state.selected_index, 0);

        let jumped = state.jump_to_symbol("src/b.rs#beta");
        assert!(jumped);
        assert_eq!(state.selected_symbol().unwrap().id, "src/b.rs#beta");
    }
}
