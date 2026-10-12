use super::MouseEncoding;

/// Encode wheel input only. Button clicks, releases and drags stay local.
pub fn wheel_report(
    encoding: MouseEncoding,
    delta: i32,
    row: usize,
    col: usize,
    modifiers: u8,
) -> Option<Vec<u8>> {
    let button = match delta.cmp(&0) {
        std::cmp::Ordering::Greater => 64,
        std::cmp::Ordering::Less => 65,
        std::cmp::Ordering::Equal => return None,
    };
    let code = button | (modifiers & 28);
    match encoding {
        MouseEncoding::Sgr => Some(format!("\x1b[<{};{};{}M", code, col + 1, row + 1).into_bytes()),
        MouseEncoding::Normal if row < 223 && col < 223 => Some(vec![
            0x1b,
            b'[',
            b'M',
            code + 32,
            col as u8 + 33,
            row as u8 + 33,
        ]),
        MouseEncoding::Normal => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_wheel_only_reports_scroll_buttons_at_one_based_coordinates() {
        assert_eq!(
            wheel_report(MouseEncoding::Sgr, 120, 2, 4, 0).unwrap(),
            b"\x1b[<64;5;3M"
        );
        assert_eq!(
            wheel_report(MouseEncoding::Sgr, -120, 2, 4, 0).unwrap(),
            b"\x1b[<65;5;3M"
        );
        assert_eq!(
            wheel_report(MouseEncoding::Sgr, 120, 2, 4, 16).unwrap(),
            b"\x1b[<80;5;3M"
        );
        assert!(wheel_report(MouseEncoding::Sgr, 0, 2, 4, 0).is_none());
    }

    #[test]
    fn legacy_wheel_respects_coordinate_limits() {
        assert_eq!(
            wheel_report(MouseEncoding::Normal, -120, 0, 0, 0).unwrap(),
            [27, 91, 77, 97, 33, 33]
        );
        assert!(wheel_report(MouseEncoding::Normal, 120, 223, 0, 0).is_none());
        assert!(wheel_report(MouseEncoding::Normal, 120, 0, 223, 0).is_none());
        assert_eq!(
            wheel_report(MouseEncoding::Normal, 120, 222, 222, 0).unwrap(),
            [27, 91, 77, 96, 255, 255]
        );
        assert!(wheel_report(MouseEncoding::Sgr, 120, 300, 400, 0).is_some());
    }
}
