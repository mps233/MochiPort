//! AppProgressIndicator module for wxDragon.
//!
//! This module provides a safe wrapper around wxWidgets' wxAppProgressIndicator class.

use crate::window::WxWidget;
use wxdragon_sys as ffi;

/// # Example
///
/// ```rust,no_run
/// use wxdragon::prelude::*;
///
/// wxdragon::main(|_| {
///     let frame = Frame::builder().with_title("My App").build();
///
///     let app_progress = AppProgressIndicator::new(None);
///     app_progress.set_value(0);
///     app_progress.set_range(1000);
///     let mut progress = 0;
///     frame.on_idle(move |_e| {
///         if progress < 1000 {
///             progress += 1;
///             app_progress.set_value(progress);
///         }
///     });
///
///     frame.show(true);
/// })
/// .unwrap();
/// ```
pub struct AppProgressIndicator {
    ptr: *mut ffi::wxd_AppProgressIndicator_t,
}

impl AppProgressIndicator {
    /// Create a new application progress
    pub fn new(parent: Option<&dyn WxWidget>) -> Self {
        let parent_ptr = if let Some(p) = parent {
            p.handle_ptr()
        } else {
            std::ptr::null_mut()
        };
        let ptr = unsafe { ffi::wxd_AppProgressIndicator_Create(parent_ptr) };
        Self { ptr }
    }

    // Check if the application progress display is available.
    pub fn is_available(&self) -> bool {
        unsafe { ffi::wxd_AppProgressIndicator_IsAvailable(self.ptr) }
    }

    // Set the progress value in taskbar button of parent window.
    pub fn set_value(&self, value: i32) {
        unsafe { ffi::wxd_AppProgressIndicator_SetValue(self.ptr, value) }
    }

    // Set the progress range in taskbar button of parent window.
    pub fn set_range(&self, range: i32) {
        unsafe { ffi::wxd_AppProgressIndicator_SetRange(self.ptr, range) }
    }

    // Makes the progress bar run in indeterminate mode.
    pub fn pulse(&self) {
        unsafe { ffi::wxd_AppProgressIndicator_Pulse(self.ptr) }
    }
}

impl Drop for AppProgressIndicator {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ffi::wxd_AppProgressIndicator_Destroy(self.ptr) };
        }
    }
}
