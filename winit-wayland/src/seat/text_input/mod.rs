use std::ops::Deref;

use dpi::{LogicalPosition, LogicalSize};
use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3;
use sctk::reexports::protocols::wp::text_input::zv3::client::zwp_text_input_v3::{
    ContentHint, ContentPurpose, Event as TextInputEvent, ZwpTextInputV3,
};
use winit_core::event::{Ime, WindowEvent};
use winit_core::window::{ImePurpose, ImeStateChange, ImeUnsupportedCapability};

use crate::state::WinitState;

#[derive(Debug)]
pub struct TextInputState {
    text_input_manager: ZwpTextInputManagerV3,
}

impl TextInputState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let text_input_manager = globals.bind(queue_handle, 1..=1, GlobalData)?;
        Ok(Self { text_input_manager })
    }
}

impl Deref for TextInputState {
    type Target = ZwpTextInputManagerV3;

    fn deref(&self) -> &Self::Target {
        &self.text_input_manager
    }
}

impl Dispatch<ZwpTextInputManagerV3, GlobalData, WinitState> for TextInputState {
    fn event(
        _state: &mut WinitState,
        _proxy: &ZwpTextInputManagerV3,
        _event: <ZwpTextInputManagerV3 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<ZwpTextInputV3, TextInputData, WinitState> for TextInputState {
    fn event(
        state: &mut WinitState,
        text_input: &ZwpTextInputV3,
        event: <ZwpTextInputV3 as Proxy>::Event,
        data: &TextInputData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        let windows = state.windows.get_mut();
        let mut text_input_data = data.inner.lock().unwrap();
        match event {
            TextInputEvent::Enter { surface } => {
                let window_id = crate::make_wid(&surface);
                text_input_data.surface = Some(surface);

                let mut window = match windows.get(&window_id) {
                    Some(window) => window.lock().unwrap(),
                    None => return,
                };

                if let Some(im_state) = window.text_input_state() {
                    text_input.set_state(Some(im_state.clone()), false);
                    // The input method doesn't have to reply anything, so a synthetic event
                    // carrying an empty state notifies the application about its presence.
                    state.events_sink.push_window_event(WindowEvent::Ime(Ime::Enabled), window_id);
                }

                window.text_input_entered(text_input);
            },
            TextInputEvent::Leave { surface } => {
                text_input_data.surface = None;

                // Always issue a disable.
                text_input.disable();
                text_input.commit();

                let window_id = crate::make_wid(&surface);

                // XXX this check is essential, because `leave` could have a
                // reference to nil surface...
                let mut window = match windows.get(&window_id) {
                    Some(window) => window.lock().unwrap(),
                    None => return,
                };

                window.text_input_left(text_input);

                state.events_sink.push_window_event(WindowEvent::Ime(Ime::Disabled), window_id);
            },
            TextInputEvent::PreeditString { text, cursor_begin, cursor_end } => {
                let text = text.unwrap_or_default();
                let cursor_begin = usize::try_from(cursor_begin)
                    .ok()
                    .and_then(|idx| text.is_char_boundary(idx).then_some(idx));
                let cursor_end = usize::try_from(cursor_end)
                    .ok()
                    .and_then(|idx| text.is_char_boundary(idx).then_some(idx));

                text_input_data.pending_preedit = Some(Preedit { text, cursor_begin, cursor_end })
            },
            TextInputEvent::CommitString { text } => {
                text_input_data.pending_preedit = None;
                text_input_data.pending_commit = text;
            },
            TextInputEvent::Done { .. } => {
                let window_id = match text_input_data.surface.as_ref() {
                    Some(surface) => crate::make_wid(surface),
                    None => return,
                };

                // Clear preedit, unless all we'll be doing next is sending a new preedit.
                if text_input_data.pending_commit.is_some()
                    || text_input_data.pending_preedit.is_none()
                {
                    state.events_sink.push_window_event(
                        WindowEvent::Ime(Ime::Preedit(String::new(), None)),
                        window_id,
                    );
                }

                // Send `Commit`.
                if let Some(text) = text_input_data.pending_commit.take() {
                    state
                        .events_sink
                        .push_window_event(WindowEvent::Ime(Ime::Commit(text)), window_id);
                }

                // Send preedit.
                if let Some(preedit) = text_input_data.pending_preedit.take() {
                    let cursor_range =
                        preedit.cursor_begin.map(|b| (b, preedit.cursor_end.unwrap_or(b)));

                    state.events_sink.push_window_event(
                        WindowEvent::Ime(Ime::Preedit(preedit.text, cursor_range)),
                        window_id,
                    );
                }
            },
            TextInputEvent::DeleteSurroundingText { .. } => {
                // Not handled.
            },
            _ => {},
        }
    }
}

pub trait ZwpTextInputV3Ext {
    /// Applies the entire state atomically to the input method. It will skip the "enable" request
    /// if `already_enabled` is `true`.
    fn set_state(&self, state: Option<ClientState>, already_enabled: bool);
}

impl ZwpTextInputV3Ext for ZwpTextInputV3 {
    fn set_state(&self, state: Option<ClientState>, already_enabled: bool) {
        if let Some(ClientState { content_type, cursor_area }) = state {
            // text_input.enabled() resets some state on every call in text_input_v3.
            // This might be an abundance of caution, though. We do update the entire state on every
            // call... do we? Things to watch out for: whether the enable event
            // affects the flow of filtered keyboard events or the visible popups.
            if !already_enabled {
                self.enable();
            }
            if let Some(ContentType { purpose, hint }) = content_type {
                self.set_content_type(hint, purpose);
            }
            if let Some((position, size)) = cursor_area {
                let (x, y) = (position.x as i32, position.y as i32);
                let (width, height) = (size.width as i32, size.height as i32);
                // The same cursor can be applied on different seats.
                // It's the compositor's responsibility to make sure that any present popups don't
                // overlap.
                self.set_cursor_rectangle(x, y, width, height);
            }
            // TODO; surrounding text
        } else {
            self.disable();
        }
        self.commit();
    }
}

/// The Data associated with the text input.
#[derive(Default)]
pub struct TextInputData {
    inner: std::sync::Mutex<TextInputDataInner>,
}

#[derive(Default)]
pub struct TextInputDataInner {
    /// The `WlSurface` we're performing input to.
    surface: Option<WlSurface>,

    /// The commit to submit on `done`.
    pending_commit: Option<String>,

    /// The preedit to submit on `done`.
    pending_preedit: Option<Preedit>,
}

/// The state of the preedit.
#[derive(Clone)]
struct Preedit {
    text: String,
    cursor_begin: Option<usize>,
    cursor_end: Option<usize>,
}

/// Arguments to content_type
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentType {
    /// Text input purpose
    purpose: ContentPurpose,
    hint: ContentHint,
}

/// State change requested by the application.
///
/// This is a version that uses text_input abstractions translated from the ones used in
/// winit::core::window::ImeStateChange.
///
/// Fields that are initially set to None are unsupported capabilities
/// and trying to set them raises an error.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct ClientState {
    content_type: Option<ContentType>,
    /// The IME cursor area which should not be covered by the input method popup.
    cursor_area: Option<(LogicalPosition<u32>, LogicalSize<u32>)>,
}

impl ClientState {    
    /// Updates the fields of the state which are present in update_fields.
    /// If no previous state present, this initializes it.
    pub fn update(state: Option<Self>, update_fields: &ImeStateChange, scale_factor: f64) -> Result<Self, ImeUnsupportedCapability> {
        let initialize = state.is_none();
        let state = state.unwrap_or_default();
        state.update_inner(update_fields, scale_factor, !initialize)
    }

    /// Converts the units from the windowing system into units expected by the Wayland protocol. If `check` is `true`, it will fail on attempts to set a value that was not advertised as a capability.
    fn update_inner(
        self,
        ImeStateChange { purpose, cursor_area, .. }: &ImeStateChange, 
        scale_factor: f64,
        check: bool,
    ) -> Result<Self, ImeUnsupportedCapability> {
        let content_type = purpose.map(|purpose| match purpose {
            ImePurpose::Password => (ContentHint::SensitiveData, ContentPurpose::Password),
            ImePurpose::Terminal => (ContentHint::None, ContentPurpose::Terminal),
            _ => (ContentHint::None, ContentPurpose::Normal),
        });
        let content_type = content_type.map(|(hint, purpose)| ContentType { hint, purpose});
        
        let cursor_area = cursor_area.map(|(position, size)| {
            let position: LogicalPosition<u32> = position.to_logical(scale_factor);
            let size: LogicalSize<u32> = size.to_logical(scale_factor);
            (position, size)
        });

        Ok(Self {
            cursor_area: Self::update_field_by_cap(self.cursor_area, cursor_area, check).map_err(|()| ImeUnsupportedCapability::CURSOR_AREA)?,
            content_type: Self::update_field_by_cap(self.content_type, content_type, check).map_err(|()| ImeUnsupportedCapability::PURPOSE)?,
        })
    }
    
    fn update_field_by_cap<T>(cap: Option<T>, value: Option<T>, check: bool) -> Result<Option<T>, ()> {
        match (cap.is_some(), value.is_some()) {
            // No such capability, but attempting to set
            (false, true) => if check {
                Err(())
            } else {
                Ok(value)
            },
            // Simple update of value
            (true, true) => Ok(value),
            // The capability also serves as the old value
            (_, false) => Ok(cap),
        }
    }
}


delegate_dispatch!(WinitState: [ZwpTextInputManagerV3: GlobalData] => TextInputState);
delegate_dispatch!(WinitState: [ZwpTextInputV3: TextInputData] => TextInputState);
