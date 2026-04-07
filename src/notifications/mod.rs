pub mod center;
pub mod daemon;
pub mod popup;

use chrono::{DateTime, Local};

#[derive(Debug, Clone)]
pub struct NotificationData {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub time: DateTime<Local>,
    pub expire_at: Option<DateTime<Local>>,
    pub expanded: bool,
    pub urgency: Urgency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

#[derive(Debug, Clone)]
pub enum NotifEvent {
    Received(NotificationData),
    Closed(u32),
}

use crate::Message;
use iced::Element;

pub fn popup_view(app: &crate::App) -> Element<'_, Message> {
    popup::view(&app.popup_notifications)
}

pub fn center_view(app: &crate::App) -> Element<'_, Message> {
    center::view(&app.notifications)
}
