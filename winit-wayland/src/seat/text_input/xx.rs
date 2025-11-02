use std::ops::Deref;

use dpi::{LogicalPosition, LogicalSize};
use sctk::globals::GlobalData;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, delegate_dispatch};
use sctk::reexports::protocols_experimental::text_input::v3::client::xx_text_input_manager_v3::XxTextInputManagerV3;
use sctk::reexports::protocols_experimental::text_input::v3::client::xx_text_input_v3::{
    Action, ContentHint, ContentPurpose, Event as TextInputEvent, XxTextInputV3
};
use tracing::warn;
use wayland_client::WEnum;
use winit_core::event::{Ime, WindowEvent};
use winit_core::window::{
    ImeCapabilities, ImeHint, ImePurpose, ImeSurroundingText,
};

use crate::state::WinitState;
use super::TextInputExt;

#[derive(Debug)]
pub struct TextInputState {
    text_input_manager: XxTextInputManagerV3,
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
    type Target = XxTextInputManagerV3;

    fn deref(&self) -> &Self::Target {
        &self.text_input_manager
    }
}

impl Dispatch<XxTextInputManagerV3, GlobalData, WinitState> for TextInputState {
    fn event(
        _state: &mut WinitState,
        _proxy: &XxTextInputManagerV3,
        _event: <XxTextInputManagerV3 as Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<XxTextInputV3, TextInputData, WinitState> for TextInputState {
    fn event(
        state: &mut WinitState,
        text_input: &XxTextInputV3,
        event: <XxTextInputV3 as Proxy>::Event,
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

                if let Some(text_input_state) = window.text_input_state() {
                    text_input.set_state(Some(text_input_state), true);
                    // The input method doesn't have to reply anything, so a synthetic event
                    // carrying an empty state notifies the application about its presence.
                    state.events_sink.push_window_event(WindowEvent::Ime(Ime::Enabled), window_id);
                }

                window.text_input_entered(text_input.into());
            },
            TextInputEvent::Leave { surface } => {
                text_input_data.surface = None;
                text_input_data.last_preedit_empty = true;

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

                window.text_input_left(text_input.into());

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
            TextInputEvent::DeleteSurroundingText { before_length, after_length } => {
                text_input_data.pending_delete = Some(DeleteSurroundingText {
                    before: before_length as usize,
                    after: after_length as usize,
                });
            },
            TextInputEvent::MoveCursor { cursor, anchor } => {
                text_input_data.pending_move = Some(MoveCursor { cursor, anchor});
            },
            TextInputEvent::PerformAction { action } => {
                text_input_data.pending_action = match action {
                    WEnum::Value(action) => Some(action),
                    WEnum::Unknown(v) => {
                        // Honor that input method intended to overwrite already enqueued action, even if we don't know what was coming instead
                        warn!("Received request to perform unknown action {v}. Performing nothing instead.");
                        None
                    }
                };
            },
            TextInputEvent::Done { .. } => {
                let window_id = match text_input_data.surface.as_ref() {
                    Some(surface) => crate::make_wid(surface),
                    None => return,
                };

                // Just in case some IME sends an event for the disabled window.
                if let Some(window) = windows.get(&window_id) {
                    if window.lock().unwrap().text_input_state().is_none() {
                        return;
                    }
                };

                // The events are sent to the user individually, so
                // CAUTION: events must always arrive in the order compatible with the application
                // order specified by the text-input-v3 protocol:
                //
                // As of version 2 (experimental):
                                
                // 1. Replace existing preedit string with the cursor.
                // 2. Delete requested surrounding text.
                // 3. Insert commit string with the cursor at its end.
                // 4. Move the cursor and selection.
                // 5. Calculate surrounding text to send.
                // 6. Insert new preedit text in cursor position.
                // 7. Place cursor inside preedit text.
                // 8. Perform the requested action.
                
                // TODO: perform the action

                if let Some(DeleteSurroundingText { before, after }) =
                    text_input_data.pending_delete
                {
                    state.events_sink.push_window_event(
                        WindowEvent::Ime(Ime::DeleteSurrounding {
                            before_bytes: before,
                            after_bytes: after,
                        }),
                        window_id,
                    );
                }

                // Clear preedit, unless all we'll be doing next is sending a new preedit and
                // the last preedit wasn't empty.
                if text_input_data.pending_commit.is_some()
                    || (text_input_data.pending_preedit.is_none()
                        && !text_input_data.last_preedit_empty)
                {
                    state.events_sink.push_window_event(
                        WindowEvent::Ime(Ime::Preedit(String::new(), None)),
                        window_id,
                    );
                    text_input_data.last_preedit_empty = true;
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

                    text_input_data.last_preedit_empty = false;
                    state.events_sink.push_window_event(
                        WindowEvent::Ime(Ime::Preedit(preedit.text, cursor_range)),
                        window_id,
                    );
                }
            },
            _ => {},
        }
    }
}

impl TextInputExt for XxTextInputV3 {
    fn set_state(&self, state: Option<&super::ClientState>, send_enable: bool) {
        let state = match state {
            Some(state) => {
                let (state, unsupported_flags) = ClientState::new(state.clone());
                if send_enable {
                    if unsupported_flags != ImeCapabilities::new() {
                        warn!(
                            "Backend doesn't support all requested IME capabilities: {:?}.\n Ignoring.",
                            unsupported_flags
                        );
                    }
                }
                state
            },
            None => {
                self.disable();
                self.commit();
                return;
            },
        };

        if send_enable {
            self.enable();
        }

        if let Some(content_type) = state.content_type() {
            self.set_content_type(content_type.hint, content_type.purpose);
        }

        if let Some((position, size)) = state.cursor_area() {
            let (x, y) = (position.x as i32, position.y as i32);
            let (width, height) = (size.width as i32, size.height as i32);
            // The same cursor can be applied on different seats.
            // It's the compositor's responsibility to make sure that any present popups don't
            // overlap.
            self.set_cursor_rectangle(x, y, width, height);
        }

        if let Some(surrounding) = state.surrounding_text() {
            self.set_surrounding_text(
                surrounding.text().into(),
                surrounding.cursor() as i32,
                surrounding.anchor() as i32,
            );
        }

        self.commit();
    }
}

/// The Data associated with the text input.
#[derive(Default)]
pub struct TextInputData {
    inner: std::sync::Mutex<TextInputDataInner>,
}

pub struct TextInputDataInner {
    /// The `WlSurface` we're performing input to.
    surface: Option<WlSurface>,

    /// The commit to submit on `done`.
    pending_commit: Option<String>,

    /// The preedit to submit on `done`.
    pending_preedit: Option<Preedit>,

    /// The text around the cursor to delete on `done`
    pending_delete: Option<DeleteSurroundingText>,

    /// The new span to select on `done`.
    pending_move: Option<MoveCursor>,
    
    /// The action to perform on `done`.
    pending_action: Option<Action>,

    /// Last preedit empty.
    last_preedit_empty: bool,
}

impl Default for TextInputDataInner {
    fn default() -> Self {
        Self {
            surface: None,
            pending_commit: None,
            pending_preedit: None,
            pending_delete: None,
            pending_move: None,
            pending_action: None,
            last_preedit_empty: true,
        }
    }
}

/// The state of the preedit.
#[derive(Clone)]
struct Preedit {
    text: String,
    cursor_begin: Option<usize>,
    cursor_end: Option<usize>,
}

/// The delete request
#[derive(Clone)]
struct DeleteSurroundingText {
    /// Bytes before cursor
    before: usize,
    /// Bytes after cursor
    after: usize,
}

/// The move_cursor request
#[derive(Clone)]
struct MoveCursor {
    /// The beginning of the resulting selection
    anchor: i32,
    /// The end of the resulting selection, with  active cursor
    cursor: i32,
}

/// State change requested by the application.
///
/// This is a version that uses text_input abstractions translated from the ones used in
/// winit::core::window::ImeStateChange.
///
/// Fields that are initially set to None are unsupported capabilities
/// and trying to set them raises an error.
#[derive(Debug, PartialEq, Clone)]
struct ClientState {
    capabilities: ImeCapabilities,
    content_type: ContentType,
    /// The IME cursor area which should not be covered by the input method popup.
    cursor_area: (LogicalPosition<u32>, LogicalSize<u32>),

    /// The `ImeSurroundingText` struct is based on the Wayland model.
    /// When this changes, another struct might be needed.
    surrounding_text: ImeSurroundingText,
}

impl ClientState {
    fn new(value: super::ClientState) -> (Self, ImeCapabilities) {
        let super::ClientState {
            capabilities,
            content_type,
            cursor_area,
            surrounding_text,
        } = value;

        let unsupported_flags = capabilities
            .without_hint_and_purpose()
            .without_cursor_area()
            .without_surrounding_text();
        
        let ret = Self {
            capabilities,
            content_type: content_type.into(),
            cursor_area,
            surrounding_text,
        };
        
        (ret, unsupported_flags)
    }

    pub fn content_type(&self) -> Option<ContentType> {
        self.capabilities.hint_and_purpose().then_some(self.content_type)
    }

    pub fn cursor_area(&self) -> Option<(LogicalPosition<u32>, LogicalSize<u32>)> {
        self.capabilities.cursor_area().then_some(self.cursor_area)
    }

    pub fn surrounding_text(&self) -> Option<&ImeSurroundingText> {
        self.capabilities.surrounding_text().then_some(&self.surrounding_text)
    }
}

/// Arguments to content_type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentType {
    /// Text input hint.
    hint: ContentHint,
    /// Text input purpose.
    purpose: ContentPurpose,
}

/// The two options influence each other, so they must be converted together.
impl From<(ImeHint, ImePurpose)> for ContentType {
    fn from((hint, purpose): (ImeHint, ImePurpose)) -> Self {
        let purpose = match purpose {
            ImePurpose::Password => ContentPurpose::Password,
            ImePurpose::Terminal => ContentPurpose::Terminal,
            ImePurpose::Phone => ContentPurpose::Phone,
            ImePurpose::Number => ContentPurpose::Number,
            ImePurpose::Url => ContentPurpose::Url,
            ImePurpose::Email => ContentPurpose::Email,
            ImePurpose::Pin => ContentPurpose::Pin,
            ImePurpose::Date => ContentPurpose::Date,
            ImePurpose::Time => ContentPurpose::Time,
            ImePurpose::DateTime => ContentPurpose::Datetime,
            _ => ContentPurpose::Normal,
        };

        let base_hint = match purpose {
            // Before the hint API was introduced, password  purpose guaranteed the
            // sensitive hint. Keep this behaviour for the sake of backwards compatibility.
            ContentPurpose::Password | ContentPurpose::Pin => ContentHint::SensitiveData,
            _ => ContentHint::None,
        };

        let mut new_hint = base_hint;
        if hint.contains(ImeHint::COMPLETION) {
            new_hint |= ContentHint::Completion;
        }
        if hint.contains(ImeHint::SPELLCHECK) {
            new_hint |= ContentHint::Spellcheck;
        }
        if hint.contains(ImeHint::AUTO_CAPITALIZATION) {
            new_hint |= ContentHint::AutoCapitalization;
        }
        if hint.contains(ImeHint::LOWERCASE) {
            new_hint |= ContentHint::Lowercase;
        }
        if hint.contains(ImeHint::UPPERCASE) {
            new_hint |= ContentHint::Uppercase;
        }
        if hint.contains(ImeHint::TITLECASE) {
            new_hint |= ContentHint::Titlecase;
        }
        if hint.contains(ImeHint::HIDDEN_TEXT) {
            new_hint |= ContentHint::HiddenText;
        }
        if hint.contains(ImeHint::SENSITIVE_DATA) {
            new_hint |= ContentHint::SensitiveData;
        }
        if hint.contains(ImeHint::LATIN) {
            new_hint |= ContentHint::Latin;
        }
        if hint.contains(ImeHint::MULTILINE) {
            new_hint |= ContentHint::Multiline;
        }

        Self { hint: new_hint, purpose }
    }
}

impl Default for ContentType {
    fn default() -> Self {
        ContentType { purpose: ContentPurpose::Normal, hint: ContentHint::None }
    }
}

delegate_dispatch!(WinitState: [XxTextInputManagerV3: GlobalData] => TextInputState);
delegate_dispatch!(WinitState: [XxTextInputV3: TextInputData] => TextInputState);
