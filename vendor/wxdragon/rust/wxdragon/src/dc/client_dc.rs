use crate::dc::DeviceContext;
use crate::window::WxWidget;

/// A device context to draw on the client area of a window.
///
/// ClientDC can be used outside of paint events, but when handling paint events,
/// you should use PaintDC instead.
pub struct ClientDC {
    dc_ptr: *mut wxdragon_sys::wxd_ClientDC_t,
}

impl ClientDC {
    /// Create a new ClientDC for the specified window
    ///
    /// # Arguments
    /// * `window` - The window to draw on
    pub fn new<W: WxWidget>(window: &W) -> Self {
        let ptr = window.handle_ptr();
        let dc_ptr = unsafe { wxdragon_sys::wxd_ClientDC_Create(ptr) };
        Self { dc_ptr }
    }
}

impl DeviceContext for ClientDC {
    fn dc_ptr(&self) -> *mut wxdragon_sys::wxd_DC_t {
        unsafe { wxdragon_sys::wxd_ClientDC_AsDC(self.dc_ptr) }
    }
}

impl Drop for ClientDC {
    fn drop(&mut self) {
        unsafe {
            wxdragon_sys::wxd_ClientDC_Destroy(self.dc_ptr);
        }
    }
}
