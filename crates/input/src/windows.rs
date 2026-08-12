use crate::{
    DisplayGeometry, InputBackend, InputError, VirtualDesktopGeometry, map_to_virtual_coordinate,
};
use remotex_protocol::{ButtonState, InputEvent, MouseButton, WheelAxis};
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
        MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
        MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput,
    },
    WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, XBUTTON1, XBUTTON2,
    },
};

pub struct WindowsMouseBackend {
    display: DisplayGeometry,
    virtual_desktop: VirtualDesktopGeometry,
}

impl WindowsMouseBackend {
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

    fn send_mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> Result<(), InputError> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let input_size = i32::try_from(std::mem::size_of::<INPUT>())
            .map_err(|_| InputError::Other("Windows INPUT size is out of range".to_owned()))?;
        // SAFETY: INPUT and its active mouse union member are fully initialized. SendInput copies
        // exactly one fixed-size value before returning.
        let inserted = unsafe { SendInput(&[input], input_size) };
        if inserted != 1 {
            return Err(InputError::Other(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }

    fn mouse_button(button: MouseButton, state: ButtonState) -> (MOUSE_EVENT_FLAGS, u32) {
        match (button, state) {
            (MouseButton::Left, ButtonState::Down) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, ButtonState::Up) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, ButtonState::Down) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, ButtonState::Up) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, ButtonState::Down) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, ButtonState::Up) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, ButtonState::Down) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON1)),
            (MouseButton::Back, ButtonState::Up) => (MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
            (MouseButton::Forward, ButtonState::Down) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON2)),
            (MouseButton::Forward, ButtonState::Up) => (MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
        }
    }
}

impl InputBackend for WindowsMouseBackend {
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
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
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
                    WheelAxis::Vertical => MOUSEEVENTF_WHEEL,
                    WheelAxis::Horizontal => MOUSEEVENTF_HWHEEL,
                };
                Self::send_mouse(0, 0, u32::from_ne_bytes(delta.to_ne_bytes()), flags)
            }
            InputEvent::Key { .. } => Err(InputError::Unsupported),
        }
    }
}
