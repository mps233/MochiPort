use serde::Serialize;
use wxdragon::prelude::*;

use super::api::{RequestLogDetail, RequestLogItem};
use super::text::GuiText;

const STYLE_JSON_DEFAULT: i32 = 0;
const STYLE_JSON_KEY: i32 = 1;
const STYLE_JSON_STRING: i32 = 2;
const STYLE_JSON_NUMBER: i32 = 3;
const STYLE_JSON_KEYWORD: i32 = 4;
const STYLE_JSON_PUNCT: i32 = 5;
const STYLE_LINE_NUMBER: i32 = 33;
const STYLE_INDENT_GUIDE: i32 = 37;
const FOLD_BASE: i32 = 0x400;
const FOLD_HEADER: i32 = 0x2000;
const FOLD_MARGIN: i32 = 2;
const FOLD_MARKER_MASK: i32 = 0xFE00_0000u32 as i32;
const FOLD_FLAG_LINE_BEFORE_CONTRACTED: i32 = 0x0004;
const FOLD_FLAG_LINE_AFTER_CONTRACTED: i32 = 0x0010;
const MARKER_FOLDER_END: i32 = 25;
const MARKER_FOLDER_OPEN_MID: i32 = 26;
const MARKER_FOLDER_MID_TAIL: i32 = 27;
const MARKER_FOLDER_TAIL: i32 = 28;
const MARKER_FOLDER_SUB: i32 = 29;
const MARKER_FOLDER: i32 = 30;
const MARKER_FOLDER_OPEN: i32 = 31;

pub(super) fn show(parent: &Frame, text: GuiText, detail: &RequestLogDetail) {
    let id = detail.summary.id;
    let dialog = Dialog::builder(parent, &text.request_log_detail_title(id))
        .with_style(
            DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder | DialogStyle::MaximizeBox,
        )
        .with_size(1480, 900)
        .build();
    dialog.set_min_size(Size::new(1120, 760));
    dialog.set_background_color(Colour::rgb(250, 251, 253));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(250, 251, 253));

    let root = BoxSizer::builder(Orientation::Vertical).build();

    let summary = StaticText::builder(&panel)
        .with_label(&summary_text(&detail.summary))
        .build();
    summary.set_foreground_color(Colour::rgb(57, 65, 80));
    root.add(
        &summary,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        12,
    );

    let notebook = Notebook::builder(&panel).build();
    add_json_tab(
        &notebook,
        text.request_log_detail_codex_request(),
        detail.request_json.as_deref(),
        text,
    );
    add_json_tab(
        &notebook,
        text.request_log_detail_upstream_request(),
        detail.upstream_request_json.as_deref(),
        text,
    );
    add_json_tab(
        &notebook,
        text.request_log_detail_response(),
        detail.response_json.as_deref(),
        text,
    );
    if let Some(error) = detail.summary.error_message.as_deref() {
        add_text_tab(&notebook, text.request_log_detail_error(), error);
    }
    root.add(&notebook, 1, SizerFlag::Expand | SizerFlag::All, 12);

    let close_button = Button::builder(&panel)
        .with_label(match text.locale {
            super::text::GuiLocale::ZhCn => "关闭",
            super::text::GuiLocale::EnUs => "Close",
        })
        .build();
    {
        let dialog = dialog;
        close_button.on_click(move |_| {
            dialog.end_modal(ID_OK);
        });
    }
    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    button_row.add(&close_button, 0, SizerFlag::All, 0);
    root.add_sizer(
        &button_row,
        0,
        SizerFlag::AlignRight | SizerFlag::Right | SizerFlag::Bottom,
        12,
    );

    panel.set_sizer(root, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();
    dialog.show_modal();
}

fn add_json_tab(parent: &Notebook, label: &str, content: Option<&str>, text: GuiText) {
    let Some(content) = content else {
        add_text_tab(parent, label, text.request_log_detail_empty());
        return;
    };
    let json_value = serde_json::from_str::<serde_json::Value>(content).ok();
    let display = json_value
        .as_ref()
        .and_then(format_json_pretty)
        .unwrap_or_else(|| content.to_string());

    let panel = Panel::builder(parent).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));

    let editor_actions = BoxSizer::builder(Orientation::Horizontal).build();
    let editor_expand_button = Button::builder(&panel)
        .with_label(match text.locale {
            super::text::GuiLocale::ZhCn => "展开代码",
            super::text::GuiLocale::EnUs => "Expand code",
        })
        .build();
    let editor_collapse_button = Button::builder(&panel)
        .with_label(match text.locale {
            super::text::GuiLocale::ZhCn => "折叠代码",
            super::text::GuiLocale::EnUs => "Collapse code",
        })
        .build();
    editor_actions.add(&editor_expand_button, 0, SizerFlag::Right, 8);
    editor_actions.add(&editor_collapse_button, 0, SizerFlag::Right, 0);

    let editor = StyledTextCtrl::builder(&panel)
        .with_size(Size::new(1200, 720))
        .build();
    configure_json_editor(&editor, &display);
    {
        let editor = editor;
        editor_expand_button.on_click(move |_| {
            set_all_editor_folds(&editor, true);
        });
    }
    {
        let editor = editor;
        editor_collapse_button.on_click(move |_| {
            set_all_editor_folds(&editor, false);
        });
    }
    {
        let editor = editor;
        editor.on_stc_margin_click(move |event| {
            if event.get_margin() == Some(FOLD_MARGIN)
                && let Some(position) = event.get_position()
            {
                toggle_editor_fold_at_position(&editor, position);
            }
        });
    }
    {
        let editor = editor;
        editor.on_stc_double_click(move |_| {
            toggle_editor_fold_at_line(&editor, editor.get_current_line());
        });
    }

    let sizer = BoxSizer::builder(Orientation::Vertical).build();
    sizer.add_sizer(&editor_actions, 0, SizerFlag::Expand | SizerFlag::All, 8);
    sizer.add(&editor, 1, SizerFlag::Expand | SizerFlag::All, 8);
    panel.set_sizer(sizer, true);
    parent.add_page(&panel, label, false, None);
}

fn format_json_pretty(value: &serde_json::Value) -> Option<String> {
    let mut output = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut output, formatter);
    value.serialize(&mut serializer).ok()?;
    String::from_utf8(output).ok()
}

fn add_text_tab(parent: &Notebook, label: &str, content: &str) {
    let panel = Panel::builder(parent).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));

    let editor = TextCtrl::builder(&panel)
        .with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly | TextCtrlStyle::DontWrap)
        .build();
    editor.set_value(content);

    let sizer = BoxSizer::builder(Orientation::Vertical).build();
    sizer.add(&editor, 1, SizerFlag::Expand | SizerFlag::All, 0);
    panel.set_sizer(sizer, true);
    parent.add_page(&panel, label, false, None);
}

fn configure_json_editor(editor: &StyledTextCtrl, content: &str) {
    editor.set_read_only(false);
    editor.set_wrap_mode(WrapMode::None);
    editor.set_tab_width(4);
    editor.set_indent(4);
    editor.set_use_tabs(false);
    editor.set_margin_line_numbers(0, true);
    editor.set_margin_width(0, 52);
    editor.set_margin_width(1, 0);
    editor.set_margin_type_typed(FOLD_MARGIN, MarginType::Symbol);
    editor.set_margin_mask(FOLD_MARGIN, FOLD_MARKER_MASK);
    editor.set_margin_width(FOLD_MARGIN, 18);
    editor.set_margin_sensitive(FOLD_MARGIN, true);
    editor.set_fold_flags(FOLD_FLAG_LINE_BEFORE_CONTRACTED | FOLD_FLAG_LINE_AFTER_CONTRACTED);
    configure_fold_markers(editor);
    editor.set_indentation_guides_typed(IndentationGuide::LookBoth);
    editor.set_view_white_space_typed(WhiteSpaceView::Invisible);
    editor.set_caret_line_visible(true);
    editor.set_caret_line_background(Colour::rgb(244, 247, 251));

    editor.style_set_foreground(STYLE_JSON_DEFAULT, Colour::rgb(38, 45, 56));
    editor.style_set_background(STYLE_JSON_DEFAULT, Colour::rgb(255, 255, 255));
    editor.style_set_size(STYLE_JSON_DEFAULT, 10);
    editor.style_clear_all();
    editor.style_set_foreground(STYLE_JSON_KEY, Colour::rgb(26, 84, 160));
    editor.style_set_bold(STYLE_JSON_KEY, true);
    editor.style_set_foreground(STYLE_JSON_STRING, Colour::rgb(20, 124, 74));
    editor.style_set_foreground(STYLE_JSON_NUMBER, Colour::rgb(151, 71, 0));
    editor.style_set_foreground(STYLE_JSON_KEYWORD, Colour::rgb(128, 61, 150));
    editor.style_set_foreground(STYLE_JSON_PUNCT, Colour::rgb(92, 99, 112));
    editor.style_set_foreground(STYLE_LINE_NUMBER, Colour::rgb(94, 103, 117));
    editor.style_set_background(STYLE_LINE_NUMBER, Colour::rgb(243, 246, 250));
    editor.style_set_foreground(STYLE_INDENT_GUIDE, Colour::rgb(174, 184, 199));
    editor.style_set_background(STYLE_INDENT_GUIDE, Colour::rgb(255, 255, 255));

    editor.set_text(content);
    apply_json_stc_highlight(editor, content);
    apply_json_stc_folding(editor, content);
    editor.empty_undo_buffer();
    editor.set_read_only(true);
}

fn configure_fold_markers(editor: &StyledTextCtrl) {
    let marker_foreground = Colour::rgb(92, 99, 112);
    let marker_background = Colour::rgb(243, 246, 250);
    editor.marker_define_symbol(
        MARKER_FOLDER,
        MarkerSymbol::BoxPlus,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_OPEN,
        MarkerSymbol::BoxMinus,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_SUB,
        MarkerSymbol::VLine,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_END,
        MarkerSymbol::BoxPlusConnected,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_OPEN_MID,
        MarkerSymbol::BoxMinusConnected,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_MID_TAIL,
        MarkerSymbol::TCorner,
        marker_foreground,
        marker_background,
    );
    editor.marker_define_symbol(
        MARKER_FOLDER_TAIL,
        MarkerSymbol::LCorner,
        marker_foreground,
        marker_background,
    );
}

fn apply_json_stc_highlight(editor: &StyledTextCtrl, content: &str) {
    editor.start_styling(0);
    let bytes = content.as_bytes();
    let mut styled_until = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'"' => {
                let start = idx;
                idx += 1;
                let mut escaped = false;
                while idx < bytes.len() {
                    let byte = bytes[idx];
                    idx += 1;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                }
                let end = idx;
                let mut probe = idx;
                while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
                    probe += 1;
                }
                let style = if probe < bytes.len() && bytes[probe] == b':' {
                    STYLE_JSON_KEY
                } else {
                    STYLE_JSON_STRING
                };
                set_json_style(editor, &mut styled_until, start, end, style);
            }
            b'-' | b'0'..=b'9' => {
                let start = idx;
                idx += 1;
                while idx < bytes.len()
                    && matches!(bytes[idx], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    idx += 1;
                }
                set_json_style(editor, &mut styled_until, start, idx, STYLE_JSON_NUMBER);
            }
            b't' | b'f' | b'n' => {
                let start = idx;
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
                    idx += 1;
                }
                set_json_style(editor, &mut styled_until, start, idx, STYLE_JSON_KEYWORD);
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                set_json_style(editor, &mut styled_until, idx, idx + 1, STYLE_JSON_PUNCT);
                idx += 1;
            }
            _ => idx += 1,
        }
    }
    if styled_until < bytes.len() {
        editor.set_styling(
            byte_len_to_i32(bytes.len() - styled_until),
            STYLE_JSON_DEFAULT,
        );
    }
}

fn set_json_style(
    editor: &StyledTextCtrl,
    styled_until: &mut usize,
    start: usize,
    end: usize,
    style: i32,
) {
    if start > *styled_until {
        editor.set_styling(byte_len_to_i32(start - *styled_until), STYLE_JSON_DEFAULT);
        *styled_until = start;
    }
    if end > start {
        editor.set_styling(byte_len_to_i32(end - start), style);
        *styled_until = end;
    }
}

fn apply_json_stc_folding(editor: &StyledTextCtrl, content: &str) {
    let mut depth = 0i32;
    for (line_index, line) in content.lines().enumerate() {
        let line_no = line_index as i32;
        let line_depth = if line_starts_json_close(line) {
            depth.saturating_sub(1)
        } else {
            depth
        };
        let header = line_has_json_fold_header(line);
        let mut level = FOLD_BASE + line_depth.max(0);
        if header {
            level |= FOLD_HEADER;
        }
        editor.set_fold_level(line_no, level);
        if header {
            editor.set_fold_expanded(line_no, true);
        }
        depth = json_depth_after_line(line, depth);
    }
}

fn set_all_editor_folds(editor: &StyledTextCtrl, expand: bool) {
    let line_count = editor.get_line_count();
    for line in 0..line_count {
        if editor.get_fold_level(line) & FOLD_HEADER == 0 {
            continue;
        }
        if editor.get_fold_expanded(line) != expand {
            editor.toggle_fold(line);
        }
        editor.set_fold_expanded(line, expand);
    }
}

fn toggle_editor_fold_at_position(editor: &StyledTextCtrl, position: i32) {
    let line = editor.line_from_position(position);
    toggle_editor_fold_at_line(editor, line);
}

fn toggle_editor_fold_at_line(editor: &StyledTextCtrl, line: i32) {
    if editor.get_fold_level(line) & FOLD_HEADER == 0 {
        return;
    }
    editor.toggle_fold(line);
}

fn line_starts_json_close(line: &str) -> bool {
    line.trim_start()
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'}' | b']'))
}

fn line_has_json_fold_header(line: &str) -> bool {
    let mut in_string = false;
    let mut escaped = false;
    let mut last_structural = None;
    for byte in line.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'}' | b'[' | b']' => last_structural = Some(byte),
            _ => {}
        }
    }
    matches!(last_structural, Some(b'{' | b'['))
}

fn json_depth_after_line(line: &str, current_depth: i32) -> i32 {
    let mut depth = current_depth;
    let mut in_string = false;
    let mut escaped = false;
    for byte in line.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

fn byte_len_to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn summary_text(log: &RequestLogItem) -> String {
    format!(
        "#{}   model={}   channel={}   protocol={}   stream={}   status={}   tokens={}   ttft={}   latency={}   created={}",
        log.id,
        log.model_id,
        log.channel,
        log.provider_type,
        if log.stream { "true" } else { "false" },
        log.status,
        format_tokens(log),
        format_duration(log.ttft_ms),
        format_duration(log.latency_ms),
        log.created_at
    )
}

fn format_tokens(log: &RequestLogItem) -> String {
    match (log.total_tokens, log.input_tokens, log.output_tokens) {
        (Some(total), Some(input), Some(output)) => format!("{total}/{input}/{output}"),
        (Some(total), _, _) => total.to_string(),
        _ => "-".to_string(),
    }
}

fn format_duration(ms: Option<i64>) -> String {
    ms.map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "-".to_string())
}
