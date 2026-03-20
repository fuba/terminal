use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Debug)]
pub enum Action {
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    SelectTab(usize),
    Copy,           // Ctrl+C: copy if selection, else send ^C
    Paste,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
    OpenConfig,
    ToggleDockHeight,
    ToggleLog,
}

const MOD_CTRL: u8 = 1;
const MOD_SHIFT: u8 = 2;
#[allow(dead_code)]
const MOD_ALT: u8 = 4;

struct Binding {
    modifiers: u8,
    vk: VIRTUAL_KEY,
    action: Action,
}

pub struct KeybindingEngine {
    bindings: Vec<Binding>,
}

impl KeybindingEngine {
    pub fn new() -> Self {
        let bindings = vec![
            // Tab management
            Binding { modifiers: MOD_CTRL, vk: VK_T, action: Action::NewTab },
            Binding { modifiers: MOD_CTRL, vk: VK_W, action: Action::CloseTab },
            Binding { modifiers: MOD_CTRL, vk: VK_RIGHT, action: Action::NextTab },
            Binding { modifiers: MOD_CTRL, vk: VK_LEFT, action: Action::PrevTab },
            Binding { modifiers: MOD_CTRL, vk: VK_1, action: Action::SelectTab(0) },
            Binding { modifiers: MOD_CTRL, vk: VK_2, action: Action::SelectTab(1) },
            Binding { modifiers: MOD_CTRL, vk: VK_3, action: Action::SelectTab(2) },
            Binding { modifiers: MOD_CTRL, vk: VK_4, action: Action::SelectTab(3) },
            Binding { modifiers: MOD_CTRL, vk: VK_5, action: Action::SelectTab(4) },
            Binding { modifiers: MOD_CTRL, vk: VK_6, action: Action::SelectTab(5) },
            Binding { modifiers: MOD_CTRL, vk: VK_7, action: Action::SelectTab(6) },
            Binding { modifiers: MOD_CTRL, vk: VK_8, action: Action::SelectTab(7) },
            Binding { modifiers: MOD_CTRL, vk: VK_9, action: Action::SelectTab(8) },
            // Clipboard
            Binding { modifiers: MOD_CTRL, vk: VK_C, action: Action::Copy },
            Binding { modifiers: MOD_CTRL, vk: VK_V, action: Action::Paste },
            // Scrollback
            Binding { modifiers: MOD_CTRL, vk: VK_UP, action: Action::ScrollPageUp },
            Binding { modifiers: MOD_CTRL, vk: VK_DOWN, action: Action::ScrollPageDown },
            Binding { modifiers: MOD_CTRL, vk: VK_HOME, action: Action::ScrollToTop },
            Binding { modifiers: MOD_CTRL, vk: VK_END, action: Action::ScrollToBottom },
            // Settings
            Binding { modifiers: MOD_CTRL, vk: VK_OEM_COMMA, action: Action::OpenConfig },
            Binding { modifiers: 0, vk: VK_F11, action: Action::ToggleDockHeight },
            Binding { modifiers: MOD_CTRL, vk: VK_L, action: Action::ToggleLog },
        ];
        KeybindingEngine { bindings }
    }

    pub fn match_key(&self, vk: VIRTUAL_KEY) -> Option<Action> {
        let mods = current_modifiers();
        for b in &self.bindings {
            if b.vk == vk && b.modifiers == mods {
                return Some(b.action.clone());
            }
        }
        None
    }
}

fn current_modifiers() -> u8 {
    let mut m = 0u8;
    if is_key_down(VK_CONTROL) { m |= MOD_CTRL; }
    if is_key_down(VK_SHIFT) { m |= MOD_SHIFT; }
    if is_key_down(VK_MENU) { m |= MOD_ALT; }
    m
}

fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(vk.0 as i32) & 0x8000u16 as i16 != 0 }
}
