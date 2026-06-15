use wxdragon::prelude::*;

pub(super) fn set_combo_value_if_changed(input: &ComboBox, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.set_value(value);
}

pub(super) fn change_text_value_if_changed(input: &TextCtrl, value: &str) {
    if input.get_value() == value {
        return;
    }
    input.change_value(value);
}

pub(super) fn strip_nul(value: &str) -> String {
    value.replace('\0', "")
}
