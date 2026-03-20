pub enum Action {
    Print(char),
    C0Control(u8),
    CsiDispatch {
        params: Vec<i32>,
        private_marker: Option<u8>,
        intermediates: Vec<u8>,
        final_byte: u8,
    },
    EscDispatch {
        intermediates: Vec<u8>,
        final_byte: u8,
    },
    OscDispatch(Vec<u8>),
    DcsDispatch(Vec<u8>),
}

#[derive(PartialEq)]
enum State {
    Ground,
    Utf8,
    Escape,
    EscapeIntermediate,
    CsiEntry,
    CsiParam,
    CsiIntermediate,
    CsiIgnore,
    OscString,
    OscEnd,
    DcsString,
    DcsEnd,
}

pub struct Parser {
    state: State,
    params: Vec<i32>,
    current_param: i32,
    has_param: bool,
    private_marker: Option<u8>,
    intermediates: Vec<u8>,
    osc_data: Vec<u8>,
    dcs_data: Vec<u8>,
    utf8_buf: u32,
    utf8_remaining: u8,
}

impl Parser {
    pub fn new() -> Self {
        Parser {
            state: State::Ground,
            params: Vec::new(),
            current_param: 0,
            has_param: false,
            private_marker: None,
            intermediates: Vec::new(),
            osc_data: Vec::new(),
            dcs_data: Vec::new(),
            utf8_buf: 0,
            utf8_remaining: 0,
        }
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Action> {
        let mut actions = Vec::new();
        for &byte in data {
            self.advance(byte, &mut actions);
        }
        actions
    }

    fn advance(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match self.state {
            State::Ground => self.ground(byte, actions),
            State::Utf8 => self.utf8(byte, actions),
            State::Escape => self.escape(byte, actions),
            State::EscapeIntermediate => self.escape_intermediate(byte, actions),
            State::CsiEntry => self.csi_entry(byte, actions),
            State::CsiParam => self.csi_param(byte, actions),
            State::CsiIntermediate => self.csi_intermediate(byte, actions),
            State::CsiIgnore => self.csi_ignore(byte),
            State::OscString => self.osc_string(byte, actions),
            State::OscEnd => self.osc_end(byte, actions),
            State::DcsString => self.dcs_string(byte, actions),
            State::DcsEnd => self.dcs_end(byte, actions),
        }
    }

    fn clear(&mut self) {
        self.params.clear();
        self.current_param = 0;
        self.has_param = false;
        self.private_marker = None;
        self.intermediates.clear();
    }

    fn finish_param(&mut self) {
        if self.has_param || !self.params.is_empty() {
            self.params.push(self.current_param);
            self.current_param = 0;
            self.has_param = false;
        }
    }

    fn ground(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x00..=0x1A | 0x1C..=0x1F => {
                if byte == 0x1B {
                    self.clear();
                    self.state = State::Escape;
                } else {
                    actions.push(Action::C0Control(byte));
                }
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            0x20..=0x7E => {
                actions.push(Action::Print(byte as char));
            }
            0x7F => {}
            0xC2..=0xDF => {
                self.utf8_buf = (byte as u32) & 0x1F;
                self.utf8_remaining = 1;
                self.state = State::Utf8;
            }
            0xE0..=0xEF => {
                self.utf8_buf = (byte as u32) & 0x0F;
                self.utf8_remaining = 2;
                self.state = State::Utf8;
            }
            0xF0..=0xF4 => {
                self.utf8_buf = (byte as u32) & 0x07;
                self.utf8_remaining = 3;
                self.state = State::Utf8;
            }
            _ => {}
        }
    }

    fn utf8(&mut self, byte: u8, actions: &mut Vec<Action>) {
        if byte & 0xC0 == 0x80 {
            self.utf8_buf = (self.utf8_buf << 6) | ((byte as u32) & 0x3F);
            self.utf8_remaining -= 1;
            if self.utf8_remaining == 0 {
                if let Some(ch) = char::from_u32(self.utf8_buf) {
                    actions.push(Action::Print(ch));
                }
                self.state = State::Ground;
            }
        } else {
            self.state = State::Ground;
            self.advance(byte, actions);
        }
    }

    fn escape(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x20..=0x2F => {
                self.intermediates.push(byte);
                self.state = State::EscapeIntermediate;
            }
            b'[' => {
                self.state = State::CsiEntry;
            }
            b']' => {
                self.osc_data.clear();
                self.state = State::OscString;
            }
            b'P' => {
                // DCS - Device Control String
                self.dcs_data.clear();
                self.state = State::DcsString;
            }
            b'X' | b'^' | b'_' => {
                // SOS, PM, APC - skip until ST
                self.osc_data.clear();
                self.state = State::OscString;
            }
            0x30..=0x7E => {
                actions.push(Action::EscDispatch {
                    intermediates: self.intermediates.clone(),
                    final_byte: byte,
                });
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                // Double ESC - stay in escape
            }
            0x00..=0x1A | 0x1C..=0x1F => {
                actions.push(Action::C0Control(byte));
            }
            _ => {
                self.state = State::Ground;
            }
        }
    }

    fn escape_intermediate(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x20..=0x2F => {
                self.intermediates.push(byte);
            }
            0x30..=0x7E => {
                actions.push(Action::EscDispatch {
                    intermediates: self.intermediates.clone(),
                    final_byte: byte,
                });
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            _ => {
                self.clear();
                self.state = State::Ground;
            }
        }
    }

    fn csi_entry(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            b'0'..=b'9' => {
                self.current_param = (byte - b'0') as i32;
                self.has_param = true;
                self.state = State::CsiParam;
            }
            b';' => {
                self.params.push(0);
                self.state = State::CsiParam;
            }
            b'<' | b'=' | b'>' | b'?' => {
                self.private_marker = Some(byte);
                self.state = State::CsiParam;
            }
            0x20..=0x2F => {
                self.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                actions.push(Action::CsiDispatch {
                    params: self.params.clone(),
                    private_marker: self.private_marker,
                    intermediates: self.intermediates.clone(),
                    final_byte: byte,
                });
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            0x00..=0x1A | 0x1C..=0x1F => {
                actions.push(Action::C0Control(byte));
            }
            _ => {
                self.clear();
                self.state = State::Ground;
            }
        }
    }

    fn csi_param(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            b'0'..=b'9' => {
                self.current_param = self.current_param * 10 + (byte - b'0') as i32;
                self.has_param = true;
            }
            b';' => {
                self.finish_param();
            }
            b':' => {
                // Sub-parameter separator (used in some SGR forms)
                self.finish_param();
            }
            b'<' | b'=' | b'>' | b'?' => {
                if self.private_marker.is_none() {
                    self.private_marker = Some(byte);
                }
            }
            0x20..=0x2F => {
                self.finish_param();
                self.intermediates.push(byte);
                self.state = State::CsiIntermediate;
            }
            0x40..=0x7E => {
                self.finish_param();
                actions.push(Action::CsiDispatch {
                    params: self.params.clone(),
                    private_marker: self.private_marker,
                    intermediates: self.intermediates.clone(),
                    final_byte: byte,
                });
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            0x00..=0x1A | 0x1C..=0x1F => {
                actions.push(Action::C0Control(byte));
            }
            _ => {
                self.state = State::CsiIgnore;
            }
        }
    }

    fn csi_intermediate(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x20..=0x2F => {
                self.intermediates.push(byte);
            }
            0x40..=0x7E => {
                self.finish_param();
                actions.push(Action::CsiDispatch {
                    params: self.params.clone(),
                    private_marker: self.private_marker,
                    intermediates: self.intermediates.clone(),
                    final_byte: byte,
                });
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            _ => {
                self.state = State::CsiIgnore;
            }
        }
    }

    fn csi_ignore(&mut self, byte: u8) {
        match byte {
            0x40..=0x7E => {
                self.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.clear();
                self.state = State::Escape;
            }
            _ => {}
        }
    }

    fn osc_string(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x07 => {
                actions.push(Action::OscDispatch(self.osc_data.clone()));
                self.osc_data.clear();
                self.state = State::Ground;
            }
            0x1B => {
                self.state = State::OscEnd;
            }
            _ => {
                if self.osc_data.len() < 4096 {
                    self.osc_data.push(byte);
                }
            }
        }
    }

    fn osc_end(&mut self, byte: u8, actions: &mut Vec<Action>) {
        if byte == b'\\' {
            actions.push(Action::OscDispatch(self.osc_data.clone()));
            self.osc_data.clear();
            self.state = State::Ground;
        } else {
            self.osc_data.clear();
            self.state = State::Escape;
            self.escape(byte, actions);
        }
    }

    fn dcs_string(&mut self, byte: u8, actions: &mut Vec<Action>) {
        match byte {
            0x1B => {
                self.state = State::DcsEnd;
            }
            _ => {
                if self.dcs_data.len() < 1024 * 1024 {
                    // Allow up to 1MB for image data
                    self.dcs_data.push(byte);
                }
            }
        }
    }

    fn dcs_end(&mut self, byte: u8, actions: &mut Vec<Action>) {
        if byte == b'\\' {
            actions.push(Action::DcsDispatch(self.dcs_data.clone()));
            self.dcs_data.clear();
            self.state = State::Ground;
        } else {
            self.dcs_data.clear();
            self.state = State::Escape;
            self.escape(byte, actions);
        }
    }
}
