/*! Abstraction over Wayland text input implementations.
 * 
 * Created to accommodate experimental protocol improvements.
 */

pub mod v3;
//pub mod xx;

use sctk::reexports::client::QueueHandle;
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
// unverified
pub use v3::ClientState;
use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::wl_seat::WlSeat;

use crate::seat::text_input::v3::ZwpTextInputV3Ext;
use crate::state::WinitState;

#[derive(Debug)]
pub enum TextInputState {
    V3(v3::TextInputState),
}

impl TextInputState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        // TODO: insert experimental before, checking exact version
        match v3::TextInputState::new(globals, queue_handle) {
            Ok(state) => Ok(Self::V3(state)),
            Err(e) => Err(e),
        }
    }
    
    pub fn get_text_input(&self, seat: &WlSeat, qh: &QueueHandle<WinitState>) -> TextInput {
        match self {
            Self::V3(mgr) => TextInput::V3(mgr.get_text_input(seat, qh, Default::default())),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum TextInput {
    V3(ZwpTextInputV3),
}

impl TextInput {
    pub fn set_state(&self, state: Option<&ClientState>, send_enable: bool) {
        match self {
            Self::V3(obj) => obj.set_state(state, send_enable),
        }
    }

    pub fn destroy(&self) {
        match self {
            Self::V3(obj) => obj.destroy(),
        }
    }
    
    pub fn as_ref(&self) -> TextInputRef<'_> {
        match self {
            TextInput::V3(v) => TextInputRef::V3(v),
        }
    }
}


// This reference type is not necessary if it's decided that .clone() on a Wayland object is cheap enough once in a while
#[derive(Debug, PartialEq)]
pub enum TextInputRef<'a> {
    V3(&'a ZwpTextInputV3),
}

impl<'a> TextInputRef<'a> {
    // This could be implemented using std::borrow::Borrow, but that's a lot of coding for little savings - ownership is needed in one place as of writing.
    pub fn into_owned(&self) -> TextInput {
        match self {
            Self::V3(v) => TextInput::V3((*v).clone())
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
