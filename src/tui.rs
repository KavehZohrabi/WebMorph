use std::{
    io::{self, Stdout},
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols::border::{self, Set as BorderSet},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    dom::Document,
    error::{Result, WebMorphError},
    html,
    network::HttpClient,
    plugins::PluginRegistry,
    transform::{ComponentKind, TuiPage, layout_components, transform},
};

const TICK: Duration = Duration::from_millis(35);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Auto,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Address,
    Tabs,
    Content,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Dialog {
    Help,
}

#[derive(Debug, Clone)]
struct BrowserTab {
    id: u64,
    title: String,
    url: String,
    page: TuiPage,
    error: Option<String>,
    loading: bool,
    scroll: usize,
    history: Vec<String>,
    history_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InteractiveTarget {
    id: String,
    label: String,
    target: Option<String>,
    is_button: bool,
    disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiAction {
    None,
    Quit,
    Navigate(String),
    Back,
    Forward,
    Reload,
    NewTab,
    CloseTab,
}

#[derive(Debug, Clone)]
struct UiState {
    tabs: Vec<BrowserTab>,
    next_tab_id: u64,
    active_tab: usize,
    address: String,
    address_cursor: usize,
    address_replacing: bool,
    focus: Focus,
    selected_interactive: usize,
    dialog: Option<Dialog>,
    theme: Theme,
    status: String,
    loading_frame: usize,
    dirty: bool,
}

impl UiState {
    fn new(page: TuiPage) -> Self {
        let address = match &page.root.kind {
            ComponentKind::Page { url, .. } => url.clone(),
            _ => String::new(),
        };
        let tab = BrowserTab {
            id: 0,
            title: page.title.clone(),
            url: address.clone(),
            page,
            error: None,
            loading: false,
            scroll: 0,
            history: if address.is_empty() {
                vec![]
            } else {
                vec![address.clone()]
            },
            history_index: 0,
        };
        Self {
            tabs: vec![tab],
            next_tab_id: 1,
            active_tab: 0,
            address_cursor: address.chars().count(),
            address,
            address_replacing: false,
            focus: Focus::Content,
            selected_interactive: 0,
            dialog: None,
            theme: Theme::Auto,
            status: "Ready".into(),
            loading_frame: 0,
            dirty: true,
        }
    }

    fn tab(&self) -> &BrowserTab {
        &self.tabs[self.active_tab]
    }
    fn tab_mut(&mut self) -> &mut BrowserTab {
        &mut self.tabs[self.active_tab]
    }

    fn handle_key(&mut self, key: KeyEvent) -> UiAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && matches!(key.code, KeyCode::Char('q' | 'Q')) {
            return UiAction::Quit;
        }
        if ctrl && matches!(key.code, KeyCode::Char('l' | 'L')) {
            self.focus = Focus::Address;
            self.address_cursor = self.address.chars().count();
            self.address_replacing = true;
            self.dirty = true;
            return UiAction::None;
        }
        if ctrl && matches!(key.code, KeyCode::Char('t' | 'T')) {
            return UiAction::NewTab;
        }
        if ctrl && matches!(key.code, KeyCode::Char('w' | 'W')) {
            return UiAction::CloseTab;
        }
        if self.dialog.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?')) {
                self.dialog = None;
                self.dirty = true;
            }
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::F(1) | KeyCode::Char('?')) {
            self.dialog = Some(Dialog::Help);
            self.dirty = true;
            return UiAction::None;
        }
        if key.code == KeyCode::Esc {
            if self.tab().loading {
                self.status = "Cancelling request…".into();
                return UiAction::Navigate(String::new());
            }
            self.focus = Focus::Content;
            self.dirty = true;
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.move_focus(key.code != KeyCode::BackTab && !shift);
            self.dirty = true;
            return UiAction::None;
        }
        if self.focus == Focus::Address {
            return self.address_key(key);
        }
        match key.code {
            KeyCode::Enter => self.activate(),
            KeyCode::Up | KeyCode::Char('k') => {
                if self.focus == Focus::Tabs {
                    self.select_tab(-1);
                } else {
                    self.tab_mut().scroll = self.tab().scroll.saturating_sub(1);
                }
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.focus == Focus::Tabs {
                    self.select_tab(1);
                } else {
                    self.tab_mut().scroll = self.tab().scroll.saturating_add(1);
                }
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Left if self.focus == Focus::Tabs => {
                self.select_tab(-1);
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Right if self.focus == Focus::Tabs => {
                self.select_tab(1);
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.tab_mut().scroll = 0;
                self.dirty = true;
                UiAction::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.tab_mut().scroll = self.tab().page.lines.len();
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Char('t') => {
                self.theme = match self.theme {
                    Theme::Auto => Theme::Dark,
                    Theme::Dark => Theme::Light,
                    Theme::Light => Theme::Auto,
                };
                self.status = format!("Theme: {:?}", self.theme);
                self.dirty = true;
                UiAction::None
            }
            KeyCode::Char('q' | 'Q') => UiAction::Quit,
            KeyCode::Char('r' | 'R') => UiAction::Reload,
            KeyCode::Char('[') => UiAction::Back,
            KeyCode::Char(']') => UiAction::Forward,
            _ => UiAction::None,
        }
    }

    fn address_key(&mut self, key: KeyEvent) -> UiAction {
        let mut chars: Vec<char> = self.address.chars().collect();
        match key.code {
            KeyCode::Enter => return UiAction::Navigate(self.address.trim().to_owned()),
            KeyCode::Esc => {
                self.focus = Focus::Content;
                self.dirty = true;
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if self.address_replacing {
                    chars.clear();
                    self.address_cursor = 0;
                    self.address_replacing = false;
                }
                chars.insert(self.address_cursor.min(chars.len()), ch);
                self.address_cursor += 1;
            }
            KeyCode::Backspace if self.address_cursor > 0 => {
                if self.address_replacing {
                    chars.clear();
                    self.address_cursor = 0;
                    self.address_replacing = false;
                } else {
                    self.address_cursor -= 1;
                    chars.remove(self.address_cursor);
                }
            }
            KeyCode::Delete if self.address_cursor < chars.len() => {
                chars.remove(self.address_cursor);
            }
            KeyCode::Left => self.address_cursor = self.address_cursor.saturating_sub(1),
            KeyCode::Right => self.address_cursor = (self.address_cursor + 1).min(chars.len()),
            KeyCode::Home => self.address_cursor = 0,
            KeyCode::End => self.address_cursor = chars.len(),
            _ => {}
        }
        self.address = chars.into_iter().collect();
        self.dirty = true;
        UiAction::None
    }

    fn activate(&mut self) -> UiAction {
        if self.focus == Focus::Help {
            self.dialog = Some(Dialog::Help);
            self.dirty = true;
            return UiAction::None;
        }
        if self.focus == Focus::Address {
            return UiAction::Navigate(self.address.trim().to_owned());
        }
        if self.focus == Focus::Tabs {
            return UiAction::None;
        }
        let targets = interactive_targets(&self.tab().page.root);
        let Some(target) = targets.get(self.selected_interactive) else {
            return UiAction::None;
        };
        if target.disabled {
            self.status = format!("{} is disabled", target.label);
            self.dirty = true;
            return UiAction::None;
        }
        if let Some(url) = &target.target {
            self.address = url.clone();
            self.address_cursor = self.address.chars().count();
            UiAction::Navigate(url.clone())
        } else {
            self.status = if target.is_button {
                format!(
                    "Button ‘{}’ selected; page scripts are not run",
                    target.label
                )
            } else {
                format!("Selected {}", target.label)
            };
            self.dirty = true;
            UiAction::None
        }
    }

    fn move_focus(&mut self, forward: bool) {
        if self.focus == Focus::Content {
            let count = interactive_targets(&self.tab().page.root).len();
            if forward && self.selected_interactive + 1 < count {
                self.selected_interactive += 1;
                return;
            }
            if !forward && self.selected_interactive > 0 {
                self.selected_interactive -= 1;
                return;
            }
        }
        let order = [Focus::Address, Focus::Tabs, Focus::Content, Focus::Help];
        let i = order
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(2);
        self.focus = order
            [(i as isize + if forward { 1 } else { -1 }).rem_euclid(order.len() as isize) as usize];
        if self.focus == Focus::Content {
            self.selected_interactive = 0;
            if !forward {
                self.selected_interactive = interactive_targets(&self.tab().page.root)
                    .len()
                    .saturating_sub(1);
            }
        }
        if self.focus == Focus::Address {
            self.address_replacing = true;
        }
    }

    fn select_tab(&mut self, delta: isize) {
        self.active_tab =
            (self.active_tab as isize + delta).rem_euclid(self.tabs.len() as isize) as usize;
        self.address = self.tab().url.clone();
        self.address_cursor = self.address.chars().count();
        self.selected_interactive = 0;
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect) -> UiAction {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.tab_mut().scroll = self.tab().scroll.saturating_add(3);
                self.dirty = true;
            }
            MouseEventKind::ScrollUp => {
                self.tab_mut().scroll = self.tab().scroll.saturating_sub(3);
                self.dirty = true;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let y = mouse.row.saturating_sub(area.y);
                let full = area.height >= 11;
                let compact = area.height >= 7 && !full;
                let address_start = 1;
                let tabs_start = if full { 4 } else { 2 };
                let content_start = if full {
                    7
                } else if compact {
                    4
                } else {
                    3
                };
                if y >= address_start && y < if full { 4 } else { 2 } {
                    self.focus = Focus::Address;
                    self.address_cursor = self.address.chars().count();
                    self.address_replacing = true;
                } else if y == 0 {
                    let x = mouse.column.saturating_sub(area.x);
                    let (help_x, reload_x, forward_x, back_x) = if area.width < 58 {
                        (46, 37, 23, 13)
                    } else {
                        (57, 42, 25, 12)
                    };
                    if x >= help_x {
                        self.dialog = Some(Dialog::Help);
                    } else if x >= reload_x {
                        return UiAction::Reload;
                    } else if x >= forward_x {
                        return UiAction::Forward;
                    } else if x >= back_x {
                        return UiAction::Back;
                    }
                } else if (full || compact) && y >= tabs_start && y < if full { 6 } else { 3 } {
                    let x = mouse.column.saturating_sub(area.x) as usize;
                    if self.tabs.len() > 0 {
                        self.active_tab = (x
                            / (area.width.max(1) as usize / self.tabs.len()).max(1))
                        .min(self.tabs.len() - 1);
                    }
                    self.address = self.tab().url.clone();
                    self.focus = Focus::Tabs;
                } else if area.height >= 7 && y >= area.height.saturating_sub(1) {
                    self.dialog = Some(Dialog::Help);
                } else if y >= content_start
                    && y < if area.height >= 7 {
                        area.height.saturating_sub(2)
                    } else {
                        area.height
                    }
                {
                    self.focus = Focus::Content;
                    let visible_row = (y - content_start) as usize + self.tab().scroll;
                    let lines = layout_components(&self.tab().page.root, area.width as usize);
                    if let Some(line) = lines.get(visible_row) {
                        let targets = interactive_targets(&self.tab().page.root);
                        if let Some((index, target)) = targets
                            .iter()
                            .enumerate()
                            .find(|(_, target)| line.contains(&format!("[{}]", target.id)))
                        {
                            self.selected_interactive = index;
                            if !target.disabled {
                                if let Some(url) = &target.target {
                                    return UiAction::Navigate(url.clone());
                                }
                                if target.is_button {
                                    return self.activate();
                                }
                            }
                        }
                    }
                }
                self.dirty = true;
            }
            _ => {}
        }
        UiAction::None
    }
}

struct NavigationResult {
    request_id: u64,
    tab_id: u64,
    url: String,
    history_target: Option<usize>,
    result: std::result::Result<TuiPage, WebMorphError>,
}

pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    ascii: bool,
    mouse_enabled: bool,
}

impl TerminalSession {
    pub fn new() -> Result<Self> {
        let ascii = ascii_only();
        let mouse_enabled = mouse_supported();
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        let enter_result = if mouse_enabled {
            execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        } else {
            execute!(stdout, EnterAlternateScreen)
        };
        if let Err(error) = enter_result {
            let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self {
                terminal,
                ascii,
                mouse_enabled,
            }),
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen);
                Err(error.into())
            }
        }
    }

    pub async fn run(&mut self, page: &TuiPage) -> Result<()> {
        self.run_state(UiState::new(page.clone()), None).await
    }

    /// Start the terminal first, then fetch the initial address inside the UI so
    /// users see loading and error states instead of a blank terminal.
    pub async fn run_url(&mut self, url: String, client: HttpClient) -> Result<()> {
        let placeholder = transform(&Document::new(url.clone(), "Loading"));
        let state = UiState::new(placeholder);
        self.run_state(state, Some((url, client))).await
    }

    async fn run_state(
        &mut self,
        mut state: UiState,
        initial: Option<(String, HttpClient)>,
    ) -> Result<()> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<NavigationResult>();
        let mut request_id = 0u64;
        let mut cancellation: Option<CancellationToken> = None;
        if let Some((url, client)) = initial {
            begin_navigation(
                &mut state,
                url,
                client,
                &sender,
                &mut request_id,
                &mut cancellation,
            );
        }
        let mut ticker = tokio::time::interval(TICK);
        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        loop {
            if state.dirty {
                self.draw(&state)?;
                state.dirty = false;
            }
            tokio::select! {
                signal = &mut shutdown => { signal?; break; }
                Some(result) = receiver.recv() => {
                    if result.request_id == request_id {
                        if let Some(index) = state.tabs.iter().position(|tab| tab.id == result.tab_id) {
                            let is_active = index == state.active_tab;
                            let tab = &mut state.tabs[index]; tab.loading = false; tab.error = None;
                            let status = match result.result {
                                Ok(page) => {
                                    tab.title = page.title.clone(); tab.url = result.url.clone(); tab.page = page; tab.scroll = 0;
                                    if let Some(index) = result.history_target { tab.history_index = index; }
                                    else { tab.history.truncate(tab.history_index.saturating_add(1)); if tab.history.last() != Some(&result.url) { tab.history.push(result.url.clone()); } tab.history_index = tab.history.len().saturating_sub(1); }
                                    "Page loaded"
                                }
                                Err(WebMorphError::Cancelled) => "Request cancelled",
                                Err(error) => { tab.error = Some(error.to_string()); "Unable to load page" }
                            };
                            if is_active {
                                state.address = result.url;
                                state.status = status.into();
                                state.focus = if status == "Unable to load page" { Focus::Address } else { Focus::Content };
                                state.address_replacing = status == "Unable to load page";
                            }
                            cancellation = None; state.dirty = true;
                        }
                    }
                }
                _ = ticker.tick() => {
                    if state.tab().loading { state.loading_frame = state.loading_frame.wrapping_add(1); state.dirty = true; }
                    if event::poll(Duration::ZERO)? {
                        match event::read()? {
                            Event::Key(key) => {
                                let action = state.handle_key(key);
                                match action {
                                    UiAction::Quit => break,
                                    UiAction::Navigate(url) if url.is_empty() => { if let Some(token) = cancellation.take() { token.cancel(); } state.tab_mut().loading = false; state.status = "Request cancelled".into(); state.dirty = true; }
                                    UiAction::Navigate(url) => begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation),
                                    UiAction::Reload => { let url = state.tab().url.clone(); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation); }
                                    UiAction::Back => { if state.tab().history_index > 0 { let target = state.tab().history_index - 1; let url = state.tab().history[target].clone(); begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation, Some(target)); } }
                                    UiAction::Forward => { if state.tab().history_index + 1 < state.tab().history.len() { let target = state.tab().history_index + 1; let url = state.tab().history[target].clone(); begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation, Some(target)); } }
                                    UiAction::NewTab => { let page = transform(&Document::new("", "New tab")); let id = state.next_tab_id; state.next_tab_id += 1; state.tabs.push(BrowserTab { id, title: "New tab".into(), url: String::new(), page, error: None, loading: false, scroll: 0, history: vec![], history_index: 0 }); state.active_tab = state.tabs.len()-1; state.address.clear(); state.address_cursor = 0; state.address_replacing = false; state.focus = Focus::Address; state.dirty = true; }
                                    UiAction::CloseTab => { if state.tabs.len() > 1 { state.tabs.remove(state.active_tab); state.active_tab = state.active_tab.min(state.tabs.len()-1); state.address = state.tab().url.clone(); state.dirty = true; } else { break; } }
                                    UiAction::None => {}
                                }
                            }
                            Event::Mouse(mouse) => {
                                let size = self.terminal.size()?;
                                let area = Rect::new(0, 0, size.width, size.height);
                                let action = state.handle_mouse(mouse, area);
                                match action {
                                    UiAction::Navigate(url) => begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation),
                                    UiAction::Back => {
                                        if state.tab().history_index > 0 {
                                            let target = state.tab().history_index - 1;
                                            let url = state.tab().history[target].clone();
                                            begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation, Some(target));
                                        }
                                    }
                                    UiAction::Forward => {
                                        if state.tab().history_index + 1 < state.tab().history.len() {
                                            let target = state.tab().history_index + 1;
                                            let url = state.tab().history[target].clone();
                                            begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation, Some(target));
                                        }
                                    }
                                    UiAction::Reload => { let url = state.tab().url.clone(); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellation); }
                                    _ => {}
                                }
                                state.dirty = true;
                            }
                            Event::Resize(_, _) => state.dirty = true,
                            _ => {}
                        }
                    }
                }
            }
        }
        if let Some(token) = cancellation {
            token.cancel();
        }
        Ok(())
    }

    fn draw(&mut self, state: &UiState) -> Result<()> {
        let theme = Palette::new(state.theme);
        self.terminal.draw(|frame| {
            let area = frame.area();
            let full = area.height >= 11;
            let compact = area.height >= 7 && !full;
            let rows = if full {
                vec![Constraint::Length(1), Constraint::Length(3), Constraint::Length(2), Constraint::Min(3), Constraint::Length(1), Constraint::Length(1)]
            } else if compact {
                vec![Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]
            } else {
                vec![Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]
            };
            let regions = Layout::vertical(rows).split(area);
            let (top, address_area, tabs_area, content_area, status_area, help_area) = if full { (regions[0], regions[1], regions[2], regions[3], regions[4], regions[5]) } else if compact { (regions[0], regions[1], regions[2], regions[3], regions[4], regions[5]) } else { (regions[0], regions[1], Rect::default(), regions[2], Rect::default(), Rect::default()) };
            let brand = if self.ascii { "WEBMORPH" } else { "WebMorph" };
            let (back, forward) = if self.ascii { ("<-", "->") } else { ("←", "→") };
            let nav = if area.width < 58 {
                let sep = if self.ascii { " | " } else { "  ·  " };
                Line::from(vec![Span::styled(format!(" {brand} "), theme.brand), Span::raw(sep), Span::styled(format!("Back {back}"), theme.muted), Span::raw(sep), Span::styled(format!("Forward {forward}"), theme.muted), Span::raw(sep), Span::styled("Reload", theme.muted), Span::raw(sep), Span::styled("?", theme.muted)])
            } else {
                Line::from(vec![Span::styled(format!(" {brand} "), theme.brand), Span::raw("  "), Span::styled(format!("[ Back {back} ]"), theme.muted), Span::raw("  "), Span::styled(format!("[ Forward {forward} ]"), theme.muted), Span::raw("  "), Span::styled("[ Reload r ]", theme.muted), Span::raw("  "), Span::styled("[ ? Help ]", theme.muted)])
            };
            frame.render_widget(Paragraph::new(nav).style(theme.base), top);
            let address_style = if state.focus == Focus::Address { theme.focus } else { theme.base };
            let address_display = address_window(&state.address, state.address_cursor, address_area.width.saturating_sub(if full { 7 } else { 5 }) as usize, state.focus == Focus::Address, self.ascii);
            let address_widget = Paragraph::new(Line::from(vec![Span::styled(" URL ", theme.accent), Span::raw(address_display)]));
            if full {
                frame.render_widget(address_widget.block(Block::default().borders(Borders::ALL).border_style(address_style).border_set(border_set(self.ascii))), address_area);
            } else {
                frame.render_widget(address_widget.style(address_style), address_area);
            }
            if tabs_area.height > 0 {
                let mut tab_spans = Vec::new();
                for (i, tab) in state.tabs.iter().enumerate() {
                    let marker = if tab.loading { " *" } else { "" };
                    let style = if i == state.active_tab { theme.selected } else { theme.muted };
                    tab_spans.push(Span::styled(format!(" {}{}{} ", i + 1, tab.title, marker), style));
                }
                let tabs_widget = Paragraph::new(Line::from(tab_spans));
                if tabs_area.height > 1 {
                    frame.render_widget(tabs_widget.block(Block::default().borders(Borders::BOTTOM).border_style(if state.focus == Focus::Tabs { theme.accent } else { theme.border }).border_set(border_set(self.ascii))), tabs_area);
                } else {
                    frame.render_widget(tabs_widget.style(if state.focus == Focus::Tabs { theme.selected } else { theme.base }), tabs_area);
                }
            }
            let tab = state.tab();
            if tab.loading {
                let spinner = spinner(state.loading_frame, self.ascii);
                let label = format!("{spinner} Loading {}", tab.url);
                frame.render_widget(Paragraph::new(if self.ascii { to_ascii(&label) } else { label }).block(Block::default().title(tab.title.as_str()).borders(Borders::ALL).border_style(theme.border).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), content_area);
            } else if let Some(error) = &tab.error {
                let lines = vec![Line::styled("Unable to open this page", theme.error.add_modifier(Modifier::BOLD)), Line::raw(""), Line::raw(error.clone()), Line::raw(""), Line::styled("Edit the address and press Enter to try again.", theme.muted)];
                frame.render_widget(Paragraph::new(Text::from(lines)).block(Block::default().title("Load error").borders(Borders::ALL).border_style(theme.error).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), content_area);
            } else {
                let page_lines = layout_components(&tab.page.root, content_area.width as usize);
                let controls = interactive_targets(&tab.page.root);
                let mut lines = Vec::new();
                for text in page_lines.iter() {
                    let shown = if self.ascii { to_ascii(text) } else { text.clone() };
                    let selected = state.focus == Focus::Content && controls.get(state.selected_interactive).is_some_and(|c| shown.contains(&format!("[{}]", c.id)));
                    lines.push(Line::styled(shown, if selected { theme.selected } else { theme.base }));
                }
                frame.render_widget(Paragraph::new(Text::from(lines)).block(Block::default().title(tab.title.as_str()).borders(Borders::ALL).border_style(if state.focus == Focus::Content { theme.accent } else { theme.border }).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }).scroll((tab.scroll.min(u16::MAX as usize) as u16, 0)), content_area);
            }
            if status_area.height > 0 {
                let label = if state.tab().loading { format!("LOADING  {}", state.tab().url) } else if state.tab().error.is_some() { format!("ERROR  {}", state.status) } else { format!("{}  ·  {}", state.status, state.tab().url) };
                let label = if self.ascii { to_ascii(&label) } else { label };
                frame.render_widget(Paragraph::new(label).style(if state.tab().error.is_some() { theme.error } else { theme.status }), status_area);
            }
            if help_area.height > 0 {
                let help = match state.focus { Focus::Address => "Enter: open   Esc: leave address   Ctrl+L: focus address   Ctrl+Q: quit", Focus::Tabs => "←/→ or ↑/↓: switch tabs   Tab: content   Ctrl+T: new tab   Ctrl+W: close", Focus::Content => "Tab: next control   Enter: activate   ↑/↓: scroll   Ctrl+L: address   ?: help   Ctrl+Q: quit", Focus::Help => "Enter: help   Esc: close   Ctrl+Q: quit" };
                frame.render_widget(Paragraph::new(if self.ascii { to_ascii(help) } else { help.into() }).style(theme.muted), help_area);
            }
            if state.dialog == Some(Dialog::Help) {
                let width = area.width.min(76).max(24); let height = area.height.min(15).max(7);
                let popup = centered(area, width, height);
                let help = vec![Line::styled("Keyboard and mouse", theme.accent.add_modifier(Modifier::BOLD)), Line::raw(""), Line::raw("Tab / Shift+Tab  Move focus between address, tabs, content, help"), Line::raw("Arrow keys      Scroll content or move across tabs"), Line::raw("Enter           Open address or activate selected link/control"), Line::raw("Ctrl+L          Focus the address bar"), Line::raw("Ctrl+T / Ctrl+W New tab / close tab"), Line::raw("[ / ]           Back / forward; r reloads"), Line::raw("Ctrl+Q          Quit"), Line::raw("Mouse           Click links and controls; wheel to scroll"), Line::raw("t               Cycle Auto, Dark, and Light themes"), Line::raw(""), Line::styled("Esc: close this panel", theme.muted)];
                frame.render_widget(Clear, popup);
                frame.render_widget(Paragraph::new(Text::from(help)).block(Block::default().title(" Help ").borders(Borders::ALL).border_style(theme.accent).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), popup);
            }
        })?;
        Ok(())
    }
}

fn begin_navigation(
    state: &mut UiState,
    url: String,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellation: &mut Option<CancellationToken>,
) {
    begin_navigation_target(state, url, client, sender, request_id, cancellation, None)
}

fn begin_navigation_target(
    state: &mut UiState,
    url: String,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellation: &mut Option<CancellationToken>,
    history_target: Option<usize>,
) {
    if url.is_empty() {
        state.status = "Enter an HTTP or HTTPS address".into();
        state.dirty = true;
        return;
    }
    if let Some(token) = cancellation.take() {
        token.cancel();
        for tab in &mut state.tabs {
            tab.loading = false;
        }
    }
    *request_id = request_id.wrapping_add(1);
    let id = *request_id;
    let token = CancellationToken::new();
    *cancellation = Some(token.clone());
    let tab_id = state.tab().id;
    let tab = state.tab_mut();
    tab.url = url.clone();
    tab.loading = true;
    tab.error = None;
    tab.scroll = 0;
    state.address = url.clone();
    state.address_cursor = state.address.chars().count();
    state.status = "Loading page".into();
    state.dirty = true;
    let tx = sender.clone();
    tokio::spawn(async move {
        let result = async {
            let parsed = HttpClient::parse_url(&url)?;
            let source = client.get_text_cancellable(&parsed, token).await?;
            let mut document = html::parse(parsed.as_str(), &source);
            PluginRegistry::default().process(&mut document);
            Ok(transform(&document))
        }
        .await;
        let _ = tx.send(NavigationResult {
            request_id: id,
            tab_id,
            url,
            history_target,
            result,
        });
    });
}

fn interactive_targets(root: &crate::transform::Component) -> Vec<InteractiveTarget> {
    fn collect(component: &crate::transform::Component, out: &mut Vec<InteractiveTarget>) {
        match &component.kind {
            ComponentKind::Link {
                text,
                target,
                interactive_id,
            } => {
                if let Some(id) = interactive_id {
                    out.push(InteractiveTarget {
                        id: id.clone(),
                        label: text.clone(),
                        target: target.clone(),
                        is_button: false,
                        disabled: false,
                    });
                }
            }
            ComponentKind::Input {
                interactive_id,
                label,
                disabled,
                ..
            } => out.push(InteractiveTarget {
                id: interactive_id.clone(),
                label: label.clone(),
                target: None,
                is_button: false,
                disabled: *disabled,
            }),
            ComponentKind::Button {
                interactive_id,
                label,
                disabled,
                ..
            } => out.push(InteractiveTarget {
                id: interactive_id.clone(),
                label: label.clone(),
                target: None,
                is_button: true,
                disabled: *disabled,
            }),
            _ => {}
        }
        for child in &component.children {
            collect(child, out);
        }
    }
    let mut out = Vec::new();
    collect(root, &mut out);
    out
}

#[derive(Clone, Copy)]
struct Palette {
    base: Style,
    border: Style,
    accent: Style,
    brand: Style,
    muted: Style,
    selected: Style,
    focus: Style,
    status: Style,
    error: Style,
}
impl Palette {
    fn new(theme: Theme) -> Self {
        let mut palette = match theme {
            Theme::Auto => Self {
                base: Style::default().fg(Color::Reset),
                border: Style::default().add_modifier(Modifier::DIM),
                accent: Style::default().add_modifier(Modifier::BOLD),
                brand: Style::default().add_modifier(Modifier::BOLD),
                muted: Style::default().add_modifier(Modifier::DIM),
                selected: Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
                focus: Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
                status: Style::default().add_modifier(Modifier::BOLD),
                error: Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            },
            Theme::Dark => Self {
                base: Style::default().fg(Color::Gray),
                border: Style::default().fg(Color::DarkGray),
                accent: Style::default().fg(Color::LightCyan),
                brand: Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
                muted: Style::default().fg(Color::Gray),
                selected: Style::default()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
                focus: Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
                status: Style::default().fg(Color::LightGreen),
                error: Style::default().fg(Color::LightRed),
            },
            Theme::Light => Self {
                base: Style::default().fg(Color::Black),
                border: Style::default().fg(Color::Gray),
                accent: Style::default().fg(Color::Blue),
                brand: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                muted: Style::default().fg(Color::DarkGray),
                selected: Style::default()
                    .fg(Color::White)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                focus: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                status: Style::default().fg(Color::Green),
                error: Style::default().fg(Color::Red),
            },
        };
        if std::env::var_os("NO_COLOR").is_some() {
            palette.base = Style::default();
            palette.border = Style::default().add_modifier(Modifier::DIM);
            palette.accent = Style::default().add_modifier(Modifier::BOLD);
            palette.brand = Style::default().add_modifier(Modifier::BOLD);
            palette.muted = Style::default().add_modifier(Modifier::DIM);
            palette.selected = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
            palette.focus = Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
            palette.status = Style::default().add_modifier(Modifier::BOLD);
            palette.error = Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        palette
    }
}

fn address_window(text: &str, cursor: usize, width: usize, focused: bool, ascii: bool) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let marker = if focused {
        Some(if ascii { '|' } else { '▏' })
    } else {
        None
    };
    let marker_width = usize::from(marker.is_some());
    if chars.len() + marker_width <= width {
        let mut visible = chars;
        if let Some(marker) = marker {
            visible.insert(cursor, marker);
        }
        return visible.into_iter().collect();
    }
    let prefix_width = usize::from(cursor > width.saturating_sub(marker_width));
    let capacity = width.saturating_sub(marker_width + prefix_width).max(1);
    let start = cursor.saturating_sub(capacity);
    let end = (start + capacity).min(chars.len());
    let mut visible = chars[start..end].to_vec();
    let insertion = cursor.saturating_sub(start).min(visible.len());
    if let Some(marker) = marker {
        visible.insert(insertion, marker);
    }
    let mut result = String::new();
    if prefix_width > 0 {
        result.push(if ascii { '<' } else { '…' });
    }
    result.extend(visible);
    result
}
fn spinner(frame: usize, ascii: bool) -> &'static str {
    if ascii {
        ["|", "/", "-", "\\"][frame % 4]
    } else {
        ["◐", "◓", "◑", "◒"][frame % 4]
    }
}
fn ascii_only() -> bool {
    std::env::var("TERM").is_ok_and(|term| term == "dumb")
        || std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .is_ok_and(|locale| {
                !locale.to_ascii_lowercase().contains("utf-8")
                    && !locale.to_ascii_lowercase().contains("utf8")
            })
}
fn mouse_supported() -> bool {
    !std::env::var("TERM").is_ok_and(|term| term == "dumb")
}
fn border_set(ascii: bool) -> BorderSet {
    if ascii {
        BorderSet {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        }
    } else {
        border::PLAIN
    }
}
fn to_ascii(value: &str) -> String {
    value
        .replace("↗", "->")
        .replace("←", "<-")
        .replace("→", "->")
        .replace("↑", "^")
        .replace("↓", "v")
        .replace("•", "*")
        .replace("│", "|")
        .replace("┌", "+")
        .replace("─", "-")
        .replace("└", "+")
        .replace("✓", "OK")
        .replace("×", "x")
        .replace("…", "...")
        .replace("·", "|")
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse_enabled {
            let _ = execute!(
                self.terminal.backend_mut(),
                DisableMouseCapture,
                LeaveAlternateScreen
            );
        } else {
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        }
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::{Focus, Theme, UiAction, UiState};
    use crate::{dom::Document, html, transform::transform};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    fn state() -> UiState {
        let doc = html::parse(
            "https://example.com",
            "<nav><a href='/home'>Home</a></nav><p>Body</p>",
        );
        UiState::new(transform(&doc))
    }

    #[test]
    fn address_shortcut_typing_and_enter_emit_navigation() {
        let mut ui = state();
        ui.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
        assert_eq!(ui.focus, Focus::Address);
        for ch in "https://example.org".chars() {
            ui.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            UiAction::Navigate("https://example.org".into())
        );
    }

    #[test]
    fn address_cursor_window_keeps_long_urls_editable() {
        let text = "https://example.org/a/very/long/path/that/exceeds/the/address/area";
        let visible = super::address_window(text, text.chars().count(), 18, true, true);
        assert!(visible.starts_with('<'));
        assert!(visible.ends_with('|'));
        assert_eq!(visible.chars().count(), 18);
    }

    #[test]
    fn focus_cycles_and_shift_tab_reverses() {
        let mut ui = state();
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Help);
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(ui.focus, Focus::Content);
    }

    #[test]
    fn tab_moves_between_controls_before_leaving_content() {
        let doc = html::parse(
            "https://example.com",
            "<a href='/one'>One</a><a href='/two'>Two</a><button>Run</button>",
        );
        let mut ui = UiState::new(transform(&doc));
        ui.focus = Focus::Content;
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Content);
        assert_eq!(ui.selected_interactive, 1);
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ui.selected_interactive, 2);
        ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(ui.focus, Focus::Help);
        ui.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(ui.focus, Focus::Content);
        assert_eq!(ui.selected_interactive, 2);
    }

    #[test]
    fn mouse_focuses_address_and_selects_content_links() {
        let mut ui = state();
        let area = ratatui::layout::Rect::new(0, 0, 80, 24);
        ui.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
        assert_eq!(ui.focus, Focus::Address);
        let action = ui.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 10,
                row: 7,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
        assert_eq!(ui.focus, Focus::Content);
        assert_eq!(ui.selected_interactive, 0);
        assert_eq!(
            action,
            UiAction::Navigate("https://example.com/home".into())
        );
    }

    #[test]
    fn enter_activates_selected_link_and_theme_cycles() {
        let mut ui = state();
        ui.focus = Focus::Content;
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            UiAction::Navigate("https://example.com/home".into())
        );
        ui.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(ui.theme, Theme::Dark);
        ui.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        assert_eq!(ui.theme, Theme::Light);
    }

    #[test]
    fn help_escape_and_quit_are_explicit() {
        let mut ui = state();
        ui.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
        assert!(ui.dialog.is_some());
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(ui.dialog.is_none());
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            UiAction::Quit
        );
    }

    #[test]
    fn help_focus_opens_dialog_and_escape_closes_it() {
        let mut ui = state();
        ui.focus = Focus::Help;
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(ui.dialog.is_some());
        ui.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(ui.dialog.is_none());
    }

    #[test]
    fn empty_pages_construct_a_ready_state() {
        let ui = UiState::new(transform(&Document::new("", "New tab")));
        assert_eq!(ui.tabs.len(), 1);
        assert_eq!(ui.status, "Ready");
    }
}
