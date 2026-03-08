use console::style;
use crossterm::{
    cursor, execute,
    terminal::{self, ClearType},
};
use std::io::{self, Write};
use std::sync::{Arc, RwLock};

const STATUS_BAR_HEIGHT: u16 = 2;

/// State shared between the session and the status bar renderer.
#[derive(Clone, Debug)]
pub struct StatusBarState {
    pub model: String,
    pub provider: String,
    pub project: String,
    pub branch: String,
    pub mode: String,
    pub extensions_count: usize,
    pub total_tokens: usize,
    pub context_limit: usize,
    pub cost: Option<f64>,
    pub processing: bool,
}

impl Default for StatusBarState {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: String::new(),
            project: String::new(),
            branch: String::new(),
            mode: "chat".to_string(),
            extensions_count: 0,
            total_tokens: 0,
            context_limit: 0,
            cost: None,
            processing: false,
        }
    }
}

pub struct StatusBar {
    state: Arc<RwLock<StatusBarState>>,
    active: bool,
}

impl StatusBar {
    pub fn new(state: StatusBarState) -> Self {
        Self {
            state: Arc::new(RwLock::new(state)),
            active: false,
        }
    }

    /// Set the scroll region to leave space for the status bar at the bottom,
    /// then render the initial bar.
    pub fn setup(&mut self) -> io::Result<()> {
        let (_, rows) = terminal::size()?;
        let scroll_end = rows.saturating_sub(STATUS_BAR_HEIGHT + 1);

        let mut stdout = io::stdout();
        // Set scroll region: rows 1..scroll_end (0-indexed internally, 1-indexed in DECSTBM)
        execute!(stdout, terminal::ScrollUp(0))?;
        write!(stdout, "\x1b[1;{}r", scroll_end)?;
        // Move cursor to top-left of scroll region
        execute!(stdout, cursor::MoveTo(0, 0))?;
        stdout.flush()?;

        self.active = true;
        self.render()?;
        Ok(())
    }

    /// Remove the status bar and restore full-screen scrolling.
    pub fn teardown(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let (_, rows) = terminal::size()?;
        let mut stdout = io::stdout();

        // Clear the status bar lines
        execute!(stdout, cursor::MoveTo(0, rows.saturating_sub(STATUS_BAR_HEIGHT)))?;
        execute!(stdout, terminal::Clear(ClearType::FromCursorDown))?;

        // Reset scroll region to full terminal
        write!(stdout, "\x1b[r")?;
        stdout.flush()?;
        Ok(())
    }

    /// Temporarily expand scroll region for readline/cliclack input.
    pub fn pause(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let (_, rows) = terminal::size()?;
        let mut stdout = io::stdout();

        // Clear status bar
        execute!(stdout, cursor::MoveTo(0, rows.saturating_sub(STATUS_BAR_HEIGHT)))?;
        execute!(stdout, terminal::Clear(ClearType::FromCursorDown))?;

        // Reset scroll region to full terminal
        write!(stdout, "\x1b[r")?;
        stdout.flush()?;
        Ok(())
    }

    /// Restore the scroll region and re-render after input is done.
    pub fn resume(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let (_, rows) = terminal::size()?;
        let scroll_end = rows.saturating_sub(STATUS_BAR_HEIGHT + 1);

        let mut stdout = io::stdout();
        write!(stdout, "\x1b[1;{}r", scroll_end)?;
        stdout.flush()?;

        self.render()?;
        Ok(())
    }

    /// Atomically update the state and re-render.
    pub fn update_state<F: FnOnce(&mut StatusBarState)>(&self, f: F) -> io::Result<()> {
        if let Ok(mut state) = self.state.write() {
            f(&mut state);
        }
        if self.active {
            self.render()?;
        }
        Ok(())
    }

    /// Toggle the processing indicator.
    pub fn set_processing(&self, processing: bool) -> io::Result<()> {
        self.update_state(|s| s.processing = processing)
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Render the two-line status bar at the bottom of the terminal.
    fn render(&self) -> io::Result<()> {
        let state = match self.state.read() {
            Ok(s) => s.clone(),
            Err(_) => return Ok(()),
        };

        let (cols, rows) = terminal::size()?;
        let width = cols as usize;
        let bar_row = rows.saturating_sub(STATUS_BAR_HEIGHT);

        let mut stdout = io::stdout();

        // Save cursor position
        execute!(stdout, cursor::SavePosition)?;

        // --- Line 1: model · project · branch │ usage bar ---
        execute!(stdout, cursor::MoveTo(0, bar_row))?;
        execute!(stdout, terminal::Clear(ClearType::CurrentLine))?;

        // CC-style: [Model] project   ━━╌╌╌ 0% | 0s
        let model_str = format!(
            "{}{}{}",
            style("[").dim(),
            style(&state.model).bold(),
            style("]").dim(),
        );
        let usage_bar = format_usage_bar(state.total_tokens, state.context_limit, 20);
        let token_str = format_tokens(state.total_tokens);
        let limit_str = format_tokens(state.context_limit);
        let pct = if state.context_limit > 0 {
            (state.total_tokens as f64 / state.context_limit as f64 * 100.0) as usize
        } else {
            0
        };

        let mut line1_parts: Vec<String> = vec![model_str];
        if !state.project.is_empty() {
            line1_parts.push(format!("{}", style(&state.project).cyan()));
        }
        if !state.branch.is_empty() {
            line1_parts.push(format!("{}", style(&state.branch).dim()));
        }

        let line1_left = line1_parts.join(&format!(" {} ", style("│").dim()));
        let line1_right = format!(
            "{} {}%  {}/{}",
            usage_bar,
            style(pct).dim(),
            style(token_str).dim(),
            style(limit_str).dim(),
        );

        // Pad to fill width
        let left_visible_len = console::measure_text_width(&line1_left);
        let right_visible_len = console::measure_text_width(&line1_right);
        let separator = if left_visible_len + right_visible_len + 3 < width {
            let gap = width.saturating_sub(left_visible_len + right_visible_len + 3);
            format!(" {} {}", style("│").dim(), " ".repeat(gap))
        } else {
            format!(" {} ", style("│").dim())
        };

        write!(stdout, " {}{}{}", line1_left, separator, line1_right)?;

        // --- Line 2: mode · extensions · cost ---
        execute!(stdout, cursor::MoveTo(0, bar_row + 1))?;
        execute!(stdout, terminal::Clear(ClearType::CurrentLine))?;

        let mut line2_parts: Vec<String> = vec![format!("{}", style(&state.mode).dim())];

        if state.extensions_count > 0 {
            line2_parts.push(format!(
                "{} {}",
                style(state.extensions_count).dim(),
                style("extensions").dim()
            ));
        }

        if let Some(cost) = state.cost {
            line2_parts.push(format!("{}", style(format!("${:.2}", cost)).dim()));
        }

        if state.processing {
            line2_parts.push(format!("{}", style("⟳").cyan()));
        }

        let line2 = line2_parts.join(&format!(" {} ", style("·").dim()));
        write!(stdout, " {}", line2)?;

        // Restore cursor position
        execute!(stdout, cursor::RestorePosition)?;
        stdout.flush()?;

        Ok(())
    }
}

/// Format a token count for display: 500 → "500", 1500 → "1.5k", 128000 → "128k"
fn format_tokens(n: usize) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 10_000 {
        let k = n as f64 / 1000.0;
        format!("{:.1}k", k)
    } else {
        format!("{}k", n / 1000)
    }
}

/// Render a color-coded usage bar: green <50%, yellow <85%, red >=85%
fn format_usage_bar(used: usize, limit: usize, bar_width: usize) -> String {
    if limit == 0 {
        return format!("{}", style("╌".repeat(bar_width)).dim());
    }

    let ratio = (used as f64 / limit as f64).min(1.0);
    let filled = (ratio * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    let filled_str = "━".repeat(filled);
    let empty_str = "╌".repeat(empty);

    let colored_filled = if ratio < 0.5 {
        format!("{}", style(&filled_str).green())
    } else if ratio < 0.85 {
        format!("{}", style(&filled_str).yellow())
    } else {
        format!("{}", style(&filled_str).red())
    };

    format!("{}{}", colored_filled, style(empty_str).dim())
}
