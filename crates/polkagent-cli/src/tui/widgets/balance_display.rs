//! Multi-asset balance display widget.
//!
//! Renders a structured panel showing on-chain account balances with:
//! - Free, reserved, and frozen amounts
//! - Existential deposit indicator
//! - Denomination formatting for DOT, KSM, and WND networks

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// Denomination
// ---------------------------------------------------------------------------

/// Known network denomination metadata.
// Public widget API — additional variants for Kusama, Westend, and custom networks.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denomination {
    /// Polkadot (10 decimals, symbol "DOT").
    Dot,
    /// Kusama (12 decimals, symbol "KSM").
    Ksm,
    /// Westend testnet (12 decimals, symbol "WND").
    Wnd,
    /// Custom denomination with explicit decimals and symbol.
    Custom {
        decimals: u8,
        // Symbol stored externally due to lifetime constraints.
    },
}

impl Denomination {
    /// Number of decimal places for this denomination.
    pub fn decimals(self) -> u8 {
        match self {
            Self::Dot => 10,
            Self::Ksm | Self::Wnd => 12,
            Self::Custom { decimals, .. } => decimals,
        }
    }

    /// The ticker symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Dot => "DOT",
            Self::Ksm => "KSM",
            Self::Wnd => "WND",
            Self::Custom { .. } => "UNIT",
        }
    }
}

/// Format a raw balance (in plancks) into a human-readable string with the
/// appropriate number of decimal places and the ticker symbol.
///
/// For readability we display at most 4 fractional digits unless the value
/// would round to zero, in which case we show enough to expose the first
/// non-zero digit.
pub fn format_balance(plancks: u128, denom: Denomination) -> String {
    let decimals = denom.decimals() as u32;
    let divisor = 10u128.pow(decimals);
    let whole = plancks / divisor;
    let frac = plancks % divisor;

    // Show 4 significant fractional digits.
    let display_decimals = 4u32.min(decimals);
    let shift = decimals.saturating_sub(display_decimals);
    let frac_shifted = frac / 10u128.pow(shift);

    let symbol = denom.symbol();
    if frac_shifted == 0 && frac > 0 {
        // Very small amount — show full precision.
        format!("{whole}.{frac:0>width$} {symbol}", width = decimals as usize)
    } else {
        format!(
            "{whole}.{frac_shifted:0>width$} {symbol}",
            width = display_decimals as usize
        )
    }
}

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// A single asset balance entry.
pub struct AssetBalance {
    /// Human-readable asset label (e.g. "DOT", "USDT").
    pub label: String,
    /// Free (transferable) balance in plancks.
    pub free: u128,
    /// Reserved (locked by the runtime) balance in plancks.
    pub reserved: u128,
    /// Frozen (e.g. staking, vesting) balance in plancks.
    pub frozen: u128,
    /// Denomination for formatting.
    pub denomination: Denomination,
}

/// Input data for the balance display widget.
pub struct BalanceDisplayData<'a> {
    /// One or more asset balances to render.
    pub assets: &'a [AssetBalance],
    /// The existential deposit for the primary asset (plancks).
    pub existential_deposit: u128,
    /// Denomination of the existential deposit.
    pub ed_denomination: Denomination,
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render the balance display panel into `area`.
///
/// Each asset gets its own row group with free / reserved / frozen amounts.
/// The existential deposit is shown at the bottom.
pub fn render(frame: &mut Frame, area: Rect, data: &BalanceDisplayData<'_>, theme: &Theme) {
    if area.width < 10 || area.height < 4 {
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .title(Span::styled(
            " Balances ",
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.bg_raised));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'_>> = Vec::new();

    for asset in data.assets {
        let denom = asset.denomination;

        // Asset header.
        lines.push(Line::from(Span::styled(
            format!("  {}", asset.label),
            Style::default()
                .fg(theme.bone)
                .add_modifier(Modifier::BOLD),
        )));

        // Free balance.
        let free_total = asset.free;
        let is_low = free_total > 0
            && free_total <= data.existential_deposit * 2
            && denom == data.ed_denomination;
        let free_color = if is_low { theme.warning } else { theme.success };
        lines.push(Line::from(vec![
            Span::styled("    Free:     ", Style::default().fg(theme.text_dim)),
            Span::styled(
                format_balance(asset.free, denom),
                Style::default().fg(free_color),
            ),
        ]));

        // Reserved balance.
        lines.push(Line::from(vec![
            Span::styled("    Reserved: ", Style::default().fg(theme.text_dim)),
            Span::styled(
                format_balance(asset.reserved, denom),
                Style::default().fg(theme.text_primary),
            ),
        ]));

        // Frozen balance.
        lines.push(Line::from(vec![
            Span::styled("    Frozen:   ", Style::default().fg(theme.text_dim)),
            Span::styled(
                format_balance(asset.frozen, denom),
                Style::default().fg(theme.text_primary),
            ),
        ]));

        // Blank line between assets.
        lines.push(Line::raw(""));
    }

    // Existential deposit indicator.
    lines.push(Line::from(vec![
        Span::styled("  ED: ", Style::default().fg(theme.text_dim)),
        Span::styled(
            format_balance(data.existential_deposit, data.ed_denomination),
            Style::default().fg(theme.text_dim),
        ),
        Span::styled(
            " (min balance)",
            Style::default().fg(theme.text_ghost),
        ),
    ]));

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}
