use crate::services::tray::TrayItemInfo;
use crate::style;
use crate::Message;
use iced::widget::{button, column, container, text, Space};
use iced::{Alignment, Element, Length};

pub fn view(items: &[TrayItemInfo]) -> Element<'_, Message> {
    if items.is_empty() {
        return Space::new().width(0.0).height(0.0).into();
    }

    let mut tray_col = column![]
        .spacing(style::SPACING_SMALL)
        .align_x(Alignment::Center);

    for item in items {
        let icon_text = if item.icon_name.is_empty() {
            item.title
                .chars()
                .next()
                .map_or_else(|| "?".to_string(), |c| c.to_string())
        } else {
            item.icon_name.clone()
        };

        let icon = text(icon_text)
            .size(style::FONT_SIZE_SMALL * 2.0)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center);

        let item_id = item.id.clone();
        let btn = button(
            container(icon)
                .width(style::FONT_SIZE_SMALL * 2.0)
                .height(style::FONT_SIZE_SMALL * 2.0)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press(Message::TrayClick(item_id))
        .style(style::transparent_button)
        .padding(0.0);

        tray_col = tray_col.push(btn);
    }

    container(tray_col)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
