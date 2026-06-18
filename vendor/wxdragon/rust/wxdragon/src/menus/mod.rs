// wxdragon/src/menus/mod.rs
//! Menu-related widgets and types (`MenuBar`, `Menu`, `MenuItem`).

pub mod menu;
pub mod menubar;
pub mod menuitem;

// Re-export main types
pub use menu::Menu;
pub use menubar::MenuBar;
pub use menuitem::{ItemKind, MenuItem};
