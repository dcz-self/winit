/*! Abstraction over Wayland text input implementations.
 * 
 * Created to accommodate experimental protocol improvements.
 */

pub mod v3;

use sctk::reexports::client::QueueHandle;
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::ZwpTextInputV3;
// unverified
pub use v3::{ClientState, ZwpTextInputV3Ext};
use wayland_client::globals::{BindError, GlobalList};
use wayland_client::protocol::wl_seat::WlSeat;

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

#[derive(Debug)]
pub enum TextInput {
    V3(ZwpTextInputV3),
}

impl TextInput {
    pub fn destroy(&self) {
        match self {
            Self::V3(obj) => obj.destroy(),
        }
    }
}