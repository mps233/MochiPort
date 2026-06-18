use crate::dc::DeviceContext;

/// A device context for drawing directly on the screen.
///
/// ScreenDC allows for capturing screen content or drawing directly on the screen.
pub struct ScreenDC {
    dc_ptr: *mut wxdragon_sys::wxd_ScreenDC_t,
}

impl ScreenDC {
    /// Create a new screen device context
    pub fn new() -> Self {
        let dc_ptr = unsafe { wxdragon_sys::wxd_ScreenDC_Create() };
        Self { dc_ptr }
    }
}

impl DeviceContext for ScreenDC {
    fn dc_ptr(&self) -> *mut wxdragon_sys::wxd_DC_t {
        unsafe { wxdragon_sys::wxd_ScreenDC_AsDC(self.dc_ptr) }
    }
}

impl Drop for ScreenDC {
    fn drop(&mut self) {
        unsafe {
            wxdragon_sys::wxd_ScreenDC_Destroy(self.dc_ptr);
        }
    }
}

impl Default for ScreenDC {
    fn default() -> Self {
        Self::new()
    }
}
