use crate::widgets::Panel;
use std::any::Any;
use std::sync::Arc;

/// Context information for virtual list items
///
/// This struct holds the current state for a virtual list item including its index
/// and associated data. It enables safe event handling when panels are reused.
#[derive(Debug, Clone)]
pub struct ItemContext {
    /// The current index of the item in the virtual list
    pub index: usize,
    /// The associated data for this item (type-erased but thread-safe)
    pub data: Arc<dyn Any + Send + Sync>,
}

/// PHASE 3: Deprecated helper functions (replaced by VirtualList instance methods)
///
/// These methods previously used a global registry which has been replaced with
/// instance-owned registries for memory safety and multi-instance support.
impl ItemContext {
    /// DEPRECATED: Use VirtualList::get_item_context_for_panel() instead
    ///
    /// This method used a global registry which has been replaced with instance-owned registry
    /// for memory safety and to prevent conflicts between multiple virtual list instances.
    #[deprecated(
        note = "Use VirtualList::get_item_context_for_panel() instead. Global registry has been replaced with instance-owned registry for memory safety."
    )]
    pub fn get_for_panel(_panel: &Panel) -> Option<ItemContext> {
        // This method is deprecated and no longer functional
        // Users should use VirtualList::get_item_context_for_panel() instead
        None
    }

    /// DEPRECATED: Use VirtualList::get_index_for_panel() instead
    ///
    /// The global registry approach has been replaced with instance-owned registries.
    #[deprecated(note = "Use VirtualList::get_index_for_panel() instead")]
    pub fn get_index_for_panel(_panel: &Panel) -> Option<usize> {
        None
    }

    /// DEPRECATED: Use VirtualList::get_data_for_panel() instead
    ///
    /// The global registry approach has been replaced with instance-owned registries.
    #[deprecated(note = "Use VirtualList::get_data_for_panel() instead")]
    pub fn get_data_for_panel<T>(_panel: &Panel) -> Option<T>
    where
        T: Clone + 'static,
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_item_context_creation() {
        let test_data = "test item data".to_string();
        let context = ItemContext {
            index: 42,
            data: Arc::new(test_data),
        };

        assert_eq!(context.index, 42);
        assert!(context.data.downcast_ref::<String>().is_some());
    }

    /*
    #[test]
    fn test_deprecated_methods_return_none() {
        // These methods should always return None since they're deprecated
        #[allow(deprecated)]
        {
            // Create a dummy panel for testing (we don't actually use it)
            // since the deprecated methods ignore their input
            use crate::widgets::Panel;
            let panel = Panel::builder_for_tests().build();

            assert!(ItemContext::get_for_panel(&panel).is_none());
            assert!(ItemContext::get_index_for_panel(&panel).is_none());
            assert!(ItemContext::get_data_for_panel::<String>(&panel).is_none());
        }
    }
    // */
}
