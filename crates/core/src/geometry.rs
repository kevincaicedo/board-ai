//! Board geometry in world units (one unit is one CSS pixel at zoom 1). Integers keep layout
//! deterministic and the exported JSON exact.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// An axis-aligned rectangle; `w` and `h` are never negative.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    #[must_use]
    pub fn right(self) -> i32 {
        self.x.saturating_add_unsigned(self.w)
    }

    #[must_use]
    pub fn bottom(self) -> i32 {
        self.y.saturating_add_unsigned(self.h)
    }

    #[must_use]
    pub fn center(self) -> Point {
        Point {
            x: self.x.saturating_add_unsigned(self.w / 2),
            y: self.y.saturating_add_unsigned(self.h / 2),
        }
    }

    /// True when the rectangles share interior area; touching edges do not intersect.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x,
            y,
            w: right.abs_diff(x),
            h: bottom.abs_diff(y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Point, Rect};

    #[test]
    fn rect_edges_center_and_intersection() {
        let a = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 40,
        };
        let touching = Rect {
            x: 100,
            y: 0,
            w: 10,
            h: 10,
        };
        let overlapping = Rect {
            x: 99,
            y: 39,
            w: 10,
            h: 10,
        };
        assert_eq!(a.center(), Point { x: 50, y: 20 });
        assert!(!a.intersects(touching));
        assert!(a.intersects(overlapping));
        assert_eq!(
            a.union(overlapping),
            Rect {
                x: 0,
                y: 0,
                w: 109,
                h: 49
            }
        );
    }
}
