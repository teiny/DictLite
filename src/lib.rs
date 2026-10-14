pub mod converter;
pub mod database;
pub mod store;
pub mod shortcut;

pub type Result<T> = std::result::Result<T, String>;
pub fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn normalize(text: &str) -> String {
    text.trim().to_ascii_lowercase()
}
pub fn language(text: &str) -> String {
    if text.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)) {
        "zh"
    } else {
        "en"
    }
    .into()
}
