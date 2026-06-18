use crate::dc::DeviceContext;
use crate::window::WxWidget;

/// A device context to draw on a window including both the client and non-client areas.
///
/// WindowDC allows drawing on the entire window area, including borders, title bar, etc.
/// For drawing only in the client area, use ClientDC instead.
pub struct WindowDC {
    dc_ptr: *mut wxdragon_sys::wxd_WindowDC_t,
}

impl WindowDC {
    /// Create a new WindowDC for the specified window
    ///
    /// # Arguments
    /// * `window` - The window to draw on
    pub fn new<W: WxWidget>(window: &W) -> Self {
        let ptr = window.handle_ptr();
        let dc_ptr = unsafe { wxdragon_sys::wxd_WindowDC_Create(ptr) };
        Self { dc_ptr }
    }
}

impl DeviceContext for WindowDC {
    fn dc_ptr(&self) -> *mut wxdragon_sys::wxd_DC_t {
        unsafe { wxdragon_sys::wxd_WindowDC_AsDC(self.dc_ptr) }
    }
}

impl Drop for WindowDC {
    fn drop(&mut self) {
        unsafe {
            wxdragon_sys::wxd_WindowDC_Destroy(self.dc_ptr);
        }
    }
}
