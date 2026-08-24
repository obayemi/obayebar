mod popup;

use crate::Message;
use iced::Element;

pub fn popup_view(app: &crate::App) -> Element<'_, Message> {
    let (visible, overflow) = app.popup_fit();
    popup::view(
        &app.popup_notifications,
        app.hovered_notif_id,
        visible,
        overflow,
    )
}
