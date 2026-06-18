//! Event system for button-like controls.

use crate::event::event_data::CommandEventData;
use crate::event::{Event, EventType};

/// Events specific to button-like controls (Button, BitmapButton, ToggleButton)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    /// Fired when the button is clicked
    Clicked,
    /// Fired when a toggle button changes state
    Toggled,
}

/// Event data for button events
#[derive(Debug)]
pub struct ButtonEventData {
    pub event: CommandEventData,
}

impl ButtonEventData {
    pub fn new(event: Event) -> Self {
        Self {
            event: CommandEventData::new(event),
        }
    }

    /// Get the widget ID that generated the event
    pub fn get_id(&self) -> i32 {
        self.event.get_id()
    }

    /// Check if the button is checked/toggled
    pub fn is_checked(&self) -> Option<bool> {
        self.event.is_checked()
    }
}

// Use the macro to implement the trait
crate::implement_category_event_handlers!(ButtonEvents, ButtonEvent, ButtonEventData,
    Clicked => click, EventType::COMMAND_BUTTON_CLICKED,
    Toggled => toggle, EventType::COMMAND_TOGGLEBUTTON_CLICKED
);
