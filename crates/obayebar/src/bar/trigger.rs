//! Bar entry that opens a panel centred on itself.

use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{layout, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{mouse, touch, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector};

use crate::panel::{PanelKind, TriggerSpot};
use crate::Message;

/// Wraps `content` so that hovering or pressing it opens `kind`'s panel
/// centred on it, and leaving it arms the grace close.
///
/// `mouse_area::on_enter` publishes a fixed message, blind to where the
/// widget was laid out, so it cannot tell the panel where to open. This
/// reads the trigger's bounds at the moment of the event instead.
///
/// A press the content captures itself (the audio icon launching
/// pavucontrol) stays the content's.
pub struct PanelTrigger<'a> {
    kind: PanelKind,
    monitor: Option<String>,
    content: Element<'a, Message>,
}

impl std::fmt::Debug for PanelTrigger<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanelTrigger")
            .field("kind", &self.kind)
            .field("monitor", &self.monitor)
            .finish_non_exhaustive()
    }
}

impl<'a> PanelTrigger<'a> {
    pub fn new(
        kind: PanelKind,
        monitor: Option<String>,
        content: impl Into<Element<'a, Message>>,
    ) -> Self {
        Self {
            kind,
            monitor,
            content: content.into(),
        }
    }

    fn open(&self, bounds: Rectangle, viewport: Rectangle) -> Message {
        Message::PanelOpen(
            self.kind,
            self.monitor.clone(),
            TriggerSpot::new(bounds, viewport),
        )
    }
}

#[derive(Default)]
struct State {
    hovered: bool,
}

/// What a trigger publishes in answer to one event.
#[derive(Debug, PartialEq, Eq)]
enum Reaction {
    Open,
    Leave,
}

impl State {
    /// Follow the pointer onto or off the trigger. `press` is an uncaptured
    /// left press or touch, which reopens the panel only while the pointer
    /// is over the trigger.
    const fn react(&mut self, hovered: bool, press: bool) -> Option<Reaction> {
        let was_hovered = std::mem::replace(&mut self.hovered, hovered);
        match (was_hovered, hovered) {
            (false, true) => Some(Reaction::Open),
            (true, true) if press => Some(Reaction::Open),
            (true, false) => Some(Reaction::Leave),
            _ => None,
        }
    }
}

const fn is_press(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            | Event::Touch(touch::Event::FingerPressed { .. })
    )
}

impl Widget<Message, Theme, Renderer> for PanelTrigger<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let [content] = tree.children.as_mut_slice() else {
            return layout::Node::new(Size::ZERO);
        };
        self.content
            .as_widget_mut()
            .layout(content, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if let [content] = tree.children.as_mut_slice() {
            self.content
                .as_widget_mut()
                .operate(content, layout, renderer, operation);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let [content] = tree.children.as_mut_slice() {
            self.content.as_widget_mut().update(
                content, event, layout, cursor, renderer, clipboard, shell, viewport,
            );
        }

        let bounds = layout.bounds();
        let press = is_press(event) && !shell.is_event_captured();
        match tree
            .state
            .downcast_mut::<State>()
            .react(cursor.is_over(bounds), press)
        {
            Some(Reaction::Open) => {
                shell.publish(self.open(bounds, *viewport));
                if press {
                    shell.capture_event();
                }
            }
            Some(Reaction::Leave) => shell.publish(Message::PanelPointerLeftTrigger(self.kind)),
            None => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let [content] = tree.children.as_slice() else {
            return mouse::Interaction::None;
        };
        self.content
            .as_widget()
            .mouse_interaction(content, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let [content] = tree.children.as_slice() {
            self.content
                .as_widget()
                .draw(content, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let [content] = tree.children.as_mut_slice() else {
            return None;
        };
        self.content
            .as_widget_mut()
            .overlay(content, layout, renderer, viewport, translation)
    }
}

impl<'a> From<PanelTrigger<'a>> for Element<'a, Message> {
    fn from(trigger: PanelTrigger<'a>) -> Self {
        Element::new(trigger)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hovering() -> State {
        State { hovered: true }
    }

    #[test]
    fn entering_opens() {
        assert_eq!(State::default().react(true, false), Some(Reaction::Open));
    }

    #[test]
    fn moving_within_does_nothing() {
        assert_eq!(hovering().react(true, false), None);
    }

    #[test]
    fn leaving_arms_the_grace_close() {
        assert_eq!(hovering().react(false, false), Some(Reaction::Leave));
    }

    #[test]
    fn a_press_reopens() {
        assert_eq!(hovering().react(true, true), Some(Reaction::Open));
    }

    #[test]
    fn staying_away_does_nothing() {
        assert_eq!(State::default().react(false, false), None);
    }

    #[test]
    fn hover_is_remembered() {
        let mut state = State::default();
        state.react(true, false);
        assert_eq!(state.react(false, false), Some(Reaction::Leave));
    }
}
