mod english;
mod portuguese_brazil;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageId {
    English,
    PortugueseBrazil,
}

impl LanguageId {
    pub const ALL: [LanguageId; 2] = [
        LanguageId::English,
        LanguageId::PortugueseBrazil,
    ];

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::PortugueseBrazil => "Português (Brasil)",
        }
    }

    pub fn strings(self) -> Strings {
        match self {
            Self::English => english::STRINGS,
            Self::PortugueseBrazil => portuguese_brazil::STRINGS,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Strings {
    pub window_title: &'static str,
    pub refresh: &'static str,
    pub update_frequency: &'static str,
    pub one_minute: &'static str,
    pub five_minutes: &'static str,
    pub fifteen_minutes: &'static str,
    pub one_hour: &'static str,
    pub settings: &'static str,
    pub start_with_windows: &'static str,
    pub reset_position: &'static str,
    pub language: &'static str,
    pub system_default: &'static str,
    pub exit: &'static str,
    pub show_widget: &'static str,
    pub session_window: &'static str,
    pub weekly_window: &'static str,
    pub now: &'static str,
    pub day_suffix: &'static str,
    pub hour_suffix: &'static str,
    pub minute_suffix: &'static str,
    pub second_suffix: &'static str,
}

pub fn resolve_language(language_override: Option<LanguageId>) -> LanguageId {
    language_override.unwrap_or(LanguageId::PortugueseBrazil)
}
