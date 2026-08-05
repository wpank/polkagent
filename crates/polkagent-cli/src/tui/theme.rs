//! ROSEDUST design system — full colour palette and style helpers.
//!
//! Many palette tokens and helper methods are defined for future views that are
//! not yet implemented. The `dead_code` lint is suppressed for this module.
//!
//! The palette is derived from Roko / Bardo with Polkadot-specific extensions.
//! See PRD-13 Appendix B for the definitive colour specification.
//!
//! # Ground rules
//!
//! - **No pure black** (`#000000`). Every background has a faint chromatic tint.
//! - **No pure white** (`#ffffff`). Maximum contrast is `bone` (`#C8B890`).
//! - Rose tones are dominant (primary accent). DOT-pink is reserved for
//!   chain-identity elements only.
//! - All colours are disabled when `NO_COLOR` is set in the environment.

#![allow(dead_code)]

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Complete ROSEDUST palette for the Polkagent TUI.
///
/// Construct via [`Theme::dark()`], [`Theme::no_color()`], or
/// [`Theme::from_env()`].  All rendering code receives a `&Theme` reference
/// and should never hard-code colour literals.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // ── Background hierarchy ──────────────────────────────────────────────
    /// Deepest background — the void. `Rgb(6, 6, 8)` / `#060608`.
    pub bg_void: Color,
    /// Card and panel backgrounds. `Rgb(12, 10, 14)` / `#0C0A0E`.
    pub bg_raised: Color,
    /// Elevated surfaces with violet undertone. `Rgb(8, 8, 16)` / `#080810`.
    pub bg_mid: Color,
    /// Warm-tinted backgrounds (agent activity). `Rgb(10, 8, 8)` / `#0A0808`.
    pub bg_warm: Color,
    /// Selection highlight. `Rgb(34, 28, 36)` / `#221C24`.
    pub bg_highlight: Color,

    // ── Rose family (primary accent) ─────────────────────────────────────
    /// Deepest rose, for subtle backgrounds. `Rgb(58, 32, 48)` / `#3A2030`.
    pub rose_deep: Color,
    /// Very dim rose, structural elements. `Rgb(72, 40, 56)` / `#482838`.
    pub rose_ember: Color,
    /// Muted, inactive, borders. `Rgb(122, 80, 96)` / `#7A5060`.
    pub rose_dim: Color,
    /// Standard accent, active elements. `Rgb(170, 112, 136)` / `#AA7088`.
    pub rose: Color,
    /// Active, highlighted, selection. `Rgb(204, 144, 168)` / `#CC90A8`.
    pub rose_bright: Color,

    // ── Secondary accent ─────────────────────────────────────────────────
    /// Highlight text, headers. `Rgb(200, 184, 144)` / `#C8B890`.
    pub bone: Color,
    /// Muted bone for supporting elements. `Rgb(138, 122, 90)` / `#8A7A5A`.
    pub bone_dim: Color,

    // ── Semantic colours ─────────────────────────────────────────────────
    /// Success / finalized / healthy (jade). `Rgb(112, 136, 122)` / `#70887A`.
    pub success: Color,
    /// Warning / approaching threshold (amber). `Rgb(170, 136, 85)` / `#AA8855`.
    pub warning: Color,
    /// Error / failed / denied (crimson). `Rgb(170, 80, 96)` / `#AA5060`.
    pub danger: Color,
    /// Knowledge / memory / info (violet). `Rgb(88, 88, 120)` / `#585878`.
    pub dream: Color,

    // ── Text hierarchy ────────────────────────────────────────────────────
    /// Invisible, structural only. `Rgb(32, 24, 32)` / `#201820`.
    pub text_phantom: Color,
    /// Tertiary, decorative, timestamps. `Rgb(48, 40, 48)` / `#302830`.
    pub text_ghost: Color,
    /// Secondary, supporting text. `Rgb(88, 72, 88)` / `#584858`.
    pub text_dim: Color,
    /// Primary body text. `Rgb(152, 128, 144)` / `#988090`.
    pub text_primary: Color,

    // ── Borders ───────────────────────────────────────────────────────────
    /// Unfocused panel border. `Rgb(24, 20, 32)` / `#181420`.
    pub border: Color,
    /// Focused panel border (= rose). `Rgb(170, 112, 136)` / `#AA7088`.
    pub border_active: Color,
    /// Knowledge panel border (= dream). `Rgb(88, 88, 120)` / `#585878`.
    pub border_dream: Color,

    // ── Polkadot extensions ──────────────────────────────────────────────
    /// Polkadot brand pink — used only for chain-identity elements.
    /// `Rgb(230, 0, 122)` / `#E6007A`. Use sparingly.
    pub dot_pink: Color,
    /// Parachain / XCM indicator. `Rgb(100, 80, 180)` / `#6450B4`.
    pub parachain_violet: Color,
    /// Finality confirmed indicator. `Rgb(80, 180, 160)` / `#50B4A0`.
    pub finalized_teal: Color,
    /// Pending / unconfirmed state. `Rgb(200, 150, 60)` / `#C8963C`.
    pub pending_amber: Color,

    // ── CRT atmosphere ────────────────────────────────────────────────────
    /// CRT scanline row background. `Rgb(5, 5, 7)` / `#050507`.
    pub scanline_dark: Color,
    /// Phosphor residue (warm). `Rgb(26, 16, 24)` / `#1A1018`.
    pub phosphor_res: Color,
    /// Warm noise cell colour. `Rgb(42, 24, 32)` / `#2A1820`.
    pub noise_warm: Color,
    /// Cool noise cell colour. `Rgb(32, 24, 40)` / `#201828`.
    pub noise_cool: Color,
}

impl Theme {
    /// Full ROSEDUST dark palette (default).
    #[must_use]
    pub fn dark() -> Self {
        Self {
            bg_void: Color::Rgb(6, 6, 8),
            bg_raised: Color::Rgb(12, 10, 14),
            bg_mid: Color::Rgb(8, 8, 16),
            bg_warm: Color::Rgb(10, 8, 8),
            bg_highlight: Color::Rgb(34, 28, 36),

            rose_deep: Color::Rgb(58, 32, 48),
            rose_ember: Color::Rgb(72, 40, 56),
            rose_dim: Color::Rgb(122, 80, 96),
            rose: Color::Rgb(170, 112, 136),
            rose_bright: Color::Rgb(204, 144, 168),

            bone: Color::Rgb(200, 184, 144),
            bone_dim: Color::Rgb(138, 122, 90),

            success: Color::Rgb(112, 136, 122),
            warning: Color::Rgb(170, 136, 85),
            danger: Color::Rgb(170, 80, 96),
            dream: Color::Rgb(88, 88, 120),

            text_phantom: Color::Rgb(32, 24, 32),
            text_ghost: Color::Rgb(48, 40, 48),
            text_dim: Color::Rgb(88, 72, 88),
            text_primary: Color::Rgb(152, 128, 144),

            border: Color::Rgb(24, 20, 32),
            border_active: Color::Rgb(170, 112, 136),
            border_dream: Color::Rgb(88, 88, 120),

            dot_pink: Color::Rgb(230, 0, 122),
            parachain_violet: Color::Rgb(100, 80, 180),
            finalized_teal: Color::Rgb(80, 180, 160),
            pending_amber: Color::Rgb(200, 150, 60),

            scanline_dark: Color::Rgb(5, 5, 7),
            phosphor_res: Color::Rgb(26, 16, 24),
            noise_warm: Color::Rgb(42, 24, 32),
            noise_cool: Color::Rgb(32, 24, 40),
        }
    }

    /// All-reset palette for `NO_COLOR` compliance.
    ///
    /// Every colour field is set to `Color::Reset` so the terminal renders
    /// with its own defaults. All style helpers return unstyled styles.
    #[must_use]
    pub fn no_color() -> Self {
        let r = Color::Reset;
        Self {
            bg_void: r,
            bg_raised: r,
            bg_mid: r,
            bg_warm: r,
            bg_highlight: r,
            rose_deep: r,
            rose_ember: r,
            rose_dim: r,
            rose: r,
            rose_bright: r,
            bone: r,
            bone_dim: r,
            success: r,
            warning: r,
            danger: r,
            dream: r,
            text_phantom: r,
            text_ghost: r,
            text_dim: r,
            text_primary: r,
            border: r,
            border_active: r,
            border_dream: r,
            dot_pink: r,
            parachain_violet: r,
            finalized_teal: r,
            pending_amber: r,
            scanline_dark: r,
            phosphor_res: r,
            noise_warm: r,
            noise_cool: r,
        }
    }

    /// Select the theme from the environment.
    ///
    /// Returns [`Theme::no_color()`] when the `NO_COLOR` environment variable
    /// is set (any value), otherwise returns [`Theme::dark()`].
    #[must_use]
    pub fn from_env() -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            Self::no_color()
        } else {
            Self::dark()
        }
    }

    // ── Style helpers ─────────────────────────────────────────────────────

    /// Border style for a focused panel.
    #[must_use]
    pub fn focused_border(&self) -> Style {
        Style::default().fg(self.border_active)
    }

    /// Border style for an unfocused panel.
    #[must_use]
    pub fn unfocused_border(&self) -> Style {
        Style::default().fg(self.border)
    }

    /// Tab label style for the active tab.
    #[must_use]
    pub fn active_tab_style(&self) -> Style {
        Style::default()
            .fg(self.rose_bright)
            .add_modifier(Modifier::BOLD)
    }

    /// Tab label style for inactive tabs.
    #[must_use]
    pub fn inactive_tab_style(&self) -> Style {
        Style::default().fg(self.text_dim)
    }

    /// Style for actively running items.
    #[must_use]
    pub fn status_active(&self) -> Style {
        Style::default()
            .fg(self.success)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for error state items.
    #[must_use]
    pub fn status_error(&self) -> Style {
        Style::default()
            .fg(self.danger)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for warning state items.
    #[must_use]
    pub fn status_warning(&self) -> Style {
        Style::default()
            .fg(self.warning)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for unknown / indeterminate state.
    #[must_use]
    pub fn status_unknown(&self) -> Style {
        Style::default().fg(self.warning)
    }

    /// Map a canonical lifecycle state string to its ROSEDUST colour.
    #[must_use]
    pub fn status_color(&self, state: &str) -> Color {
        match state {
            "finalized" | "succeeded" | "active" | "completed" => self.success,
            "working" | "signed" | "submitted" | "included" | "started" => self.rose,
            "waiting_approval" | "pending" | "queued" | "created" | "unknown" => self.warning,
            "failed" | "reverted" | "denied" | "expired" | "timed_out" => self.danger,
            "cancelled" | "stopped" | "idle" | "paused" | "deactivated" | "configured" => {
                self.text_dim
            }
            _ => self.text_primary,
        }
    }

    /// Map a progress ratio `0.0`..=`1.0` to a semantic colour.
    ///
    /// - `>= 0.75` → success (jade)
    /// - `>= 0.4`  → warning (amber)
    /// - `< 0.4`   → danger (crimson)
    #[must_use]
    pub fn progress_color(&self, ratio: f64) -> Color {
        if ratio >= 0.75 {
            self.success
        } else if ratio >= 0.4 {
            self.warning
        } else {
            self.danger
        }
    }

    /// Title style (bone, bold) used for section headers.
    #[must_use]
    pub fn title_style(&self) -> Style {
        Style::default().fg(self.bone).add_modifier(Modifier::BOLD)
    }

    /// Dimmed text style for timestamps and secondary labels.
    #[must_use]
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.text_dim)
    }

    /// Primary text style for body content.
    #[must_use]
    pub fn primary_style(&self) -> Style {
        Style::default().fg(self.text_primary)
    }

    /// Rose-coloured style for primary accents and active glyphs.
    #[must_use]
    pub fn rose_style(&self) -> Style {
        Style::default().fg(self.rose)
    }

    /// Bright rose style for selected items and highlights.
    #[must_use]
    pub fn rose_bright_style(&self) -> Style {
        Style::default()
            .fg(self.rose_bright)
            .add_modifier(Modifier::BOLD)
    }
}
