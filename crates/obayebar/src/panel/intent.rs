//! Holds a hover-open back until the pointer rests on its trigger.

use super::{PanelKind, TriggerSpot};

/// Everything `open_panel` needs to place a panel on its trigger.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRequest {
    pub kind: PanelKind,
    pub monitor: Option<String>,
    pub spot: TriggerSpot,
}

/// Identifies one hover, so the timer of an earlier one cannot open a panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

/// What a hover asks for.
#[derive(Debug, PartialEq)]
pub enum Hovered {
    /// Another panel is up: switching to this one must be instantaneous.
    OpenNow(OpenRequest),
    /// Wait for the open delay, then [`OpenIntent::settle`] this ticket.
    Wait(Ticket),
}

/// The hover waiting for the open delay to elapse, if any.
#[derive(Debug, Default)]
pub struct OpenIntent {
    issued: u64,
    pending: Option<(Ticket, OpenRequest)>,
}

impl OpenIntent {
    /// Record the pointer arriving on a trigger. Any earlier wait is dropped:
    /// the pointer can rest on one trigger at a time.
    pub fn hover(&mut self, request: OpenRequest, a_panel_is_open: bool) -> Hovered {
        if a_panel_is_open {
            self.cancel();
            return Hovered::OpenNow(request);
        }
        self.issued = self.issued.wrapping_add(1);
        let ticket = Ticket(self.issued);
        self.pending = Some((ticket, request));
        Hovered::Wait(ticket)
    }

    /// The pointer left `kind`'s trigger before its delay elapsed. A leave for
    /// a trigger other than the waiting one is stale and changes nothing.
    pub fn left(&mut self, kind: PanelKind) {
        if self.pending.as_ref().is_some_and(|(_, r)| r.kind == kind) {
            self.cancel();
        }
    }

    /// Drop the waiting hover, because a panel opened some other way.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// The open delay of `ticket` elapsed: the request to open, if its hover
    /// is still the one waiting.
    pub fn settle(&mut self, ticket: Ticket) -> Option<OpenRequest> {
        self.pending
            .take_if(|(waiting, _)| *waiting == ticket)
            .map(|(_, request)| request)
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use iced::Rectangle;

    use super::*;

    fn request(kind: PanelKind) -> OpenRequest {
        OpenRequest {
            kind,
            monitor: Some("DP-1".into()),
            spot: TriggerSpot::new(Rectangle::default(), Rectangle::default()),
        }
    }

    fn wait(intent: &mut OpenIntent, kind: PanelKind) -> Ticket {
        match intent.hover(request(kind), false) {
            Hovered::Wait(ticket) => ticket,
            Hovered::OpenNow(_) => panic!("a hover with no panel up must wait"),
        }
    }

    #[test]
    fn a_hover_while_a_panel_is_up_opens_now() {
        assert_eq!(
            OpenIntent::default().hover(request(PanelKind::Audio), true),
            Hovered::OpenNow(request(PanelKind::Audio))
        );
    }

    #[test]
    fn resting_on_the_trigger_opens_it() {
        let mut intent = OpenIntent::default();
        let ticket = wait(&mut intent, PanelKind::Audio);
        assert_eq!(intent.settle(ticket), Some(request(PanelKind::Audio)));
    }

    #[test]
    fn a_settled_hover_opens_only_once() {
        let mut intent = OpenIntent::default();
        let ticket = wait(&mut intent, PanelKind::Audio);
        intent.settle(ticket);
        assert_eq!(intent.settle(ticket), None);
    }

    #[test]
    fn crossing_the_trigger_opens_nothing() {
        let mut intent = OpenIntent::default();
        let ticket = wait(&mut intent, PanelKind::Audio);
        intent.left(PanelKind::Audio);
        assert_eq!(intent.settle(ticket), None);
    }

    #[test]
    fn a_stale_leave_keeps_the_newer_hover() {
        let mut intent = OpenIntent::default();
        wait(&mut intent, PanelKind::Audio);
        let ticket = wait(&mut intent, PanelKind::Network);
        intent.left(PanelKind::Audio);
        assert_eq!(intent.settle(ticket), Some(request(PanelKind::Network)));
    }

    #[test]
    fn an_earlier_hover_timer_opens_nothing() {
        let mut intent = OpenIntent::default();
        let first = wait(&mut intent, PanelKind::Audio);
        intent.left(PanelKind::Audio);
        let second = wait(&mut intent, PanelKind::Audio);
        assert_eq!(intent.settle(first), None);
        assert_eq!(intent.settle(second), Some(request(PanelKind::Audio)));
    }

    #[test]
    fn moving_to_another_trigger_drops_the_first() {
        let mut intent = OpenIntent::default();
        let first = wait(&mut intent, PanelKind::Audio);
        wait(&mut intent, PanelKind::Network);
        assert_eq!(intent.settle(first), None);
    }

    #[test]
    fn an_open_by_other_means_cancels_the_wait() {
        let mut intent = OpenIntent::default();
        let ticket = wait(&mut intent, PanelKind::Audio);
        intent.cancel();
        assert_eq!(intent.settle(ticket), None);
    }
}
