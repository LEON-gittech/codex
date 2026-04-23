use crate::color::blend;
use crate::color::is_light;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use ratatui::style::Color;
use ratatui::style::Style;

const USER_MESSAGE_FALLBACK_BG: Color = Color::DarkGray;
const USER_MESSAGE_DARK_ALPHA: f32 = 0.20;

pub fn user_message_style() -> Style {
    user_message_style_for(default_bg())
}

pub fn proposed_plan_style() -> Style {
    proposed_plan_style_for(default_bg())
}

/// Returns the style for a user-authored message using the provided terminal background.
pub fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(user_message_bg(bg)),
        None => Style::default().bg(user_message_fallback_bg()),
    }
}

pub fn proposed_plan_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(proposed_plan_bg(bg)),
        None => Style::default().bg(user_message_fallback_bg()),
    }
}

#[allow(clippy::disallowed_methods)]
pub fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
    let (top, alpha) = if is_light(terminal_bg) {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), USER_MESSAGE_DARK_ALPHA)
    };
    best_color(blend(top, terminal_bg, alpha))
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
    user_message_bg(terminal_bg)
}

pub fn user_message_fallback_bg() -> Color {
    USER_MESSAGE_FALLBACK_BG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_terminal_bg_uses_configured_stronger_blend() {
        let expected = best_color(blend((255, 255, 255), (0, 0, 0), USER_MESSAGE_DARK_ALPHA));
        assert_eq!(user_message_bg((0, 0, 0)), expected);
    }

    #[test]
    fn missing_terminal_bg_uses_explicit_fallback_panel() {
        assert_eq!(
            user_message_style_for(None).bg,
            Some(USER_MESSAGE_FALLBACK_BG)
        );
        assert_eq!(
            proposed_plan_style_for(None).bg,
            Some(USER_MESSAGE_FALLBACK_BG)
        );
    }
}
