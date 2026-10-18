use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{error::Result, transform::TuiPage};

pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let mut stdout = io::stdout();
                let _ = execute!(stdout, LeaveAlternateScreen);
                Err(error.into())
            }
        }
    }

    pub async fn run(&mut self, page: &TuiPage) -> Result<()> {
        let mut offset = 0usize;
        let mut ticks = tokio::time::interval(Duration::from_millis(50));
        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown);
        loop {
            self.draw(page, offset)?;
            tokio::select! {
                signal = &mut shutdown => {
                    signal?;
                    break;
                }
                _ = ticks.tick() => {
                    if event::poll(Duration::ZERO)? {
                        if let Event::Key(key) = event::read()? {
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                                KeyCode::Char('q' | 'Q') | KeyCode::Esc => break,
                                KeyCode::Down | KeyCode::Char('j') => offset = offset.saturating_add(1),
                                KeyCode::Up | KeyCode::Char('k') => offset = offset.saturating_sub(1),
                                KeyCode::Char('g') => offset = 0,
                                KeyCode::Char('G') => offset = page.lines.len(),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn draw(&mut self, page: &TuiPage, offset: usize) -> Result<()> {
        self.terminal.draw(|frame| {
            let areas =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());
            let lines = page
                .lines
                .iter()
                .map(|line| Line::raw(line.as_str()))
                .collect::<Vec<_>>();
            let content = Paragraph::new(Text::from(lines))
                .block(
                    Block::default()
                        .title(page.title.as_str())
                        .borders(Borders::ALL),
                )
                .wrap(Wrap { trim: true })
                .scroll((offset.min(u16::MAX as usize) as u16, 0));
            frame.render_widget(content, areas[0]);
            let help = Paragraph::new("↑/↓ or j/k: scroll   g/G: top/bottom   q/Esc: quit")
                .style(Style::default().add_modifier(Modifier::DIM));
            frame.render_widget(help, areas[1]);
        })?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}
