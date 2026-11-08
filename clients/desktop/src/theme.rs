//! The home-space design system (the web client's `styles/tokens.css`) for iced: the colour
//! tokens and the style functions built on them. The designs are in Penpot (file "Home Space Client", page "Desktop").

use iced::{
    Background, Border, Color, Font, Shadow, Theme, Vector,
    border::Radius,
    color,
    font::{Family, Stretch, Style, Weight},
    theme::Palette,
    widget::{button, container, progress_bar, text, text_input},
};

/// Controls and menu items.
const RADIUS_SM: f32 = 6.0;
/// In-page boxes and menus.
const RADIUS_MD: f32 = 10.0;
/// Dialogs and floating panels.
const RADIUS_LG: f32 = 16.0;
const DISC_RADIUS: f32 = 28.0;
const RADIUS_BIG_LOGO: f32 = 14.0;
const RADIUS_KBD: f32 = 4.0;
const RADIUS_LOGO: f32 = 8.0;
const RADIUS_BAR: f32 = 3.0;
const SHADOW_SM: Shadow = Shadow {
    color: Color::TRANSPARENT,
    offset: Vector::new(0.0, 1.0),
    blur_radius: 2.0,
};
const SHADOW_LG: Shadow = Shadow {
    color: Color::TRANSPARENT,
    offset: Vector::new(0.0, 12.0),
    blur_radius: 32.0,
};
/// Body text and labels.
pub const TEXT_SIZE: f32 = 14.0;
pub const DISABLED_ALPHA: f32 = 0.4;
pub const FADED_ALPHA: f32 = 0.55;
const CHECKBOX_BORDER: f32 = 1.5;
const ACTIVE_BORDER: f32 = 2.0;
const RADIUS_CHECKBOX: f32 = 4.0;
/// Keeps a box's top corners inside the card's rounded border.
const RADIUS_INNER: f32 = RADIUS_MD - 1.0;
/// How far a disabled filled button fades.
const BUTTON_DISABLED_ALPHA: f32 = 0.55;

/// The system's UI font, by name: the generic sans-serif family resolves to a font that exists
/// nowhere but on some Linux systems, and elsewhere each weight falls back to a font of its own.
#[cfg(target_os = "macos")]
const UI_FONT: &str = "Helvetica Neue";
#[cfg(target_os = "windows")]
const UI_FONT: &str = "Segoe UI";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const UI_FONT: &str = "Noto Sans";

const fn ui_font(weight: Weight) -> Font {
    Font {
        family: Family::Name(UI_FONT),
        weight,
        stretch: Stretch::Normal,
        style: Style::Normal,
    }
}

#[cfg(target_os = "macos")]
const MONO_FONT: &str = "Menlo";
#[cfg(target_os = "windows")]
const MONO_FONT: &str = "Consolas";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const MONO_FONT: &str = "Noto Sans Mono";

/// Paths on disk.
pub const MONO: Font = Font {
    family: Family::Name(MONO_FONT),
    ..ui_font(Weight::Normal)
};
/// Body text.
pub const FONT: Font = ui_font(Weight::Normal);
/// Labels, buttons and sidebar items.
pub const MEDIUM: Font = ui_font(Weight::Medium);
/// Headings.
pub const SEMIBOLD: Font = ui_font(Weight::Semibold);
/// The user's initial and other short emphasis.
pub const BOLD: Font = ui_font(Weight::Bold);

#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub bg: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub sidebar: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub border_strong: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    /// Accent at 12%: selected rows, the active sidebar item.
    pub accent_soft: Color,
    pub danger: Color,
    pub on_danger: Color,
    /// Danger at 12%.
    pub danger_soft: Color,
    pub success: Color,
    pub backdrop: Color,
    pub shadow: Color,
}

const LIGHT: Tokens = Tokens {
    bg: color!(0xf4f5f7),
    surface: color!(0xffffff),
    surface_hover: color!(0xf2f4f7),
    sidebar: color!(0xeceef2),
    text: color!(0x14171c),
    muted: color!(0x5c6573),
    border: color!(0xe2e5ea),
    border_strong: color!(0xcdd2da),
    accent: color!(0x2563eb),
    accent_hover: color!(0x1d4ed8),
    on_accent: color!(0xffffff),
    accent_soft: color!(0x2563eb, 0.12),
    danger: color!(0xc0362c),
    on_danger: color!(0xffffff),
    danger_soft: color!(0xc0362c, 0.12),
    success: color!(0x15803d),
    backdrop: color!(0x080a0e, 0.55),
    shadow: color!(0x101828, 0.08),
};

const DARK: Tokens = Tokens {
    bg: color!(0x0e1014),
    surface: color!(0x171a20),
    surface_hover: color!(0x1e222a),
    sidebar: color!(0x13161b),
    text: color!(0xe7e9ec),
    muted: color!(0x98a1b0),
    border: color!(0x262b34),
    border_strong: color!(0x353b46),
    accent: color!(0x6d9eff),
    accent_hover: color!(0x8ab1ff),
    on_accent: color!(0x0a1220),
    accent_soft: color!(0x6d9eff, 0.12),
    danger: color!(0xff8a80),
    on_danger: color!(0x1a0705),
    danger_soft: color!(0xff8a80, 0.12),
    success: color!(0x5fd39a),
    backdrop: color!(0x080a0e, 0.55),
    shadow: color!(0x000000, 0.45),
};

/// The iced theme of a mode. Built-in widgets, such as scrollables, take their colours from its
/// palette; everything else uses [`tokens`].
pub fn app(dark: bool) -> Theme {
    let t = if dark { DARK } else { LIGHT };
    let name = if dark {
        "Home Space Dark"
    } else {
        "Home Space Light"
    };
    Theme::custom(
        name,
        Palette {
            background: t.surface,
            text: t.text,
            primary: t.accent,
            success: t.success,
            // The design system has no warning colour; iced needs one.
            warning: if dark {
                color!(0xfbbf24)
            } else {
                color!(0xb45309)
            },
            danger: t.danger,
        },
    )
}

/// The tokens of the mode `theme` is in.
pub fn tokens(theme: &Theme) -> Tokens {
    if theme.extended_palette().is_dark {
        DARK
    } else {
        LIGHT
    }
}

fn shadow_sm(t: &Tokens) -> Shadow {
    Shadow {
        color: t.shadow,
        ..SHADOW_SM
    }
}

fn shadow_lg(t: &Tokens) -> Shadow {
    Shadow {
        color: t.shadow,
        ..SHADOW_LG
    }
}

fn outline(color: Color, radius: f32) -> Border {
    Border {
        color,
        width: 1.0,
        radius: Radius::from(radius),
    }
}

/// A button filled with `fill`, which `hover` replaces under the pointer.
fn filled(status: button::Status, fill: Color, hover: Color, label: Color) -> button::Style {
    let (background, text_color) = match status {
        button::Status::Active => (fill, label),
        button::Status::Hovered | button::Status::Pressed => (hover, label),
        button::Status::Disabled => (
            fill.scale_alpha(BUTTON_DISABLED_ALPHA),
            label.scale_alpha(BUTTON_DISABLED_ALPHA),
        ),
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border::default().rounded(RADIUS_SM),
        ..button::Style::default()
    }
}

/// The one main action of a view.
pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    filled(status, t.accent, t.accent_hover, t.on_accent)
}

/// A destructive action that needs no second thought any more, as in a confirmation.
pub fn danger(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    filled(status, t.danger, t.danger, t.on_danger)
}

/// An outlined button, next to a [`primary`] one or on its own.
pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    let hover = t.surface_hover;
    let style = filled(status, t.surface, hover, t.text);
    button::Style {
        border: outline(t.border_strong, RADIUS_SM),
        shadow: shadow_sm(&t),
        ..style
    }
}

/// A grid row: transparent, tinted on hover, accent-tinted when selected.
pub fn row(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let background = match (selected, status) {
            (true, _) => Some(t.accent_soft),
            (false, button::Status::Hovered | button::Status::Pressed) => Some(t.surface_hover),
            (false, _) => None,
        };
        button::Style {
            background: background.map(Background::Color),
            text_color: t.text,
            ..button::Style::default()
        }
    }
}

/// A folder's icon and name in a row, which open it.
pub fn name_link(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => t.accent,
            button::Status::Active | button::Status::Disabled => t.text,
        },
        ..button::Style::default()
    }
}

/// The sticky header above the grid.
pub fn header(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface_hover.into()),
        text_color: Some(t.muted),
        border: Border::default().rounded(Radius::default().top(RADIUS_INNER)),
        ..container::Style::default()
    }
}

/// A column title that sorts: muted, darker under the pointer.
pub fn column_title(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    button::Style {
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => t.text,
            button::Status::Active | button::Status::Disabled => t.muted,
        },
        ..button::Style::default()
    }
}

/// A flat toolbar or menu button that highlights on hover.
pub fn tool(theme: &Theme, status: button::Status) -> button::Style {
    let t = tokens(theme);
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => Some(t.surface_hover),
        button::Status::Active | button::Status::Disabled => None,
    };
    let text_color = match status {
        button::Status::Disabled => t.text.scale_alpha(DISABLED_ALPHA),
        _ => t.text,
    };
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: Border::default().rounded(RADIUS_SM),
        ..button::Style::default()
    }
}

/// Like [`tool`], with danger-coloured text.
pub fn danger_tool(theme: &Theme, status: button::Status) -> button::Style {
    let base = tool(theme, status);
    match status {
        button::Status::Disabled => base,
        _ => button::Style {
            text_color: tokens(theme).danger,
            ..base
        },
    }
}

/// A text field: outlined, with an accent outline while it has the focus.
pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let t = tokens(theme);
    let (background, border) = match status {
        text_input::Status::Active | text_input::Status::Hovered => (t.surface, t.border_strong),
        text_input::Status::Focused { .. } => (t.surface, t.accent),
        text_input::Status::Disabled => (t.surface_hover, t.border),
    };
    text_input::Style {
        background: background.into(),
        border: outline(border, RADIUS_SM),
        icon: t.muted,
        placeholder: t.muted,
        value: t.text,
        selection: t.accent_soft,
    }
}

/// A progress bar.
pub fn progress(theme: &Theme) -> progress_bar::Style {
    let t = tokens(theme);
    progress_bar::Style {
        background: t.surface_hover.into(),
        bar: t.accent.into(),
        border: Border::default().rounded(RADIUS_BAR),
    }
}

/// The window behind everything.
pub fn window(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.bg.into()),
        text_color: Some(t.text),
        ..container::Style::default()
    }
}

/// The dimmed layer behind a dialog.
pub fn backdrop(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).backdrop.into()),
        ..container::Style::default()
    }
}

/// A raised surface: dialogs and the transfer panel.
pub fn panel(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: outline(t.border, RADIUS_LG),
        shadow: shadow_lg(&t),
        ..container::Style::default()
    }
}

/// A popup: context menus and tooltips.
pub fn menu(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: outline(t.border, RADIUS_MD),
        shadow: shadow_lg(&t),
        ..container::Style::default()
    }
}

/// A hairline between two things.
pub fn divider(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).border.into()),
        ..container::Style::default()
    }
}

/// A single-line error message.
pub fn error(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(tokens(theme).danger),
        ..container::Style::default()
    }
}

/// The disc behind a sync badge, so it stays readable over the icon it sits on.
pub fn badge(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).surface.into()),
        border: Border::default().rounded(f32::MAX),
        ..container::Style::default()
    }
}

/// Secondary text, such as where a folder syncs to.
pub fn muted(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(tokens(theme).muted),
        ..container::Style::default()
    }
}

/// Secondary text on its own, outside a [`muted`] container.
pub fn muted_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).muted),
    }
}

/// Text in the main colour, inside something that mutes its text.
pub fn strong_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).text),
    }
}

/// Text saying something went well.
pub fn success_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).success),
    }
}

/// Text saying something went wrong.
pub fn danger_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).danger),
    }
}

/// Text of something that cannot be used yet, such as a file waiting to be uploaded.
pub fn disabled(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(tokens(theme).text.scale_alpha(DISABLED_ALPHA)),
        ..container::Style::default()
    }
}

/// The sidebar's background.
pub fn sidebar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).sidebar.into()),
        ..container::Style::default()
    }
}

/// A place in the sidebar: accent-tinted while it is the one shown.
pub fn nav(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        if active {
            button::Style {
                background: Some(t.accent_soft.into()),
                text_color: t.accent,
                border: Border::default().rounded(RADIUS_SM),
                ..button::Style::default()
            }
        } else {
            tool(theme, status)
        }
    }
}

/// The sync status in the sidebar, a button on a raised box, outlined while its page is shown.
pub fn status_card(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tokens(theme);
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => t.surface_hover,
            button::Status::Active | button::Status::Disabled => t.surface,
        };
        let (color, width) = if active {
            (t.accent, ACTIVE_BORDER)
        } else {
            (t.border, 1.0)
        };
        button::Style {
            background: Some(background.into()),
            text_color: t.text,
            border: Border {
                color,
                width,
                radius: Radius::from(RADIUS_MD),
            },
            ..button::Style::default()
        }
    }
}

/// A breadcrumb that leads somewhere: muted next to the bold current one.
pub fn crumb(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: tokens(theme).muted,
        ..tool(theme, status)
    }
}

/// The separator between breadcrumbs.
pub fn crumb_separator(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).border_strong),
    }
}

/// A box resting on the window: the listing and the pages.
pub fn card(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: outline(t.border, RADIUS_MD),
        shadow: shadow_sm(&t),
        ..container::Style::default()
    }
}

/// The toolbar while several nodes are selected.
pub fn selection_bar(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).accent_soft.into()),
        border: Border::default().rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// Text in the accent colour.
pub fn accent_text(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(tokens(theme).accent),
    }
}

/// A keyboard shortcut drawn as a key.
pub fn kbd(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        text_color: Some(t.muted),
        border: outline(t.border_strong, RADIUS_KBD),
        ..container::Style::default()
    }
}

/// The count of what failed, on a sidebar item.
pub fn danger_count(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.danger_soft.into()),
        text_color: Some(t.danger),
        border: Border::default().rounded(f32::MAX),
        ..container::Style::default()
    }
}

/// The app's mark behind the home icon.
pub fn logo(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).accent.into()),
        border: Border::default().rounded(RADIUS_LOGO),
        ..container::Style::default()
    }
}

/// The app's mark on the sign-in card.
pub fn big_logo(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).accent.into()),
        border: Border::default().rounded(RADIUS_BIG_LOGO),
        ..container::Style::default()
    }
}

/// The disc with the user's initial.
pub fn avatar(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.accent_soft.into()),
        text_color: Some(t.accent),
        border: Border::default().rounded(f32::MAX),
        ..container::Style::default()
    }
}

/// A checkbox's box, filled once anything is checked.
pub fn checkbox(filled: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        let (background, border) = if filled {
            (t.accent, t.accent)
        } else {
            (t.surface, t.border_strong)
        };
        container::Style {
            background: Some(background.into()),
            border: Border {
                color: border,
                width: CHECKBOX_BORDER,
                radius: Radius::from(RADIUS_CHECKBOX),
            },
            ..container::Style::default()
        }
    }
}

/// The dash in a checkbox when only some of what it covers is checked.
pub fn checkbox_dash(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).on_accent.into()),
        border: Border::default().rounded(1.0),
        ..container::Style::default()
    }
}

/// A pill with a short fact, such as where a folder syncs to.
pub fn chip(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.muted),
        border: outline(t.border, f32::MAX),
        ..container::Style::default()
    }
}

/// The heading of a list's first group, following the box's corners.
pub fn first_section(theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(tokens(theme).text),
        ..header(theme)
    }
}

/// A heading between the groups of a list.
pub fn section(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface_hover.into()),
        ..container::Style::default()
    }
}

/// A notice about what went wrong.
pub fn alert_error(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.danger_soft.into()),
        text_color: Some(t.danger),
        border: Border::default().rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// A notice about how something went.
pub fn alert_info(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.accent_soft.into()),
        text_color: Some(t.text),
        border: Border::default().rounded(RADIUS_SM),
        ..container::Style::default()
    }
}

/// A value shown like an input that cannot be typed in.
pub fn field(theme: &Theme) -> container::Style {
    let t = tokens(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: outline(t.border_strong, RADIUS_SM),
        ..container::Style::default()
    }
}

/// The disc behind the icon of a dialog that warns.
pub fn danger_disc(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).danger_soft.into()),
        border: Border::default().rounded(DISC_RADIUS),
        ..container::Style::default()
    }
}

/// What the listing turns into while files are dragged over the window.
pub fn drop_zone(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(tokens(theme).accent_soft.into()),
        border: Border::default().rounded(RADIUS_MD),
        ..container::Style::default()
    }
}

/// A short state, accent-tinted while it is on.
pub fn pill(on: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let t = tokens(theme);
        let (background, color) = if on {
            (t.accent_soft, t.accent)
        } else {
            (t.surface_hover, t.muted)
        };
        container::Style {
            background: Some(background.into()),
            text_color: Some(color),
            border: Border::default().rounded(f32::MAX),
            ..container::Style::default()
        }
    }
}

/// No colours of its own, where a function has to pick between styles.
pub fn plain(_theme: &Theme) -> container::Style {
    container::Style::default()
}
