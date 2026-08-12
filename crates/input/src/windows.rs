use crate::{
    DisplayGeometry, InputBackend, InputError, VirtualDesktopGeometry, map_to_virtual_coordinate,
};
use remotex_protocol::{ButtonState, InputEvent, KeyCode, MouseButton, WheelAxis};
use windows::Win32::UI::{
    Input::KeyboardAndMouse as wininput,
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, XBUTTON1, XBUTTON2,
    },
};

/// Windows input adapter. Protocol keys remain platform-neutral until this boundary.
pub struct WindowsInputBackend {
    display: DisplayGeometry,
    virtual_desktop: VirtualDesktopGeometry,
}

impl WindowsInputBackend {
    pub fn new(display: DisplayGeometry) -> Result<Self, InputError> {
        // SAFETY: GetSystemMetrics is a read-only process-independent query with fixed indices.
        let (origin_x, origin_y, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let virtual_desktop = VirtualDesktopGeometry {
            origin_x,
            origin_y,
            width: u32::try_from(width).map_err(|_| InputError::InvalidDisplayGeometry)?,
            height: u32::try_from(height).map_err(|_| InputError::InvalidDisplayGeometry)?,
        };
        if display.width == 0
            || display.height == 0
            || virtual_desktop.width < 2
            || virtual_desktop.height < 2
        {
            return Err(InputError::InvalidDisplayGeometry);
        }
        Ok(Self {
            display,
            virtual_desktop,
        })
    }

    fn send_input(input: wininput::INPUT) -> Result<(), InputError> {
        let input_size = i32::try_from(std::mem::size_of::<wininput::INPUT>())
            .map_err(|_| InputError::Other("Windows INPUT size is out of range".to_owned()))?;
        // SAFETY: The active INPUT union member is initialized by the caller. SendInput copies
        // exactly one fixed-size value before returning.
        let inserted = unsafe { wininput::SendInput(&[input], input_size) };
        if inserted != 1 {
            return Err(InputError::Other(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }

    fn send_mouse(
        dx: i32,
        dy: i32,
        data: u32,
        flags: wininput::MOUSE_EVENT_FLAGS,
    ) -> Result<(), InputError> {
        Self::send_input(wininput::INPUT {
            r#type: wininput::INPUT_MOUSE,
            Anonymous: wininput::INPUT_0 {
                mi: wininput::MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
    }

    fn send_keyboard(key: KeyCode, state: ButtonState) -> Result<(), InputError> {
        let virtual_key = Self::virtual_key(key);
        let mut flags = if Self::is_extended_key(key) {
            wininput::KEYEVENTF_EXTENDEDKEY
        } else {
            wininput::KEYBD_EVENT_FLAGS::default()
        };
        if state == ButtonState::Up {
            flags |= wininput::KEYEVENTF_KEYUP;
        }
        Self::send_input(wininput::INPUT {
            r#type: wininput::INPUT_KEYBOARD,
            Anonymous: wininput::INPUT_0 {
                ki: wininput::KEYBDINPUT {
                    wVk: virtual_key,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
    }

    fn mouse_button(button: MouseButton, state: ButtonState) -> (wininput::MOUSE_EVENT_FLAGS, u32) {
        match (button, state) {
            (MouseButton::Left, ButtonState::Down) => (wininput::MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, ButtonState::Up) => (wininput::MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, ButtonState::Down) => (wininput::MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, ButtonState::Up) => (wininput::MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, ButtonState::Down) => (wininput::MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, ButtonState::Up) => (wininput::MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, ButtonState::Down) => {
                (wininput::MOUSEEVENTF_XDOWN, u32::from(XBUTTON1))
            }
            (MouseButton::Back, ButtonState::Up) => {
                (wininput::MOUSEEVENTF_XUP, u32::from(XBUTTON1))
            }
            (MouseButton::Forward, ButtonState::Down) => {
                (wininput::MOUSEEVENTF_XDOWN, u32::from(XBUTTON2))
            }
            (MouseButton::Forward, ButtonState::Up) => {
                (wininput::MOUSEEVENTF_XUP, u32::from(XBUTTON2))
            }
        }
    }

    const fn is_extended_key(key: KeyCode) -> bool {
        matches!(
            key,
            KeyCode::Delete
                | KeyCode::Insert
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::ArrowLeft
                | KeyCode::ArrowRight
                | KeyCode::ArrowUp
                | KeyCode::ArrowDown
                | KeyCode::ControlRight
                | KeyCode::AltRight
                | KeyCode::SuperLeft
                | KeyCode::SuperRight
        )
    }

    const fn virtual_key(key: KeyCode) -> wininput::VIRTUAL_KEY {
        match key {
            KeyCode::KeyA => wininput::VK_A,
            KeyCode::KeyB => wininput::VK_B,
            KeyCode::KeyC => wininput::VK_C,
            KeyCode::KeyD => wininput::VK_D,
            KeyCode::KeyE => wininput::VK_E,
            KeyCode::KeyF => wininput::VK_F,
            KeyCode::KeyG => wininput::VK_G,
            KeyCode::KeyH => wininput::VK_H,
            KeyCode::KeyI => wininput::VK_I,
            KeyCode::KeyJ => wininput::VK_J,
            KeyCode::KeyK => wininput::VK_K,
            KeyCode::KeyL => wininput::VK_L,
            KeyCode::KeyM => wininput::VK_M,
            KeyCode::KeyN => wininput::VK_N,
            KeyCode::KeyO => wininput::VK_O,
            KeyCode::KeyP => wininput::VK_P,
            KeyCode::KeyQ => wininput::VK_Q,
            KeyCode::KeyR => wininput::VK_R,
            KeyCode::KeyS => wininput::VK_S,
            KeyCode::KeyT => wininput::VK_T,
            KeyCode::KeyU => wininput::VK_U,
            KeyCode::KeyV => wininput::VK_V,
            KeyCode::KeyW => wininput::VK_W,
            KeyCode::KeyX => wininput::VK_X,
            KeyCode::KeyY => wininput::VK_Y,
            KeyCode::KeyZ => wininput::VK_Z,
            KeyCode::Digit0 => wininput::VK_0,
            KeyCode::Digit1 => wininput::VK_1,
            KeyCode::Digit2 => wininput::VK_2,
            KeyCode::Digit3 => wininput::VK_3,
            KeyCode::Digit4 => wininput::VK_4,
            KeyCode::Digit5 => wininput::VK_5,
            KeyCode::Digit6 => wininput::VK_6,
            KeyCode::Digit7 => wininput::VK_7,
            KeyCode::Digit8 => wininput::VK_8,
            KeyCode::Digit9 => wininput::VK_9,
            KeyCode::F1 => wininput::VK_F1,
            KeyCode::F2 => wininput::VK_F2,
            KeyCode::F3 => wininput::VK_F3,
            KeyCode::F4 => wininput::VK_F4,
            KeyCode::F5 => wininput::VK_F5,
            KeyCode::F6 => wininput::VK_F6,
            KeyCode::F7 => wininput::VK_F7,
            KeyCode::F8 => wininput::VK_F8,
            KeyCode::F9 => wininput::VK_F9,
            KeyCode::F10 => wininput::VK_F10,
            KeyCode::F11 => wininput::VK_F11,
            KeyCode::F12 => wininput::VK_F12,
            KeyCode::Enter => wininput::VK_RETURN,
            KeyCode::Escape => wininput::VK_ESCAPE,
            KeyCode::Tab => wininput::VK_TAB,
            KeyCode::Backspace => wininput::VK_BACK,
            KeyCode::Delete => wininput::VK_DELETE,
            KeyCode::Insert => wininput::VK_INSERT,
            KeyCode::Home => wininput::VK_HOME,
            KeyCode::End => wininput::VK_END,
            KeyCode::PageUp => wininput::VK_PRIOR,
            KeyCode::PageDown => wininput::VK_NEXT,
            KeyCode::ArrowLeft => wininput::VK_LEFT,
            KeyCode::ArrowRight => wininput::VK_RIGHT,
            KeyCode::ArrowUp => wininput::VK_UP,
            KeyCode::ArrowDown => wininput::VK_DOWN,
            KeyCode::Space => wininput::VK_SPACE,
            KeyCode::ShiftLeft => wininput::VK_LSHIFT,
            KeyCode::ShiftRight => wininput::VK_RSHIFT,
            KeyCode::ControlLeft => wininput::VK_LCONTROL,
            KeyCode::ControlRight => wininput::VK_RCONTROL,
            KeyCode::AltLeft => wininput::VK_LMENU,
            KeyCode::AltRight => wininput::VK_RMENU,
            KeyCode::SuperLeft => wininput::VK_LWIN,
            KeyCode::SuperRight => wininput::VK_RWIN,
        }
    }
}

impl InputBackend for WindowsInputBackend {
    fn execute(&mut self, event: &InputEvent) -> Result<(), InputError> {
        match event {
            InputEvent::MouseMove {
                display_id,
                normalized_x,
                normalized_y,
            } => {
                if let Some(display_id) = display_id {
                    if display_id != &self.display.id {
                        return Err(InputError::UnknownDisplay(display_id.clone()));
                    }
                }
                let x = map_to_virtual_coordinate(
                    *normalized_x,
                    self.display.origin_x,
                    self.display.width,
                    self.virtual_desktop.origin_x,
                    self.virtual_desktop.width,
                )?;
                let y = map_to_virtual_coordinate(
                    *normalized_y,
                    self.display.origin_y,
                    self.display.height,
                    self.virtual_desktop.origin_y,
                    self.virtual_desktop.height,
                )?;
                Self::send_mouse(
                    x,
                    y,
                    0,
                    wininput::MOUSEEVENTF_MOVE
                        | wininput::MOUSEEVENTF_ABSOLUTE
                        | wininput::MOUSEEVENTF_VIRTUALDESK,
                )
            }
            InputEvent::MouseButtonDown { button } => {
                let (flags, data) = Self::mouse_button(*button, ButtonState::Down);
                Self::send_mouse(0, 0, data, flags)
            }
            InputEvent::MouseButtonUp { button } => {
                let (flags, data) = Self::mouse_button(*button, ButtonState::Up);
                Self::send_mouse(0, 0, data, flags)
            }
            InputEvent::MouseWheel { axis, delta } => {
                let flags = match axis {
                    WheelAxis::Vertical => wininput::MOUSEEVENTF_WHEEL,
                    WheelAxis::Horizontal => wininput::MOUSEEVENTF_HWHEEL,
                };
                Self::send_mouse(0, 0, u32::from_ne_bytes(delta.to_ne_bytes()), flags)
            }
            InputEvent::KeyDown { key } => Self::send_keyboard(*key, ButtonState::Down),
            InputEvent::KeyUp { key } => Self::send_keyboard(*key, ButtonState::Up),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_right_modifiers_use_extended_flag() {
        assert!(WindowsInputBackend::is_extended_key(KeyCode::Delete));
        assert!(WindowsInputBackend::is_extended_key(KeyCode::ControlRight));
        assert!(!WindowsInputBackend::is_extended_key(KeyCode::ControlLeft));
        assert!(!WindowsInputBackend::is_extended_key(KeyCode::KeyA));
    }

    #[test]
    fn protocol_keys_map_to_expected_windows_virtual_keys() {
        assert_eq!(
            WindowsInputBackend::virtual_key(KeyCode::KeyA),
            wininput::VK_A
        );
        assert_eq!(
            WindowsInputBackend::virtual_key(KeyCode::ControlRight),
            wininput::VK_RCONTROL
        );
        assert_eq!(
            WindowsInputBackend::virtual_key(KeyCode::ArrowLeft),
            wininput::VK_LEFT
        );
    }
}
