use iced::Rectangle;
use num_traits::ToPrimitive;

use obayebar::style;

/// Where a panel's trigger sits on its bar, in the bar surface's logical
/// pixels. The bar spans its output's full height, so this is also where the
/// trigger sits on the output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerSpot {
    centre_y: f32,
    output_height: f32,
}

impl TriggerSpot {
    /// Spot of a trigger laid out at `bounds` on a bar surface of `viewport`.
    pub fn new(bounds: Rectangle, viewport: Rectangle) -> Self {
        Self {
            centre_y: bounds.center_y(),
            output_height: viewport.height,
        }
    }

    /// Layer-shell margin placing a panel of content `height` beside the bar,
    /// centred on the trigger and kept `PANEL_GAP_PX` clear of the output's
    /// top and bottom edges. A panel taller than the room sticks to the top.
    pub(super) fn margin(self, height: u32) -> (i32, i32, i32, i32) {
        let height = f64::from(height);
        let gap = f64::from(style::PANEL_GAP_PX);
        let lowest = (f64::from(self.output_height) - height - gap).max(gap);
        let top = (f64::from(self.centre_y) - height / 2.0).clamp(gap, lowest);
        (
            top.round().to_i32().unwrap_or_default(),
            0,
            0,
            style::BAR_WIDTH.cast_signed(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: Rectangle = Rectangle {
        x: 0.0,
        y: 0.0,
        width: 54.0,
        height: 1080.0,
    };

    fn spot_at(centre_y: f32) -> TriggerSpot {
        TriggerSpot::new(
            Rectangle {
                x: 0.0,
                y: centre_y - 10.0,
                width: 54.0,
                height: 20.0,
            },
            OUTPUT,
        )
    }

    fn top(spot: TriggerSpot, height: u32) -> i32 {
        spot.margin(height).0
    }

    #[test]
    fn spot_is_the_trigger_centre_on_a_full_height_bar() {
        assert_eq!(
            spot_at(120.0),
            TriggerSpot {
                centre_y: 120.0,
                output_height: 1080.0,
            }
        );
    }

    #[test]
    fn panel_is_centred_on_its_trigger_when_there_is_room() {
        assert_eq!(
            spot_at(500.0).margin(200),
            (400, 0, 0, style::BAR_WIDTH.cast_signed())
        );
    }

    #[test]
    fn panel_near_the_top_keeps_the_gap_to_the_edge() {
        assert_eq!(top(spot_at(20.0), 200), 8);
    }

    #[test]
    fn panel_near_the_bottom_keeps_the_gap_to_the_edge() {
        assert_eq!(top(spot_at(1070.0), 200), 872);
    }

    #[test]
    fn panel_taller_than_the_output_sticks_to_the_top_gap() {
        assert_eq!(top(spot_at(500.0), 2000), 8);
    }
}
