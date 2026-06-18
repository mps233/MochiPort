use crate::dc::DeviceContext;
use crate::window::WxWidget;

/// A device context to draw on a window during a paint event.
///
/// This DC must be created in response to a paint event, and only one
/// PaintDC should exist at a time for a given window.
pub struct PaintDC {
    dc_ptr: *mut wxdragon_sys::wxd_PaintDC_t,
}

impl PaintDC {
    /// Create a new PaintDC for the specified window during a paint event
    ///
    /// # Arguments
    /// * `window` - The window to draw on
    pub fn new<W: WxWidget>(window: &W) -> Self {
        let ptr = window.handle_ptr();
        let dc_ptr = unsafe { wxdragon_sys::wxd_PaintDC_Create(ptr) };
        Self { dc_ptr }
    }
}

impl DeviceContext for PaintDC {
    fn dc_ptr(&self) -> *mut wxdragon_sys::wxd_DC_t {
        unsafe { wxdragon_sys::wxd_PaintDC_AsDC(self.dc_ptr) }
    }
}

impl Drop for PaintDC {
    fn drop(&mut self) {
        unsafe {
            wxdragon_sys::wxd_PaintDC_Destroy(self.dc_ptr);
        }
    }
}
