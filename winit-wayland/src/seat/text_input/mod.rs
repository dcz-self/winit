/*! Abstraction over Wayland text input implementations.
 * 
 * Created to accommodate experimental protocol improvements.
 */

pub mod v3;
pub mod xx;

use dpi::{LogicalPosition, LogicalSize};
use sctk::reexports::client::QueueHandle;
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
use sctk::reexports::protocols_experimental::text_input::v3::client::xx_text_input_v3::XxTextInputV3;
use tracing::{info, warn};

use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::wl_seat::WlSeat;
use winit_core::window::{ImeCapabilities, ImeHint, ImePurpose, ImeRequestData, ImeSurroundingText};

use crate::state::WinitState;

#[derive(Debug)]
pub enum TextInputState {
    V3(v3::TextInputState),
    Xx(xx::TextInputState),
}

impl TextInputState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        match xx::TextInputState::new(globals, queue_handle) {
            Ok(state) => {
                info!("Using experimental text input protocol");
                return Ok(Self::Xx(state));
            }
            Err(e) => {
                info!("Failed to use xx-text-input: {e}");
            }
        };
        Ok(Self::V3(v3::TextInputState::new(globals, queue_handle)?))
    }
    
    pub fn get_text_input(&self, seat: &WlSeat, qh: &QueueHandle<WinitState>) -> TextInput {
        match self {
            Self::V3(mgr) => TextInput::V3(mgr.get_text_input(seat, qh, Default::default())),
            Self::Xx(mgr) => TextInput::Xx(mgr.get_text_input(seat, qh, Default::default())),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum TextInput {
    V3(ZwpTextInputV3),
    Xx(XxTextInputV3),
}

impl TextInput {
    pub fn set_state(&self, state: Option<&ClientState>, send_enable: bool) {
        match self {
            Self::V3(obj) => obj.set_state(state.into(), send_enable),
            Self::Xx(obj) => obj.set_state(state.into(), send_enable),
        }
    }

    pub fn destroy(&self) {
        match self {
            Self::V3(obj) => obj.destroy(),
            Self::Xx(obj) => obj.destroy(),
        }
    }
    
    pub fn as_ref(&self) -> TextInputRef<'_> {
        match self {
            TextInput::V3(v) => TextInputRef::V3(v),
            TextInput::Xx(v) => TextInputRef::Xx(v),
        }
    }
}


// This reference type is not necessary if it's decided that .clone() on a Wayland object is cheap enough once in a while
#[derive(Debug, PartialEq)]
pub enum TextInputRef<'a> {
    V3(&'a ZwpTextInputV3),
    Xx(&'a XxTextInputV3),
}

impl<'a> TextInputRef<'a> {
    // This could be implemented using std::borrow::Borrow, but that's a lot of coding for little savings - ownership is needed in one place as of writing.
    pub fn into_owned(&self) -> TextInput {
        match self {
            Self::V3(v) => TextInput::V3((*v).clone()),
            Self::Xx(v) => TextInput::Xx((*v).clone()),
        }
    }
}

impl From<ZwpTextInputV3> for TextInput {
    fn from(value: ZwpTextInputV3) -> Self {
        Self::V3(value)
    }
}

impl<'a> From<&'a ZwpTextInputV3> for TextInputRef<'a> {
    fn from(value: &'a ZwpTextInputV3) -> Self {
        Self::V3(value)
    }
}

impl From<XxTextInputV3> for TextInput {
    fn from(value: XxTextInputV3) -> Self {
        Self::Xx(value)
    }
}

impl<'a> From<&'a XxTextInputV3> for TextInputRef<'a> {
    fn from(value: &'a XxTextInputV3) -> Self {
        Self::Xx(value)
    }
}

impl<'a> PartialEq<TextInput> for TextInputRef<'a> {
    fn eq(&self, other: &TextInput) -> bool {
        self == &other.as_ref()
    }
}

impl<'a> PartialEq<TextInputRef<'a>> for TextInput {
    fn eq(&self, other: &TextInputRef<'a>) -> bool {
        other.eq(self)
    }
}

trait TextInputExt {
    /// Applies the entire state atomically to the input method.
    ///
    /// It will send the "enable" request and warn user about capabilities unsupported by the backend
    /// if and only if `send_enable` is `true`.
    fn set_state(&self, state: Option<&ClientState>, send_enable: bool);
}

/// State requested by the application.
///
/// This is a version that uses text_input abstractions translated from the ones used in
/// winit::core::window::ImeStateChange.
///
/// Fields that are initially set to None are unsupported capabilities
/// and trying to set them raises an error.
#[derive(Debug, PartialEq, Clone)]
pub struct ClientState {
    capabilities: ImeCapabilities,
    content_type: (ImeHint, ImePurpose),
    /// The IME cursor area which should not be covered by the input method popup.
    cursor_area: (LogicalPosition<u32>, LogicalSize<u32>),

    /// The `ImeSurroundingText` struct is based on the Wayland model.
    /// When this changes, another struct might be needed.
    surrounding_text: ImeSurroundingText,
}

impl ClientState {
    pub fn new(
        capabilities: ImeCapabilities,
        request_data: ImeRequestData,
        scale_factor: f64,
    ) -> Self {
        let mut this = Self {
            capabilities,
            content_type: Default::default(),
            cursor_area: Default::default(),
            surrounding_text: ImeSurroundingText::new(String::new(), 0, 0).unwrap(),
        };

        this.update(request_data, scale_factor);
        this
    }

    pub fn capabilities(&self) -> ImeCapabilities {
        self.capabilities
    }

    /// Updates the fields of the state which are present in update_fields.
    pub fn update(&mut self, request_data: ImeRequestData, scale_factor: f64) {
        if let Some((hint, purpose)) = request_data.hint_and_purpose {
            if self.capabilities.hint_and_purpose() {
                self.content_type = (hint, purpose);
            } else {
                warn!("discarding IME hint and purpose update because capability is not enabled.");
            }
        }

        if let Some((position, size)) = request_data.cursor_area {
            if self.capabilities.cursor_area() {
                let position: LogicalPosition<u32> = position.to_logical(scale_factor);
                let size: LogicalSize<u32> = size.to_logical(scale_factor);
                self.cursor_area = (position, size);
            } else {
                warn!("discarding IME cursor area update because capability is not enabled.");
            }
        }

        if let Some(surrounding) = request_data.surrounding_text {
            if self.capabilities.surrounding_text() {
                self.surrounding_text = surrounding;
            } else {
                warn!("discarding IME surrounding text update because capability is not enabled.");
            }
        }
    }

    pub fn content_type(&self) -> Option<(ImeHint, ImePurpose)> {
        self.capabilities.hint_and_purpose().then_some(self.content_type)
    }

    pub fn cursor_area(&self) -> Option<(LogicalPosition<u32>, LogicalSize<u32>)> {
        self.capabilities.cursor_area().then_some(self.cursor_area)
    }

    pub fn surrounding_text(&self) -> Option<&ImeSurroundingText> {
        self.capabilities.surrounding_text().then_some(&self.surrounding_text)
    }
}
