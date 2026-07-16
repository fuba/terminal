use base64::Engine;

/// Handle OSC 52 clipboard operation.
/// Format: 52;c;<base64 data> (set clipboard)
/// Format: 52;c;? (query clipboard)
/// Returns SetClipboard(text) or QueryClipboard.
pub enum Osc52Action {
    SetClipboard(String),
    QueryClipboard,
}

pub fn parse(payload: &str) -> Option<Osc52Action> {
    // Strip "52;" prefix
    let rest = payload.strip_prefix("52;")?;

    // Get selection parameter (usually 'c' for clipboard) and data
    let (_sel, data) = rest.split_once(';')?;

    if data == "?" {
        return Some(Osc52Action::QueryClipboard);
    }

    // Decode base64
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    Some(Osc52Action::SetClipboard(text))
}
