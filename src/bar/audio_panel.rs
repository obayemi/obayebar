use crate::services::audio::AudioInfo;
use crate::style;
use crate::Message;
use iced::widget::{button, column, container, mouse_area, row, slider, text, Space};
use iced::{Alignment, Border, Element, Length};

fn separator<'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1.0))
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(style::with_alpha(
                style::M3_OUTLINE_VARIANT,
                0.5,
            ))),
            ..container::Style::default()
        })
        .into()
}

fn sink_entry(description: &str, sink_id: u32, is_selected: bool) -> Element<'_, Message> {
    let (bg, text_color) = if is_selected {
        (
            style::with_alpha(style::M3_PRIMARY, 0.15),
            style::M3_PRIMARY,
        )
    } else {
        (iced::Color::TRANSPARENT, style::M3_ON_SURFACE)
    };

    button(
        text(description)
            .size(style::FONT_SIZE_NORMAL)
            .color(text_color)
            .width(Length::Fill),
    )
    .on_press(Message::AudioSetDefaultSink(sink_id))
    .style(move |_theme, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let bg_color = if hover {
            style::with_alpha(style::M3_ON_SURFACE, 0.08)
        } else {
            bg
        };
        button::Style {
            background: Some(iced::Background::Color(bg_color)),
            text_color,
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            shadow: iced::Shadow::default(),
            snap: false,
        }
    })
    .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
    .width(Length::Fill)
    .into()
}

fn volume_section(audio: &AudioInfo) -> Element<'_, Message> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let volume_pct = (audio.volume * 100.0).round() as u32;
    let volume_text: Element<'_, Message> = if audio.muted {
        text("Volume (Muted)")
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)
            .into()
    } else {
        text(format!("Volume ({volume_pct}%)"))
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)
            .into()
    };

    let mute_icon = if audio.muted {
        style::ICON_VOLUME_OFF
    } else {
        style::ICON_VOLUME_UP
    };

    let mute_btn = button(
        text(mute_icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(if audio.muted {
                style::M3_ERROR
            } else {
                style::M3_ON_SURFACE
            })
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(Message::AudioSetMute(!audio.muted))
    .style(style::transparent_button)
    .padding(style::PADDING_SMALL);

    let volume_row_height =
        Length::Fixed(style::PADDING_SMALL.mul_add(2.0, style::FONT_SIZE_LARGE));

    let volume_slider = container(
        slider(0.0..=1.0, audio.volume, Message::AudioSetVolume)
            .step(0.01)
            .width(Length::Fill),
    )
    .height(volume_row_height)
    .align_y(Alignment::Center);

    column![
        volume_text,
        row![volume_slider, mute_btn]
            .spacing(style::SPACING_SMALLER)
            .align_y(Alignment::Center)
            .height(volume_row_height),
    ]
    .spacing(style::SPACING_SMALL)
    .into()
}

pub fn view(audio: &AudioInfo) -> Element<'_, Message> {
    let header = row![
        text(audio.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY),
        text("Audio")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    // Output device selection
    let mut sink_list = column![text("Output device")
        .size(style::FONT_SIZE_SMALLER)
        .color(style::M3_ON_SURFACE_VARIANT)]
    .spacing(2.0)
    .width(Length::Fill);

    let selected_name = audio.default_sink_name.as_deref();

    for sink in &audio.sinks {
        let is_selected = selected_name == Some(sink.name.as_str());
        sink_list = sink_list.push(sink_entry(&sink.description, sink.id, is_selected));
    }

    if audio.sinks.is_empty() {
        sink_list = sink_list.push(
            text("No output devices found")
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    }

    let content = column![header, sink_list, separator(), volume_section(audio),]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    let panel = container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(style::audio_panel_container);

    mouse_area(
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::End)
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: style::PANEL_GAP,
                left: style::PANEL_GAP,
            })
            .style(style::panel_wrapper_container),
    )
    .on_exit(Message::CloseAllPanels)
    .into()
}
