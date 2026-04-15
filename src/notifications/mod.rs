mod popup;

use crate::Message;
use iced::Element;

pub fn popup_view(app: &crate::App) -> Element<'_, Message> {
    popup::view(&app.popup_notifications)
}
