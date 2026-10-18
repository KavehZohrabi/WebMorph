use std::{
    collections::HashMap,
    io::{self, Stdout},
    process::Command,
    sync::Arc,
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
use zeroize::Zeroizing;

use crate::{
    browser::BrowserBackend,
    dom::{Document, FormControl, FormDefinition},
    error::{Result, WebMorphError},
    html,
    network::HttpClient,
    plugins::PluginRegistry,
    storage::Storage,
    transform::{
        ComponentKind, TuiPage, layout_components, strip_terminal_controls,
        strip_terminal_controls_in_place, transform,
    },
};

const TICK: Duration = Duration::from_millis(35);
const MAX_LAYOUT_CACHE_BYTES: usize = 8 * 1024 * 1024;
type FormValues = HashMap<String, Zeroizing<String>>;

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
    ConfirmSubmit {
        form_id: String,
        submitter: Option<String>,
    },
}

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
    selected_interactive: usize,
    request_id: u64,
    form_values: FormValues,
    layout_cache: Option<(usize, u64, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InteractiveTarget {
    id: String,
    label: String,
    target: Option<String>,
    is_button: bool,
    disabled: bool,
    form_control: Option<FormControl>,
    form: Option<FormDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiAction {
    None,
    Quit,
    Navigate(String),
    NavigateNewTab(String),
    OpenExternal(String),
    Back,
    Forward,
    Reload,
    Home,
    NewTab,
    CloseTab,
    NextTab,
    PreviousTab,
    SubmitForm {
        form_id: String,
        submitter: Option<String>,
    },
}

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
    editing_field: Option<String>,
    browser_backend: Option<Arc<dyn BrowserBackend>>,
}

impl UiState {
    fn new(page: TuiPage) -> Self {
        let address = match &page.root.kind {
            ComponentKind::Page { url, .. } => url.clone(),
            _ => String::new(),
        };
        let form_values = default_form_values(&page.forms);
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
            selected_interactive: 0,
            request_id: 0,
            form_values,
            layout_cache: None,
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
            editing_field: None,
            browser_backend: None,
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
            self.editing_field = None;
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
        if ctrl && key.code == KeyCode::Tab {
            return if shift {
                UiAction::PreviousTab
            } else {
                UiAction::NextTab
            };
        }
        if self.dialog.is_some() {
            match self.dialog.clone() {
                Some(Dialog::ConfirmSubmit { form_id, submitter }) => match key.code {
                    KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                        self.dialog = None;
                        return UiAction::SubmitForm { form_id, submitter };
                    }
                    KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                        self.dialog = None;
                        self.status = "Form submission cancelled".into();
                        self.dirty = true;
                    }
                    _ => {}
                },
                Some(Dialog::Help)
                    if matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('?')) =>
                {
                    self.dialog = None;
                    self.dirty = true;
                }
                _ => {}
            }
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::F(1) | KeyCode::Char('?')) {
            self.dialog = Some(Dialog::Help);
            self.dirty = true;
            return UiAction::None;
        }
        if key.code == KeyCode::Esc {
            if self.editing_field.take().is_some() {
                self.status = "Field editing finished".into();
                self.dirty = true;
                return UiAction::None;
            }
            if self.tab().loading {
                self.status = "Cancelling request…".into();
                return UiAction::Navigate(String::new());
            }
            self.focus = Focus::Content;
            self.dirty = true;
            return UiAction::None;
        }
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.editing_field = None;
            self.move_focus(key.code != KeyCode::BackTab && !shift);
            self.dirty = true;
            return UiAction::None;
        }
        if self.focus == Focus::Address {
            return self.address_key(key);
        }
        if let Some(id) = self.editing_field.clone() {
            self.edit_form_field(&id, key);
            return UiAction::None;
        }
        if let Some(control) = self.selected_select() {
            if key.code == KeyCode::Up {
                self.cycle_select(&control, -1);
                return UiAction::None;
            }
            if key.code == KeyCode::Down {
                self.cycle_select(&control, 1);
                return UiAction::None;
            }
        }
        match key.code {
            KeyCode::Char(' ') if self.focus == Focus::Content => self.activate(false),
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => self.external_target(),
            KeyCode::Enter => self.activate(shift),
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
            KeyCode::Char('h' | 'H') => UiAction::Home,
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

    fn activate(&mut self, open_new_tab: bool) -> UiAction {
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
        let targets = interactive_targets(&self.tab().page);
        let Some(target) = targets.get(self.selected_interactive).cloned() else {
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
            if open_new_tab {
                UiAction::NavigateNewTab(url.clone())
            } else {
                UiAction::Navigate(url.clone())
            }
        } else if let Some(control) = target.form_control.clone() {
            match control.control_type.as_str() {
                "checkbox" => {
                    let current = self
                        .tab()
                        .form_values
                        .get(&control.interactive_id)
                        .map(|value| value.as_str())
                        .unwrap_or(if control.checked { "true" } else { "false" });
                    let toggled = (!matches!(current, "true" | "on" | "1")).to_string();
                    self.tab_mut()
                        .form_values
                        .insert(control.interactive_id.clone(), Zeroizing::new(toggled));
                    self.dirty = true;
                    UiAction::None
                }
                "radio" => {
                    if let (Some(form), Some(name)) = (&target.form, &control.name) {
                        for field in form.controls.iter().filter(|field| {
                            field.control_type == "radio" && field.name.as_ref() == Some(name)
                        }) {
                            self.tab_mut().form_values.insert(
                                field.interactive_id.clone(),
                                Zeroizing::new("false".into()),
                            );
                        }
                    }
                    self.tab_mut().form_values.insert(
                        control.interactive_id.clone(),
                        Zeroizing::new("true".into()),
                    );
                    self.dirty = true;
                    UiAction::None
                }
                "select" => {
                    if control.multiple {
                        self.status =
                            format!("{}: multi-select controls are not supported", target.label);
                        self.dirty = true;
                        return UiAction::None;
                    }
                    self.cycle_select(&control, 1);
                    UiAction::None
                }
                "file" => {
                    self.status = "File uploads are not supported".into();
                    self.dirty = true;
                    UiAction::None
                }
                "submit" | "image" => {
                    self.request_form_submit(&target, Some(control.interactive_id))
                }
                "reset" => {
                    if let Some(form) = &target.form {
                        for field in &form.controls {
                            let default = if field.control_type == "password" {
                                String::new()
                            } else {
                                field.value.clone()
                            };
                            self.tab_mut()
                                .form_values
                                .insert(field.interactive_id.clone(), Zeroizing::new(default));
                        }
                    }
                    self.dirty = true;
                    UiAction::None
                }
                "button" => {
                    self.status = "JavaScript-driven buttons are not supported".into();
                    self.dirty = true;
                    UiAction::None
                }
                _ => {
                    self.editing_field = Some(control.interactive_id.clone());
                    self.status = format!("Editing {}; Enter or Esc finishes", target.label);
                    self.dirty = true;
                    UiAction::None
                }
            }
        } else if target.is_button {
            self.status = "JavaScript-driven buttons are not supported".into();
            self.dirty = true;
            UiAction::None
        } else {
            self.status = format!("Selected {}", target.label);
            self.dirty = true;
            UiAction::None
        }
    }

    fn request_form_submit(
        &mut self,
        target: &InteractiveTarget,
        submitter: Option<String>,
    ) -> UiAction {
        let Some(form) = &target.form else {
            self.status = "This control is not associated with a supported form".into();
            self.dirty = true;
            return UiAction::None;
        };
        if let Some(error) = validate_form(form, &self.tab().form_values) {
            self.status = error;
            self.dirty = true;
            return UiAction::None;
        }
        if form.method == "get"
            && form
                .controls
                .iter()
                .any(|field| field.control_type == "password")
        {
            self.status = "Password forms cannot use GET; use a POST form instead".into();
            self.dirty = true;
            return UiAction::None;
        }
        let sensitive = form.method == "post"
            || form
                .controls
                .iter()
                .any(|field| field.control_type == "password");
        if sensitive {
            self.dialog = Some(Dialog::ConfirmSubmit {
                form_id: form.id.clone(),
                submitter,
            });
            self.dirty = true;
            UiAction::None
        } else {
            UiAction::SubmitForm {
                form_id: form.id.clone(),
                submitter,
            }
        }
    }

    fn edit_form_field(&mut self, id: &str, key: KeyEvent) {
        let control = self
            .tab()
            .page
            .forms
            .iter()
            .flat_map(|form| &form.controls)
            .find(|field| field.interactive_id == id)
            .cloned();
        let Some(control) = control else {
            self.editing_field = None;
            return;
        };
        let mut value = self
            .tab()
            .form_values
            .get(id)
            .cloned()
            .unwrap_or_else(|| Zeroizing::new(String::new()));
        if key.code == KeyCode::Enter
            && control.control_type == "textarea"
            && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            value.push('\n');
            self.tab_mut().form_values.insert(id.to_owned(), value);
            self.dirty = true;
            return;
        } else if key.code == KeyCode::Enter || key.code == KeyCode::Esc {
            self.editing_field = None;
            self.dirty = true;
            return;
        }
        match key.code {
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if control
                    .maxlength
                    .is_none_or(|max| value.chars().count() < max)
                {
                    value.push(ch);
                }
            }
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Delete => {
                value.clear();
            }
            _ => return,
        }
        self.tab_mut().form_values.insert(id.to_owned(), value);
        self.dirty = true;
    }

    fn selected_select(&self) -> Option<FormControl> {
        interactive_targets(&self.tab().page)
            .get(self.selected_interactive)
            .and_then(|target| target.form_control.clone())
            .filter(|control| control.control_type == "select")
    }

    fn cycle_select(&mut self, control: &FormControl, delta: isize) {
        if control.options.is_empty() {
            self.status = "This select has no available choices".into();
            return;
        }
        let current = self
            .tab()
            .form_values
            .get(&control.interactive_id)
            .cloned()
            .map(|value| value.to_string())
            .unwrap_or_else(|| control.value.clone());
        let index = control
            .options
            .iter()
            .position(|(value, _)| *value == current)
            .unwrap_or(0);
        let next = (index as isize + delta).rem_euclid(control.options.len() as isize) as usize;
        self.tab_mut().form_values.insert(
            control.interactive_id.clone(),
            Zeroizing::new(control.options[next].0.clone()),
        );
        self.status = format!("{}: {}", control.label, control.options[next].1);
        self.dirty = true;
    }

    fn external_target(&mut self) -> UiAction {
        if self.focus != Focus::Content {
            return UiAction::None;
        }
        let targets = interactive_targets(&self.tab().page);
        let Some(target) = targets.get(self.selected_interactive) else {
            return UiAction::None;
        };
        if target.disabled {
            return UiAction::None;
        }
        if let Some(url) = &target.target {
            UiAction::OpenExternal(url.clone())
        } else {
            UiAction::None
        }
    }

    fn move_focus(&mut self, forward: bool) {
        if self.focus == Focus::Content {
            let count = interactive_targets(&self.tab().page).len();
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
                self.selected_interactive = interactive_targets(&self.tab().page)
                    .len()
                    .saturating_sub(1);
            }
        }
        if self.focus == Focus::Address {
            self.address_replacing = true;
        }
    }

    fn select_tab(&mut self, delta: isize) {
        self.tab_mut().selected_interactive = self.selected_interactive;
        self.tab_mut().layout_cache = None;
        self.active_tab =
            (self.active_tab as isize + delta).rem_euclid(self.tabs.len() as isize) as usize;
        self.address = self.tab().url.clone();
        self.address_cursor = self.address.chars().count();
        self.selected_interactive = self.tab().selected_interactive;
        self.focus = Focus::Tabs;
        self.dirty = true;
    }

    fn append_new_tab(&mut self, url: String) {
        self.tab_mut().layout_cache = None;
        let page = transform(&Document::new(url.clone(), "New tab"));
        let form_values = default_form_values(&page.forms);
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(BrowserTab {
            id,
            title: "New tab".into(),
            url: String::new(),
            page,
            error: None,
            loading: false,
            scroll: 0,
            history: vec![],
            history_index: 0,
            selected_interactive: 0,
            request_id: 0,
            form_values,
            layout_cache: None,
        });
        self.active_tab = self.tabs.len() - 1;
        self.address.clear();
        self.address_cursor = 0;
        self.address_replacing = false;
        self.selected_interactive = 0;
        self.focus = Focus::Address;
        self.dirty = true;
        if !url.is_empty() {
            self.address = url.clone();
            self.address_cursor = url.chars().count();
            self.address_replacing = true;
        }
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
                        self.tab_mut().layout_cache = None;
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
                        let targets = interactive_targets(&self.tab().page);
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
                                if target.is_button || target.form_control.is_some() {
                                    self.focus = Focus::Content;
                                    if target.form_control.as_ref().is_some_and(|field| {
                                        !matches!(
                                            field.control_type.as_str(),
                                            "checkbox"
                                                | "radio"
                                                | "select"
                                                | "submit"
                                                | "reset"
                                                | "button"
                                        )
                                    }) {
                                        self.editing_field = Some(target.id.clone());
                                    }
                                    return self.activate(false);
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
    history_target: Option<usize>,
    result: std::result::Result<(TuiPage, String), WebMorphError>,
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
    pub async fn run_url(
        &mut self,
        url: String,
        client: HttpClient,
        storage: Storage,
        browser_backend: Option<Arc<dyn BrowserBackend>>,
    ) -> Result<()> {
        let placeholder = transform(&Document::new(url.clone(), "Loading"));
        let state = UiState::new(placeholder);
        let mut state = state;
        state.browser_backend = browser_backend;
        self.run_state(state, Some((url, client, Some(storage))))
            .await
    }

    async fn run_state(
        &mut self,
        mut state: UiState,
        initial: Option<(String, HttpClient, Option<Storage>)>,
    ) -> Result<()> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<NavigationResult>();
        let mut request_id = 0u64;
        let mut cancellations: HashMap<u64, CancellationToken> = HashMap::new();
        let mut initial = initial;
        let mut storage = initial.as_mut().and_then(|(_, _, storage)| storage.take());
        if let Some((url, client, _)) = initial {
            begin_navigation(
                &mut state,
                url,
                client,
                &sender,
                &mut request_id,
                &mut cancellations,
            );
        }
        let mut ticker = tokio::time::interval(TICK);
        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        loop {
            if state.dirty {
                let width = self.terminal.size()?.width.saturating_sub(2).max(1) as usize;
                for (index, tab) in state.tabs.iter_mut().enumerate() {
                    if index != state.active_tab {
                        tab.layout_cache = None;
                    }
                }
                let tab = state.tab_mut();
                if tab
                    .layout_cache
                    .as_ref()
                    .is_none_or(|(cached_width, request, _)| {
                        *cached_width != width || *request != tab.request_id
                    })
                {
                    let lines = layout_components(&tab.page.root, width);
                    let bytes = lines
                        .iter()
                        .fold(0usize, |total, line| total.saturating_add(line.len()));
                    tab.layout_cache =
                        (bytes <= MAX_LAYOUT_CACHE_BYTES).then_some((width, tab.request_id, lines));
                }
                self.draw(&state)?;
                state.dirty = false;
            }
            tokio::select! {
                signal = &mut shutdown => { signal?; break; }
                Some(result) = receiver.recv() => {
                    if let Some(index) = state.tabs.iter().position(|tab| tab.id == result.tab_id && tab.request_id == result.request_id) {
                            cancellations.remove(&result.tab_id);
                            let is_active = index == state.active_tab;
                            let tab = &mut state.tabs[index]; tab.loading = false; tab.error = None;
                            let status = match result.result {
                                Ok((page, final_url)) => {
                                    if let Some(storage) = storage.as_mut() {
                                        if let Err(error) = storage.record_visit(&final_url, &page.title) {
                                            tracing::warn!(%error, "failed to persist browsing history");
                                        }
                                    }
                                    tab.title = page.title.clone(); tab.url = final_url.clone(); tab.form_values = default_form_values(&page.forms); tab.page = page; tab.scroll = 0; tab.layout_cache = None;
                                    tab.selected_interactive = 0;
                                    if let Some(index) = result.history_target { tab.history_index = index; tab.history[index] = final_url.clone(); }
                                    else { tab.history.truncate(tab.history_index.saturating_add(1)); if tab.history.last() != Some(&final_url) { tab.history.push(final_url.clone()); } tab.history_index = tab.history.len().saturating_sub(1); }
                                    "Page loaded"
                                }
                                Err(WebMorphError::Cancelled) => "Request cancelled",
                                Err(error) => { tab.error = Some(error.to_string()); "Unable to load page" }
                            };
                            if is_active {
                                if let Some(tab) = state.tabs.iter().find(|tab| tab.id == result.tab_id) { state.address = tab.url.clone(); }
                                if status == "Page loaded" { state.selected_interactive = 0; }
                                state.status = status.into();
                                state.focus = if status == "Unable to load page" { Focus::Address } else { Focus::Content };
                                state.address_replacing = status == "Unable to load page";
                            }
                            state.dirty = true;
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
                                    UiAction::Navigate(url) if url.is_empty() => { if let Some(token) = cancellations.remove(&state.tab().id) { token.cancel(); } state.tab_mut().loading = false; state.status = "Request cancelled".into(); state.dirty = true; }
                                    UiAction::Navigate(url) => begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations),
                                    UiAction::NavigateNewTab(url) => { state.append_new_tab(url.clone()); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations); }
                                    UiAction::OpenExternal(url) => { state.status = match open_external(&url) { Ok(()) => "Opened link in system browser".into(), Err(error) => format!("Could not open external link: {error}") }; state.dirty = true; }
                                    UiAction::Reload => { let url = state.tab().url.clone(); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations); }
                                    UiAction::Home => { if let Ok(mut url) = HttpClient::parse_url(&state.tab().url) { url.set_path("/"); url.set_query(None); url.set_fragment(None); begin_navigation(&mut state, url.to_string(), HttpClient::default(), &sender, &mut request_id, &mut cancellations); } }
                                    UiAction::Back => { if state.tab().history_index > 0 { let target = state.tab().history_index - 1; let url = state.tab().history[target].clone(); begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations, Some(target)); } }
                                    UiAction::Forward => { if state.tab().history_index + 1 < state.tab().history.len() { let target = state.tab().history_index + 1; let url = state.tab().history[target].clone(); begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations, Some(target)); } }
                                    UiAction::NewTab => { state.append_new_tab(String::new()); }
                                    UiAction::NextTab => state.select_tab(1),
                                    UiAction::PreviousTab => state.select_tab(-1),
                                    UiAction::SubmitForm { form_id, submitter } => begin_form_submission(&mut state, &form_id, submitter.as_deref(), HttpClient::default(), &sender, &mut request_id, &mut cancellations),
                                    UiAction::CloseTab => { if state.tabs.len() > 1 { let removed = state.tabs.remove(state.active_tab); if let Some(token) = cancellations.remove(&removed.id) { token.cancel(); } state.active_tab = state.active_tab.min(state.tabs.len()-1); state.address = state.tab().url.clone(); state.selected_interactive = state.tab().selected_interactive; state.dirty = true; } else { break; } }
                                    UiAction::None => {}
                                }
                            }
                            Event::Mouse(mouse) => {
                                let size = self.terminal.size()?;
                                let area = Rect::new(0, 0, size.width, size.height);
                                let action = state.handle_mouse(mouse, area);
                                match action {
                                    UiAction::Navigate(url) => begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations),
                                    UiAction::NavigateNewTab(url) => { state.append_new_tab(url.clone()); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations); }
                                    UiAction::OpenExternal(url) => { state.status = match open_external(&url) { Ok(()) => "Opened link in system browser".into(), Err(error) => format!("Could not open external link: {error}") }; state.dirty = true; }
                                    UiAction::Back => {
                                        if state.tab().history_index > 0 {
                                            let target = state.tab().history_index - 1;
                                            let url = state.tab().history[target].clone();
                                            begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations, Some(target));
                                        }
                                    }
                                    UiAction::Forward => {
                                        if state.tab().history_index + 1 < state.tab().history.len() {
                                            let target = state.tab().history_index + 1;
                                            let url = state.tab().history[target].clone();
                                            begin_navigation_target(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations, Some(target));
                                        }
                                    }
                                    UiAction::Reload => { let url = state.tab().url.clone(); begin_navigation(&mut state, url, HttpClient::default(), &sender, &mut request_id, &mut cancellations); }
                                    UiAction::Home => { if let Ok(mut url) = HttpClient::parse_url(&state.tab().url) { url.set_path("/"); url.set_query(None); url.set_fragment(None); begin_navigation(&mut state, url.to_string(), HttpClient::default(), &sender, &mut request_id, &mut cancellations); } }
                                    UiAction::NextTab => state.select_tab(1),
                                    UiAction::PreviousTab => state.select_tab(-1),
                                    UiAction::SubmitForm { form_id, submitter } => begin_form_submission(&mut state, &form_id, submitter.as_deref(), HttpClient::default(), &sender, &mut request_id, &mut cancellations),
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
        for token in cancellations.into_values() {
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
                    let title = strip_terminal_controls(&tab.title);
                    tab_spans.push(Span::styled(format!(" {}{}{} ", i + 1, title, marker), style));
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
                let label = strip_terminal_controls(&format!("{spinner} Loading {}", tab.url));
                let safe_title = strip_terminal_controls(&tab.title);
                frame.render_widget(Paragraph::new(if self.ascii { to_ascii(&label) } else { label }).block(Block::default().title(safe_title).borders(Borders::ALL).border_style(theme.border).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), content_area);
            } else if let Some(error) = &tab.error {
                let safe_error = strip_terminal_controls(error);
                let lines = vec![Line::styled("Unable to open this page", theme.error.add_modifier(Modifier::BOLD)), Line::raw(""), Line::raw(safe_error), Line::raw(""), Line::styled("Edit the address and press Enter to try again.", theme.muted)];
                frame.render_widget(Paragraph::new(Text::from(lines)).block(Block::default().title("Load error").borders(Borders::ALL).border_style(theme.error).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), content_area);
            } else {
                let fallback_lines;
                let page_lines = if let Some((_, _, lines)) = &tab.layout_cache {
                    lines.as_slice()
                } else {
                    fallback_lines = layout_components(&tab.page.root, content_area.width as usize);
                    fallback_lines.as_slice()
                };
                let controls = interactive_targets(&tab.page);
                let mut lines = Vec::new();
                let visible_rows = content_area.height.saturating_sub(2) as usize;
                for text in visible_window(page_lines, tab.scroll, visible_rows) {
                    let mut shown = render_form_line(text, &controls, tab);
                    strip_terminal_controls_in_place(&mut shown);
                    let shown = if self.ascii { to_ascii(&shown) } else { shown };
                    let selected = state.focus == Focus::Content && controls.get(state.selected_interactive).is_some_and(|c| shown.contains(&format!("[{}]", c.id)));
                    lines.push(Line::styled(shown, if selected { theme.selected } else { theme.base }));
                }
                let safe_title = strip_terminal_controls(&tab.title);
                frame.render_widget(Paragraph::new(Text::from(lines)).block(Block::default().title(safe_title).borders(Borders::ALL).border_style(if state.focus == Focus::Content { theme.accent } else { theme.border }).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), content_area);
            }
            if status_area.height > 0 {
                let label = if state.tab().loading { format!("LOADING  {}", state.tab().url) } else if state.tab().error.is_some() { format!("ERROR  {}", state.status) } else { format!("{}  ·  {}", state.status, state.tab().url) };
                let label = strip_terminal_controls(&label);
                let label = if self.ascii { to_ascii(&label) } else { label };
                frame.render_widget(Paragraph::new(label).style(if state.tab().error.is_some() { theme.error } else { theme.status }), status_area);
            }
            if help_area.height > 0 {
                let help = match state.focus { Focus::Address => "Enter: open   Esc: leave address   Ctrl+L: focus address   Ctrl+Q: quit", Focus::Tabs => "←/→ or ↑/↓: switch tabs   Ctrl+Tab: cycle   Ctrl+T: new tab   Ctrl+W: close", Focus::Content => "Tab: next control   Enter: open   Shift+Enter: new tab   Alt+Enter: system browser   Ctrl+L: address", Focus::Help => "Enter: help   Esc: close   Ctrl+Q: quit" };
                frame.render_widget(Paragraph::new(if self.ascii { to_ascii(help) } else { help.into() }).style(theme.muted), help_area);
            }
            if state.dialog == Some(Dialog::Help) {
                let width = area.width.min(76).max(24); let height = area.height.min(21).max(7);
                let popup = centered(area, width, height);
                let help = vec![Line::styled("Keyboard and mouse", theme.accent.add_modifier(Modifier::BOLD)), Line::raw(""), Line::raw("Tab / Shift+Tab  Move between controls"), Line::raw("Enter           Edit a field or activate a control"), Line::raw("Space           Toggle a checkbox or radio option"), Line::raw("Arrow keys      Scroll pages; choose select options"), Line::raw("Shift+Enter     Open a selected link in a new tab"), Line::raw("Alt+Enter       Open a selected link in system browser"), Line::raw("Ctrl+L          Focus the address bar"), Line::raw("Ctrl+T / Ctrl+W New tab / close tab"), Line::raw("Ctrl+Tab        Next tab; Ctrl+Shift+Tab previous tab"), Line::raw("h               Home (site root)"), Line::raw("[ / ]           Back / forward; r reloads"), Line::raw("Ctrl+Q          Quit"), Line::raw("Mouse           Click fields and controls; wheel to scroll"), Line::raw("POST submissions ask for confirmation"), Line::raw(""), Line::styled("Esc: close this panel", theme.muted)];
                frame.render_widget(Clear, popup);
                frame.render_widget(Paragraph::new(Text::from(help)).block(Block::default().title(" Help ").borders(Borders::ALL).border_style(theme.accent).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), popup);
            } else if matches!(state.dialog, Some(Dialog::ConfirmSubmit { .. })) {
                let width = area.width.min(62).max(24); let height = area.height.min(7).max(5);
                let popup = centered(area, width, height);
                let text = vec![Line::styled("Confirm form submission", theme.error.add_modifier(Modifier::BOLD)), Line::raw("This may send sensitive information to the website."), Line::raw("Press Enter or y to submit; Esc or n to cancel.")];
                frame.render_widget(Clear, popup);
                frame.render_widget(Paragraph::new(Text::from(text)).block(Block::default().title(" Confirm ").borders(Borders::ALL).border_style(theme.error).border_set(border_set(self.ascii))).wrap(Wrap { trim: true }), popup);
            }
        })?;
        Ok(())
    }
}

fn visible_window<T>(items: &[T], start: usize, count: usize) -> &[T] {
    let start = start.min(items.len());
    let end = start.saturating_add(count).min(items.len());
    &items[start..end]
}

fn begin_navigation(
    state: &mut UiState,
    url: String,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellations: &mut HashMap<u64, CancellationToken>,
) {
    begin_navigation_target(state, url, client, sender, request_id, cancellations, None)
}

fn begin_navigation_target(
    state: &mut UiState,
    url: String,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellations: &mut HashMap<u64, CancellationToken>,
    history_target: Option<usize>,
) {
    begin_navigation_request(
        state,
        url,
        client,
        sender,
        request_id,
        cancellations,
        history_target,
        None,
    );
}

fn begin_navigation_request(
    state: &mut UiState,
    url: String,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellations: &mut HashMap<u64, CancellationToken>,
    history_target: Option<usize>,
    post_fields: Option<Vec<(String, String)>>,
) {
    if post_fields.is_some() && state.browser_backend.is_some() {
        state.status = "Chromium mode does not support form submission yet".into();
        state.dirty = true;
        return;
    }
    if url.trim().is_empty() {
        state.status = "Enter an HTTP or HTTPS address".into();
        state.dirty = true;
        return;
    }
    let tab_id = state.tab().id;
    if state.tab().loading && state.tab().url == url.trim() {
        return;
    }
    if let Some(token) = cancellations.remove(&tab_id) {
        token.cancel();
    }
    *request_id = request_id.wrapping_add(1);
    let id = *request_id;
    let parsed_url = match normalize_address(url.trim()) {
        Ok(url) => url,
        Err(error) => {
            let tab = state.tab_mut();
            tab.request_id = id;
            tab.loading = false;
            tab.error = Some(error.to_string());
            state.status = "Invalid address".into();
            state.dirty = true;
            return;
        }
    };
    let url = parsed_url.to_string();
    let token = CancellationToken::new();
    let tab = state.tab_mut();
    tab.request_id = id;
    tab.url = url.clone();
    tab.loading = true;
    tab.error = None;
    tab.scroll = 0;
    state.address = url.clone();
    state.address_cursor = state.address.chars().count();
    state.status = "Loading page".into();
    state.dirty = true;
    let tx = sender.clone();
    cancellations.insert(tab_id, token.clone());
    let browser_backend = state.browser_backend.clone();
    tokio::spawn(async move {
        let result = async {
            let parsed = HttpClient::parse_url(&url)?;
            let (mut document, final_url) = if let Some(fields) = post_fields {
                let (final_url, source) = client
                    .submit_form_cancellable(&parsed, fields, token)
                    .await?;
                (
                    html::parse(final_url.as_str(), &source),
                    final_url.to_string(),
                )
            } else if let Some(browser) = browser_backend {
                let (document, final_url) = browser.render(&url, token).await?;
                (document, final_url)
            } else {
                let (final_url, source) = client
                    .get_text_with_final_url_cancellable(&parsed, token)
                    .await?;
                (
                    html::parse(final_url.as_str(), &source),
                    final_url.to_string(),
                )
            };
            let report = PluginRegistry::default().process(&mut document);
            let mut page = transform(&document);
            crate::plugins::apply_report_to_page(&mut page, report);
            Ok((page, final_url))
        }
        .await;
        let _ = tx.send(NavigationResult {
            request_id: id,
            tab_id,
            history_target,
            result,
        });
    });
}

fn normalize_address(input: &str) -> Result<String> {
    let has_scheme = input
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|prefix| prefix.contains(':'));
    let candidate = if has_scheme {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    HttpClient::parse_url(&candidate).map(|url| url.to_string())
}

fn begin_form_submission(
    state: &mut UiState,
    form_id: &str,
    submitter: Option<&str>,
    client: HttpClient,
    sender: &mpsc::UnboundedSender<NavigationResult>,
    request_id: &mut u64,
    cancellations: &mut HashMap<u64, CancellationToken>,
) {
    let Some(form) = state
        .tab()
        .page
        .forms
        .iter()
        .find(|form| form.id == form_id)
        .cloned()
    else {
        state.status = "Form is no longer available".into();
        state.dirty = true;
        return;
    };
    if let Some(error) = validate_form(&form, &state.tab().form_values) {
        state.status = error;
        state.dirty = true;
        return;
    }
    let Ok(mut action) = HttpClient::parse_url(&form.action) else {
        state.status = "Form action URL is invalid".into();
        state.dirty = true;
        return;
    };
    let fields = submitted_fields(&form, &state.tab().form_values, submitter);
    if form.method == "get" {
        action = HttpClient::append_form_query(&action, &fields);
        begin_navigation_request(
            state,
            action.to_string(),
            client,
            sender,
            request_id,
            cancellations,
            None,
            None,
        );
    } else if form.method == "post" && form.enctype == "application/x-www-form-urlencoded" {
        begin_navigation_request(
            state,
            action.to_string(),
            client,
            sender,
            request_id,
            cancellations,
            None,
            Some(fields),
        );
    } else {
        state.status = format!(
            "Unsupported form method or encoding: {} ({})",
            form.method, form.enctype
        );
        state.dirty = true;
    }
}

fn open_external(url: &str) -> std::io::Result<()> {
    let parsed = HttpClient::parse_url(url)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("rundll32");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    command.arg("url.dll,FileProtocolHandler");
    command.arg(parsed.as_str());
    command.spawn().map(|_| ())
}

fn interactive_targets(page: &TuiPage) -> Vec<InteractiveTarget> {
    let root = &page.root;
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
                        form_control: None,
                        form: None,
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
                form_control: None,
                form: None,
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
                form_control: None,
                form: None,
            }),
            _ => {}
        }
        for child in &component.children {
            collect(child, out);
        }
    }
    let mut out = Vec::new();
    collect(root, &mut out);
    for target in &mut out {
        for form in &page.forms {
            if let Some(control) = form
                .controls
                .iter()
                .find(|control| control.interactive_id == target.id)
            {
                if !control.label.is_empty() {
                    target.label = control.label.clone();
                }
                target.form_control = Some(control.clone());
                target.form = Some(form.clone());
                target.disabled = control.disabled;
                break;
            }
        }
    }
    out.retain(|target| {
        target
            .form_control
            .as_ref()
            .is_none_or(|control| control.control_type != "hidden")
    });
    out
}

fn default_form_values(forms: &[FormDefinition]) -> FormValues {
    let mut values = HashMap::new();
    for field in forms.iter().flat_map(|form| &form.controls) {
        let value = match field.control_type.as_str() {
            "password" => String::new(),
            "checkbox" | "radio" => field.checked.to_string(),
            _ => field.value.clone(),
        };
        values.insert(field.interactive_id.clone(), Zeroizing::new(value));
    }
    values
}

fn render_form_line(line: &str, targets: &[InteractiveTarget], tab: &BrowserTab) -> String {
    for target in targets {
        let Some(field) = &target.form_control else {
            continue;
        };
        if !line.contains(&format!("[{}]", target.id)) {
            continue;
        }
        let prefix = line.split("[ ]").next().unwrap_or("");
        let value = tab
            .form_values
            .get(&field.interactive_id)
            .map(|value| value.as_str())
            .unwrap_or(&field.value);
        let display = match field.control_type.as_str() {
            "password" => "•".repeat(value.chars().count()),
            "checkbox" => {
                if matches!(value, "true" | "on" | "1") {
                    "x".into()
                } else {
                    " ".into()
                }
            }
            "radio" => {
                if matches!(value, "true" | "on" | "1") {
                    "●".into()
                } else {
                    "○".into()
                }
            }
            "select" => field
                .options
                .iter()
                .find(|(option, _)| option == value)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| value.into()),
            "submit" | "image" => {
                return format!(
                    "{prefix}[ {} ] [{}]",
                    if target.label.is_empty() {
                        "Submit"
                    } else {
                        &target.label
                    },
                    target.id
                );
            }
            "reset" | "button" => return format!("{prefix}[ {} ] [{}]", target.label, target.id),
            _ => value.into(),
        };
        let label = if target.label.is_empty() {
            field.name.as_deref().unwrap_or("Input")
        } else {
            &target.label
        };
        return match field.control_type.as_str() {
            "checkbox" | "radio" => format!("{prefix}[{display}] {label} [{}]", target.id),
            _ => format!("{prefix}{label}: {display} [{}]", target.id),
        };
    }
    line.to_owned()
}

fn validate_form(form: &FormDefinition, values: &FormValues) -> Option<String> {
    if form.requires_javascript {
        return Some("This form requires JavaScript, which WebMorph does not execute".into());
    }
    if !matches!(form.method.as_str(), "get" | "post") {
        return Some(format!("Form method '{}' is not supported", form.method));
    }
    if form.method == "post" && form.enctype != "application/x-www-form-urlencoded" {
        return Some(format!("Form encoding '{}' is not supported", form.enctype));
    }
    if form.method == "get"
        && form
            .controls
            .iter()
            .any(|field| field.control_type == "password")
    {
        return Some("Password forms cannot use GET; use a POST form instead".into());
    }
    for field in form.controls.iter().filter(|field| !field.disabled) {
        let value = values
            .get(&field.interactive_id)
            .map(|value| value.as_str())
            .unwrap_or(&field.value);
        let checked = matches!(value, "true" | "on" | "1");
        if field.control_type == "file" {
            return Some(format!("{}: file uploads are not supported", field.label));
        }
        if field.control_type == "select" && field.multiple {
            return Some(format!(
                "{}: multi-select controls are not supported",
                field.label
            ));
        }
        if field.required {
            let valid = match field.control_type.as_str() {
                "checkbox" => checked,
                "radio" => form
                    .controls
                    .iter()
                    .filter(|candidate| {
                        candidate.control_type == "radio" && candidate.name == field.name
                    })
                    .any(|candidate| {
                        values
                            .get(&candidate.interactive_id)
                            .map(|value| value.as_str())
                            .unwrap_or(if candidate.checked { "true" } else { "false" })
                            == "true"
                    }),
                _ => !value.trim().is_empty(),
            };
            if !valid {
                return Some(format!("{} is required", field.label));
            }
        }
        if !value.is_empty() {
            if field
                .minlength
                .is_some_and(|min| value.chars().count() < min)
            {
                return Some(format!(
                    "{} must contain at least {} characters",
                    field.label,
                    field.minlength.unwrap()
                ));
            }
            if field
                .maxlength
                .is_some_and(|max| value.chars().count() > max)
            {
                return Some(format!(
                    "{} must contain at most {} characters",
                    field.label,
                    field.maxlength.unwrap()
                ));
            }
            if field.control_type == "email" && (!value.contains('@') || value.ends_with('@')) {
                return Some(format!("{} must be a valid email address", field.label));
            }
            if field.control_type == "number" && value.parse::<f64>().is_err() {
                return Some(format!("{} must be a number", field.label));
            }
        }
    }
    None
}

fn submitted_fields(
    form: &FormDefinition,
    values: &FormValues,
    submitter: Option<&str>,
) -> Vec<(String, String)> {
    form.controls
        .iter()
        .filter_map(|field| {
            let name = field.name.as_ref().filter(|name| !name.is_empty())?;
            if field.disabled {
                return None;
            }
            match field.control_type.as_str() {
                "submit" | "image" => {
                    if submitter == Some(field.interactive_id.as_str()) {
                        Some((
                            name.clone(),
                            values
                                .get(&field.interactive_id)
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| field.value.clone()),
                        ))
                    } else {
                        None
                    }
                }
                "button" | "reset" => None,
                "checkbox" | "radio" => {
                    let value = values
                        .get(&field.interactive_id)
                        .map(|value| value.as_str())
                        .unwrap_or(if field.checked { "true" } else { "false" });
                    if matches!(value, "true" | "on" | "1") {
                        Some((
                            name.clone(),
                            if field.value.is_empty() {
                                "on".into()
                            } else {
                                field.value.clone()
                            },
                        ))
                    } else {
                        None
                    }
                }
                _ => Some((
                    name.clone(),
                    values
                        .get(&field.interactive_id)
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| field.value.clone()),
                )),
            }
        })
        .collect()
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
    let safe_text = strip_terminal_controls(text);
    let chars: Vec<char> = safe_text.chars().collect();
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
    use super::{
        CancellationToken, Focus, HashMap, HttpClient, Theme, UiAction, UiState, Zeroizing, mpsc,
        visible_window,
    };
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
    fn visible_window_limits_scrolling_work_to_the_viewport() {
        let rows: Vec<_> = (0..10_000).collect();
        assert_eq!(visible_window(&rows, 0, 3), [0, 1, 2]);
        assert_eq!(visible_window(&rows, 5_000, 2), [5_000, 5_001]);
        assert_eq!(visible_window(&rows, 20_000, 3), []);
    }

    fn form_state(source: &str) -> UiState {
        UiState::new(transform(&html::parse(
            "https://example.com/search/index",
            source,
        )))
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
    fn ctrl_tab_switches_tabs_and_preserves_each_tabs_selection() {
        let mut ui = state();
        ui.append_new_tab(String::new());
        assert_eq!(ui.tabs.len(), 2);
        ui.focus = Focus::Content;
        ui.selected_interactive = 1;
        assert_eq!(
            ui.handle_key(KeyEvent::new(
                KeyCode::Tab,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )),
            UiAction::PreviousTab
        );
        ui.select_tab(-1);
        assert_eq!(ui.active_tab, 0);
        assert_eq!(ui.focus, Focus::Tabs);
        assert_eq!(ui.selected_interactive, 0);
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            UiAction::NextTab
        );
        ui.select_tab(1);
        assert_eq!(ui.active_tab, 1);
        assert_eq!(ui.selected_interactive, 1);
    }

    #[test]
    fn shift_enter_opens_selected_link_in_new_tab() {
        let mut ui = state();
        ui.focus = Focus::Content;
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            UiAction::NavigateNewTab("https://example.com/home".into())
        );
    }

    #[test]
    fn alt_enter_is_an_explicit_external_open_action() {
        let mut ui = state();
        ui.focus = Focus::Content;
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)),
            UiAction::OpenExternal("https://example.com/home".into())
        );
    }

    #[test]
    fn address_normalization_adds_https_and_rejects_unsafe_schemes() {
        assert_eq!(
            super::normalize_address("example.org/path").unwrap(),
            "https://example.org/path"
        );
        assert!(super::normalize_address("javascript:alert(1)").is_err());
    }

    #[tokio::test]
    async fn invalid_url_supersedes_pending_navigation_without_crashing() {
        let mut ui = state();
        let (sender, _receiver) = mpsc::unbounded_channel();
        let mut cancellations = HashMap::new();
        let mut request_id = 0;
        let old = CancellationToken::new();
        ui.tab_mut().loading = true;
        ui.tab_mut().url = "https://example.com/old".into();
        cancellations.insert(ui.tab().id, old.clone());
        super::begin_navigation(
            &mut ui,
            "file:///etc/passwd".into(),
            HttpClient::default(),
            &sender,
            &mut request_id,
            &mut cancellations,
        );
        assert!(old.is_cancelled());
        assert!(!ui.tab().loading);
        assert!(
            ui.tab()
                .error
                .as_deref()
                .unwrap()
                .contains("only http and https")
        );
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

    #[test]
    fn form_editing_accepts_utf8_and_password_is_masked() {
        let mut ui = form_state(
            r#"<form action="/login" method="post"><label>User<input id="user" name="user" required></label><label>Password<input id="pass" name="pass" type="password"></label><button>Sign in</button></form>"#,
        );
        ui.selected_interactive = 0;
        ui.activate(false);
        for ch in ['Z', 'o', 'ë'] {
            ui.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            ui.tab()
                .form_values
                .get("wm-0001")
                .map(|value| value.as_str()),
            Some("Zoë")
        );
        ui.selected_interactive = 1;
        ui.activate(false);
        for ch in "secret".chars() {
            ui.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        let targets = super::interactive_targets(&ui.tab().page);
        let rendered = super::render_form_line("[ ] pass [wm-0002]", &targets, ui.tab());
        assert!(rendered.contains("••••••"));
        assert!(!rendered.contains("secret"));
        assert!(ui.tab().page.forms[0].controls[1].value.is_empty());
    }

    #[test]
    fn required_validation_and_post_confirmation_are_explicit() {
        let mut ui = form_state(
            r#"<form action="/login" method="post"><input id="user" name="user" required><input id="pass" name="pass" type="password" required><button id="submit" type="submit">Login</button></form>"#,
        );
        ui.selected_interactive = 2;
        assert_eq!(ui.activate(false), UiAction::None);
        assert!(ui.status.contains("user is required"));
        ui.tab_mut()
            .form_values
            .insert("wm-0001".into(), Zeroizing::new("person".into()));
        ui.tab_mut()
            .form_values
            .insert("wm-0002".into(), Zeroizing::new("secret".into()));
        assert_eq!(ui.activate(false), UiAction::None);
        assert!(matches!(
            ui.dialog,
            Some(super::Dialog::ConfirmSubmit { .. })
        ));
        assert_eq!(
            ui.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            UiAction::None
        );
        assert!(ui.dialog.is_none());
    }

    #[test]
    fn checkbox_and_select_are_controls_with_successful_values() {
        let mut ui = form_state(
            r#"<form action="/search"><input id="q" name="q" value="birds"><input id="news" type="checkbox" name="news" value="yes"><select id="sort" name="sort"><option value="new">Newest</option><option value="top">Top</option></select><button id="go" type="submit" name="go" value="1">Search</button></form>"#,
        );
        ui.selected_interactive = 1;
        ui.activate(false);
        ui.selected_interactive = 2;
        ui.activate(false);
        let form = &ui.tab().page.forms[0];
        let values = super::submitted_fields(form, &ui.tab().form_values, Some("wm-0004"));
        assert!(values.contains(&("news".into(), "yes".into())));
        assert!(values.contains(&("sort".into(), "top".into())));
        assert!(values.contains(&("go".into(), "1".into())));
    }

    #[test]
    fn textarea_accepts_multiline_input_and_html_validation_is_reported() {
        let mut ui = form_state(
            r#"<form action="/comment"><textarea id="body" name="body" required minlength="4">Hi</textarea><input id="email" name="email" type="email"><button type="submit">Send</button></form>"#,
        );
        ui.selected_interactive = 0;
        ui.activate(false);
        ui.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(
            ui.tab()
                .form_values
                .get("wm-0001")
                .map(|value| value.as_str()),
            Some("Hi\n")
        );
        let form = ui.tab().page.forms[0].clone();
        assert!(
            super::validate_form(&form, &ui.tab().form_values)
                .unwrap()
                .contains("at least 4")
        );
        ui.tab_mut()
            .form_values
            .insert("wm-0001".into(), Zeroizing::new("hello".into()));
        ui.tab_mut()
            .form_values
            .insert("wm-0002".into(), Zeroizing::new("not-an-email".into()));
        assert!(
            super::validate_form(&form, &ui.tab().form_values)
                .unwrap()
                .contains("valid email")
        );
    }

    #[test]
    fn mouse_selection_places_focus_in_a_text_field() {
        let mut ui = form_state(
            r#"<form action="/search"><input id="query" name="q" placeholder="Search"><button type="submit">Go</button></form>"#,
        );
        let action = ui.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 12,
                row: 8,
                modifiers: KeyModifiers::NONE,
            },
            ratatui::layout::Rect::new(0, 0, 80, 24),
        );
        assert_eq!(action, UiAction::None);
        assert_eq!(ui.editing_field.as_deref(), Some("wm-0001"));
    }

    #[test]
    fn javascript_form_handlers_are_reported_as_unsupported() {
        let ui = form_state(
            r#"<form action="/submit" onsubmit="return false"><input name="q"><button type="submit">Go</button></form>"#,
        );
        let form = &ui.tab().page.forms[0];
        assert!(form.requires_javascript);
        assert!(
            super::validate_form(form, &ui.tab().form_values)
                .unwrap()
                .contains("requires JavaScript")
        );
    }
}
