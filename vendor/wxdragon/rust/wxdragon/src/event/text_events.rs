//! Event system for text input controls.

use crate::event::event_data::CommandEventData;
use crate::event::{Event, EventType};

/// Events specific to text input controls (TextCtrl, SearchCtrl, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEvent {
    /// Fired when the text changes
    Updated,
    /// Fired when Enter key is pressed in the control
    EnterPressed,
}

/// Event data for text events
#[derive(Debug)]
pub struct TextEventData {
    pub event: CommandEventData,
}

impl TextEventData {
    pub fn new(event: Event) -> Self {
        Self {
            event: CommandEventData::new(event),
        }
    }

    /// Get the widget ID that generated the event
    pub fn get_id(&self) -> i32 {
        self.event.get_id()
    }

    /// Get the string value from the text control
    pub fn get_string(&self) -> Option<String> {
        self.event.get_string()
    }
}

// Use the macro to implement the trait
crate::implement_category_event_handlers!(TextEvents, TextEvent, TextEventData,
    Updated => text_updated, EventType::TEXT,
    EnterPressed => enter_pressed, EventType::TEXT_ENTER
);
