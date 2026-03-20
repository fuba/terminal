use base64::Engine;

/// Parse iTerm2 inline image from OSC 1337 payload.
/// Format: File=name=<b64>;size=<n>;inline=1;width=<w>;height=<h>:<base64 data>
/// Returns (width, height, rgba_data).
pub fn decode(payload: &str) -> Option<(u32, u32, Vec<u8>)> {
    // Split at ':' to get params and data
    let (params_str, data_b64) = payload.split_once(':')?;

    // Parse params
    let params_str = params_str.strip_prefix("File=")?;
    let mut inline = false;
    for param in params_str.split(';') {
        if let Some((key, val)) = param.split_once('=') {
            match key {
                "inline" => inline = val == "1",
                _ => {}
            }
        }
    }

    if !inline {
        return None;
    }

    // Decode base64 image data
    let data = base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .ok()?;

    // Decode image using the image crate
    let img = image::load_from_memory(&data).ok()?;
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();

    Some((width, height, rgba.into_raw()))
}
