pub(super) fn strip_nul(value: &str) -> String {
    value.replace('\0', "")
}
