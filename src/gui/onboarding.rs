use qrcode::{Color, QrCode};
use wxdragon::{prelude::*, timer::Timer};

use super::api::{ApiClient, WechatOnboardPoll};
use super::provider::strip_nul;
use super::show_error;

fn qr_bitmap(value: &str) -> Option<(Bitmap, i32)> {
    let code = QrCode::new(value.as_bytes()).ok()?;
    const TARGET_PIXELS: usize = 560;
    let quiet_zone = 4usize;
    let cells = code.width() + quiet_zone * 2;
    let module_size = (TARGET_PIXELS / cells).clamp(3, 12);
    let image_size = cells * module_size;
    let mut rgba = vec![255u8; image_size * image_size * 4];

    for y in 0..image_size {
        for x in 0..image_size {
            let cell_x = x / module_size;
            let cell_y = y / module_size;
            let dark = cell_x >= quiet_zone
                && cell_y >= quiet_zone
                && cell_x < quiet_zone + code.width()
                && cell_y < quiet_zone + code.width()
                && code[(cell_x - quiet_zone, cell_y - quiet_zone)] == Color::Dark;

            let offset = (y * image_size + x) * 4;
            let value = if dark { 0 } else { 255 };
            rgba[offset] = value;
            rgba[offset + 1] = value;
            rgba[offset + 2] = value;
            rgba[offset + 3] = 255;
        }
    }

    Bitmap::from_rgba(&rgba, image_size as u32, image_size as u32)
        .map(|bitmap| (bitmap, image_size as i32))
}

pub(super) fn prompt_telegram_bot_token(parent: &Frame) -> Option<String> {
    let dialog = Dialog::builder(parent, "添加 Telegram 机器人")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(520, 300)
        .build();
    dialog.set_min_size(Size::new(520, 280));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label("填写 BotFather 提供的 Bot Token")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(
        &title,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let input = TextCtrl::builder(&panel)
        .with_value("")
        .with_style(TextCtrlStyle::Default | TextCtrlStyle::ProcessEnter)
        .build();
    input.set_min_size(Size::new(460, 30));
    sizer.add(
        &input,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let hint = StaticText::builder(&panel)
        .with_label("仅支持与机器人私聊；群聊暂不接入。")
        .build();
    hint.set_foreground_color(Colour::rgb(103, 111, 124));
    sizer.add(
        &hint,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let cancel_button = Button::builder(&panel)
        .with_id(ID_CANCEL)
        .with_label("取消")
        .build();
    let save_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("保存并接入")
        .build();
    save_button.set_default();
    buttons.add_stretch_spacer(1);
    buttons.add(&cancel_button, 0, SizerFlag::Right, 8);
    buttons.add(&save_button, 0, SizerFlag::Right, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom | SizerFlag::Top,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    {
        let dialog = dialog;
        cancel_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }
    {
        let dialog = dialog;
        save_button.on_click(move |_| dialog.end_modal(ID_OK));
    }
    {
        let dialog = dialog;
        input.on_text_enter(move |_| dialog.end_modal(ID_OK));
    }

    input.set_focus();
    let result = dialog.show_modal();
    let token = strip_nul(&input.get_value()).trim().to_string();
    dialog.destroy();

    if result != ID_OK {
        return None;
    }
    if token.is_empty() {
        show_error(parent, "请输入 Telegram Bot Token。");
        return None;
    }
    Some(token)
}

pub(super) fn show_feishu_onboard_dialog(parent: &Frame, api: ApiClient) {
    let start = match api.start_feishu_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, "扫码使用新机器人")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(660, 760)
        .build();
    dialog.set_min_size(Size::new(560, 660));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label("请使用飞书扫码")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.verification_uri_complete) {
        let qr_panel = Panel::builder(&panel).build();
        qr_panel.set_background_color(Colour::rgb(255, 255, 255));
        qr_panel.set_min_size(Size::new(500, 500));

        let qr = StaticBitmap::builder(&qr_panel)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::AspectFit))
            .with_size(Size::new(qr_size.max(500), qr_size.max(500)))
            .build();
        qr.set_min_size(Size::new(500, 500));

        let qr_sizer = BoxSizer::builder(Orientation::Vertical).build();
        qr_sizer.add(&qr, 1, SizerFlag::Expand | SizerFlag::All, 0);
        qr_panel.set_sizer(qr_sizer, true);

        sizer.add(
            &qr_panel,
            1,
            SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&panel)
            .with_label("二维码生成失败，请使用浏览器打开链接。")
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let fallback_link = HyperlinkCtrl::builder(&panel)
        .with_label("扫码失败？打开飞书确认链接")
        .with_url(&start.verification_uri_complete)
        .build();
    sizer.add(
        &fallback_link,
        0,
        SizerFlag::AlignCenterHorizontal | SizerFlag::Bottom,
        12,
    );

    let info = StaticText::builder(&panel)
        .with_label("扫码完成后会自动关闭。")
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(600);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let device_code = start.device_code.clone();
        let dialog = dialog;
        timer.on_tick(move |_| match api.poll_feishu_onboard(&device_code) {
            Ok(result) if result.done => {
                dialog.end_modal(ID_OK);
            }
            Ok(result) => {
                if is_feishu_onboard_pending(result.error.as_ref()) {
                    info.set_label("扫码完成后会自动关闭。");
                } else if result.error.is_some() {
                    info.set_label("接入失败，请关闭后重试。");
                }
            }
            Err(_) => {
                info.set_label("接入失败，请关闭后重试。");
            }
        });
    }
    timer.start(1500, false);

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }

    dialog.show_modal();
    timer.stop();
    dialog.destroy();
}

pub(super) fn show_wechat_onboard_dialog(parent: &Frame, api: ApiClient) {
    let start = match api.start_wechat_onboard() {
        Ok(start) => start,
        Err(err) => {
            show_error(parent, &err);
            return;
        }
    };

    let dialog = Dialog::builder(parent, "扫码连接微信")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(660, 760)
        .build();
    dialog.set_min_size(Size::new(560, 660));
    dialog.set_background_color(Colour::rgb(255, 255, 255));

    let panel = Panel::builder(&dialog).build();
    panel.set_background_color(Colour::rgb(255, 255, 255));
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    let title = StaticText::builder(&panel)
        .with_label("请使用微信扫码")
        .build();
    title.set_foreground_color(Colour::rgb(21, 25, 31));
    sizer.add(&title, 0, SizerFlag::All, 18);

    if let Some((bitmap, qr_size)) = qr_bitmap(&start.qrcode_url) {
        let qr_panel = Panel::builder(&panel).build();
        qr_panel.set_background_color(Colour::rgb(255, 255, 255));
        qr_panel.set_min_size(Size::new(500, 500));

        let qr = StaticBitmap::builder(&qr_panel)
            .with_bitmap(Some(bitmap))
            .with_scale_mode(Some(ScaleMode::AspectFit))
            .with_size(Size::new(qr_size.max(500), qr_size.max(500)))
            .build();
        qr.set_min_size(Size::new(500, 500));

        let qr_sizer = BoxSizer::builder(Orientation::Vertical).build();
        qr_sizer.add(&qr, 1, SizerFlag::Expand | SizerFlag::All, 0);
        qr_panel.set_sizer(qr_sizer, true);

        sizer.add(
            &qr_panel,
            1,
            SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
            12,
        );
    } else {
        let qr_error = StaticText::builder(&panel)
            .with_label("二维码生成失败，请关闭后重试。")
            .build();
        qr_error.set_foreground_color(Colour::rgb(185, 55, 55));
        sizer.add(
            &qr_error,
            0,
            SizerFlag::AlignCenterHorizontal | SizerFlag::Top | SizerFlag::Bottom,
            80,
        );
    }

    let verify_row = BoxSizer::builder(Orientation::Horizontal).build();
    let verify_label = StaticText::builder(&panel).with_label("验证码").build();
    verify_label.set_foreground_color(Colour::rgb(78, 86, 98));
    let verify_code = TextCtrl::builder(&panel).with_value("").build();
    verify_code.set_min_size(Size::new(220, 30));
    verify_code.enable(false);
    verify_row.add(
        &verify_label,
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::Right,
        8,
    );
    verify_row.add(&verify_code, 0, SizerFlag::Right, 0);
    sizer.add_sizer(
        &verify_row,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let info = StaticText::builder(&panel)
        .with_label(&format!(
            "扫码完成后会自动关闭。二维码约 {} 秒后过期。",
            start.expires_in
        ))
        .build();
    info.set_foreground_color(Colour::rgb(88, 96, 108));
    info.wrap(600);
    sizer.add(
        &info,
        0,
        SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let close_button = Button::builder(&panel).with_label("关闭").build();
    buttons.add_stretch_spacer(1);
    buttons.add(&close_button, 1, SizerFlag::Expand, 0);
    sizer.add_sizer(
        &buttons,
        0,
        SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Bottom,
        18,
    );

    panel.set_sizer(sizer, true);
    let dialog_sizer = BoxSizer::builder(Orientation::Vertical).build();
    dialog_sizer.add(&panel, 1, SizerFlag::Expand, 0);
    dialog.set_sizer(dialog_sizer, true);
    dialog.center();

    let timer = Timer::new(&dialog);
    {
        let api = api.clone();
        let session_key = start.session_key.clone();
        let dialog = dialog;
        timer.on_tick(move |_| {
            let code = verify_code.get_value();
            let code = code.trim();
            match api.poll_wechat_onboard(&session_key, (!code.is_empty()).then_some(code)) {
                Ok(result) if result.done => {
                    dialog.end_modal(ID_OK);
                }
                Ok(result) => {
                    if result.need_verify_code.unwrap_or(false) {
                        verify_code.enable(true);
                    }
                    info.set_label(&wechat_onboard_status_text(&result));
                    info.wrap(600);
                }
                Err(_) => {
                    info.set_label("接入失败，请关闭后重试。");
                }
            }
        });
    }
    timer.start(1500, false);

    {
        let dialog = dialog;
        close_button.on_click(move |_| dialog.end_modal(ID_CANCEL));
    }

    dialog.show_modal();
    timer.stop();
    dialog.destroy();
}

fn wechat_onboard_status_text(result: &WechatOnboardPoll) -> String {
    if result.need_verify_code.unwrap_or(false) {
        return "微信需要验证码，请输入后等待自动确认。".to_string();
    }
    if let Some(error) = result.error.as_ref().and_then(|value| value.as_str()) {
        return match error {
            "expired" => "二维码已过期，请关闭后重新扫码。".to_string(),
            "verify_code_blocked" => "验证码被限制，请稍后重试。".to_string(),
            _ => format!("接入暂未完成：{error}"),
        };
    }
    match result.status.as_deref() {
        Some("wait") => "等待微信扫码。".to_string(),
        Some("scaned") => "已扫码，请在微信里确认。".to_string(),
        Some("scaned_but_redirect") => "已扫码，正在切换微信登录入口。".to_string(),
        Some("confirmed") => "已确认，正在保存配置。".to_string(),
        Some("binded_redirect") if result.already_connected.unwrap_or(false) => {
            "该微信已完成绑定。".to_string()
        }
        Some(status) => format!("当前状态：{status}"),
        None => "扫码完成后会自动关闭。".to_string(),
    }
}

fn is_feishu_onboard_pending(error: Option<&serde_json::Value>) -> bool {
    matches!(
        error.and_then(|value| value.as_str()),
        Some("authorization_pending" | "slow_down")
    )
}
