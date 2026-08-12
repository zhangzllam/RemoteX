//! Permissioned, platform-neutral remote input execution and pressed-state tracking.

use remotex_protocol::{ButtonState, DisplayId, InputEvent, MouseButton};
use std::collections::HashSet;
use thiserror::Error;

const NORMALIZED_MAX: u64 = 65_535;
const MOUSE_BUTTONS: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];

#[derive(Debug, Error)]
pub enum InputError {
    #[error("input permission denied")]
    PermissionDenied,
    #[error("normalized coordinate is not finite")]
    InvalidCoordinate,
    #[error("display geometry is invalid")]
    InvalidDisplayGeometry,
    #[error("input event targets an unavailable display {0}")]
    UnknownDisplay(DisplayId),
    #[error("unsupported input event")]
    Unsupported,
    #[error("input execution failed: {0}")]
    Other(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayGeometry {
    pub id: DisplayId,
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VirtualDesktopGeometry {
    pub origin_x: i32,
    pub origin_y: i32,
    pub width: u32,
    pub height: u32,
}

/// Converts a browser-style unit coordinate to the protocol's full `u16` range.
/// Finite values outside 0.0-1.0 are clamped; NaN and infinities are rejected.
pub fn normalize_unit_coordinate(value: f64) -> Result<u16, InputError> {
    if !value.is_finite() {
        return Err(InputError::InvalidCoordinate);
    }
    let scaled = value.clamp(0.0, 1.0) * f64::from(u16::MAX);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(scaled.round() as u16)
}

/// Maps a display-relative normalized coordinate into the Windows virtual-desktop range.
pub fn map_to_virtual_coordinate(
    normalized: u16,
    display_origin: i32,
    display_extent: u32,
    virtual_origin: i32,
    virtual_extent: u32,
) -> Result<i32, InputError> {
    if display_extent == 0 || virtual_extent < 2 {
        return Err(InputError::InvalidDisplayGeometry);
    }
    let pixel_offset = (u64::from(normalized) * u64::from(display_extent - 1) + NORMALIZED_MAX / 2)
        / NORMALIZED_MAX;
    let pixel = i64::from(display_origin)
        .checked_add(i64::try_from(pixel_offset).map_err(|_| InputError::InvalidDisplayGeometry)?)
        .ok_or(InputError::InvalidDisplayGeometry)?;
    let virtual_last = i64::from(virtual_extent - 1);
    let relative = (pixel - i64::from(virtual_origin)).clamp(0, virtual_last);
    let mapped = (relative * i64::from(u16::MAX) + virtual_last / 2) / virtual_last;
    i32::try_from(mapped).map_err(|_| InputError::InvalidDisplayGeometry)
}

pub trait InputBackend: Send {
    fn execute(&mut self, event: &InputEvent) -> Result<(), InputError>;
}

pub trait InputController: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), InputError>;
    fn release_all(&mut self) -> Result<(), InputError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputState {
    pressed_mouse_buttons: HashSet<MouseButton>,
}

impl InputState {
    #[must_use]
    pub fn is_mouse_button_pressed(&self, button: MouseButton) -> bool {
        self.pressed_mouse_buttons.contains(&button)
    }

    #[must_use]
    pub fn pressed_mouse_button_count(&self) -> usize {
        self.pressed_mouse_buttons.len()
    }
}

/// Enforces the Session permission and guarantees injected buttons are released on drop.
pub struct PermissionedInputController<B: InputBackend> {
    backend: B,
    state: InputState,
    control_input: bool,
}

impl<B: InputBackend> PermissionedInputController<B> {
    #[must_use]
    pub fn new(backend: B, control_input: bool) -> Self {
        Self {
            backend,
            state: InputState::default(),
            control_input,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &InputState {
        &self.state
    }

    fn transition_button(
        &mut self,
        button: MouseButton,
        state: ButtonState,
    ) -> Result<(), InputError> {
        let pressed = self.state.pressed_mouse_buttons.contains(&button);
        if (state == ButtonState::Down && pressed) || (state == ButtonState::Up && !pressed) {
            return Ok(());
        }
        let event = match state {
            ButtonState::Down => InputEvent::MouseButtonDown { button },
            ButtonState::Up => InputEvent::MouseButtonUp { button },
        };
        self.backend.execute(&event)?;
        match state {
            ButtonState::Down => {
                self.state.pressed_mouse_buttons.insert(button);
            }
            ButtonState::Up => {
                self.state.pressed_mouse_buttons.remove(&button);
            }
        }
        Ok(())
    }

    fn release_pressed_buttons(&mut self) -> Result<(), InputError> {
        let mut first_error = None;
        for button in MOUSE_BUTTONS {
            if !self.state.pressed_mouse_buttons.remove(&button) {
                continue;
            }
            if let Err(error) = self.backend.execute(&InputEvent::MouseButtonUp { button }) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

impl<B: InputBackend> InputController for PermissionedInputController<B> {
    fn apply(&mut self, event: InputEvent) -> Result<(), InputError> {
        if !self.control_input {
            return Err(InputError::PermissionDenied);
        }
        match event {
            InputEvent::MouseButtonDown { button } => {
                self.transition_button(button, ButtonState::Down)
            }
            InputEvent::MouseButtonUp { button } => self.transition_button(button, ButtonState::Up),
            InputEvent::MouseMove { .. } | InputEvent::MouseWheel { .. } => {
                self.backend.execute(&event)
            }
            InputEvent::Key { .. } => Err(InputError::Unsupported),
        }
    }

    fn release_all(&mut self) -> Result<(), InputError> {
        self.release_pressed_buttons()
    }
}

impl<B: InputBackend> Drop for PermissionedInputController<B> {
    fn drop(&mut self) {
        let _result = self.release_pressed_buttons();
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

#[cfg(windows)]
pub use windows::WindowsMouseBackend;

#[cfg(test)]
mod tests {
    use super::*;
    use remotex_protocol::WheelAxis;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingBackend {
        events: Vec<InputEvent>,
    }

    impl InputBackend for RecordingBackend {
        fn execute(&mut self, event: &InputEvent) -> Result<(), InputError> {
            self.events.push(event.clone());
            Ok(())
        }
    }

    #[test]
    fn unit_coordinates_are_normalized_clamped_and_validated() {
        assert_eq!(normalize_unit_coordinate(0.0).expect("zero"), 0);
        assert_eq!(normalize_unit_coordinate(0.5).expect("half"), 32_768);
        assert_eq!(normalize_unit_coordinate(1.0).expect("one"), u16::MAX);
        assert_eq!(normalize_unit_coordinate(-2.0).expect("clamp low"), 0);
        assert_eq!(
            normalize_unit_coordinate(2.0).expect("clamp high"),
            u16::MAX
        );
        assert!(normalize_unit_coordinate(f64::NAN).is_err());
        assert!(normalize_unit_coordinate(f64::INFINITY).is_err());
    }

    #[test]
    fn selected_display_coordinates_map_into_virtual_desktop() {
        assert_eq!(
            map_to_virtual_coordinate(0, 0, 1920, -1280, 3200).expect("left edge"),
            26_222
        );
        assert_eq!(
            map_to_virtual_coordinate(u16::MAX, 0, 1920, -1280, 3200).expect("right edge"),
            65_535
        );
        assert!(map_to_virtual_coordinate(0, 0, 0, 0, 1920).is_err());
    }

    #[test]
    fn permission_is_enforced_before_backend_execution() {
        let mut controller = PermissionedInputController::new(RecordingBackend::default(), false);
        assert!(matches!(
            controller.apply(InputEvent::MouseWheel {
                axis: WheelAxis::Vertical,
                delta: 120,
            }),
            Err(InputError::PermissionDenied)
        ));
        assert!(controller.backend.events.is_empty());
    }

    #[test]
    fn duplicate_transitions_are_ignored_and_release_all_unsticks_buttons() {
        let mut controller = PermissionedInputController::new(RecordingBackend::default(), true);
        controller
            .apply(InputEvent::MouseButtonDown {
                button: MouseButton::Left,
            })
            .expect("button down");
        controller
            .apply(InputEvent::MouseButtonDown {
                button: MouseButton::Left,
            })
            .expect("duplicate down");
        assert!(
            controller
                .state()
                .is_mouse_button_pressed(MouseButton::Left)
        );
        assert_eq!(controller.backend.events.len(), 1);

        controller.release_all().expect("release all");
        assert_eq!(controller.state().pressed_mouse_button_count(), 0);
        assert_eq!(
            controller.backend.events,
            vec![
                InputEvent::MouseButtonDown {
                    button: MouseButton::Left
                },
                InputEvent::MouseButtonUp {
                    button: MouseButton::Left
                }
            ]
        );
    }

    #[test]
    fn normal_down_up_sequences_support_double_clicks() {
        let mut controller = PermissionedInputController::new(RecordingBackend::default(), true);
        for _ in 0..2 {
            controller
                .apply(InputEvent::MouseButtonDown {
                    button: MouseButton::Left,
                })
                .expect("button down");
            controller
                .apply(InputEvent::MouseButtonUp {
                    button: MouseButton::Left,
                })
                .expect("button up");
        }
        assert_eq!(controller.backend.events.len(), 4);
    }

    #[test]
    fn dropping_controller_releases_pressed_buttons() {
        struct SharedRecordingBackend(Arc<Mutex<Vec<InputEvent>>>);

        impl InputBackend for SharedRecordingBackend {
            fn execute(&mut self, event: &InputEvent) -> Result<(), InputError> {
                self.0
                    .lock()
                    .expect("event recording lock")
                    .push(event.clone());
                Ok(())
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        {
            let mut controller =
                PermissionedInputController::new(SharedRecordingBackend(Arc::clone(&events)), true);
            controller
                .apply(InputEvent::MouseButtonDown {
                    button: MouseButton::Right,
                })
                .expect("button down");
        }
        assert_eq!(
            *events.lock().expect("event recording lock"),
            vec![
                InputEvent::MouseButtonDown {
                    button: MouseButton::Right
                },
                InputEvent::MouseButtonUp {
                    button: MouseButton::Right
                }
            ]
        );
    }
}
