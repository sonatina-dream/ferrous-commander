//! Rubber-band selection: which items a dragged rectangle touches in each list layout.
//!
//! Coordinates are list-content pixels: from the top-left corner of the list, with the
//! scroll offset added back, so a band keeps its anchor while the list scrolls.

/// A rectangle given by two corners in any order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub a: (f32, f32),
    pub b: (f32, f32),
}

impl Band {
    pub fn left(&self) -> f32 {
        self.a.0.min(self.b.0)
    }
    pub fn right(&self) -> f32 {
        self.a.0.max(self.b.0)
    }
    pub fn top(&self) -> f32 {
        self.a.1.min(self.b.1)
    }
    pub fn bottom(&self) -> f32 {
        self.a.1.max(self.b.1)
    }

    /// True once the pointer moved far enough to count as a drag rather than a click.
    pub fn is_drag(&self) -> bool {
        (self.a.0 - self.b.0).abs() > 3.0 || (self.a.1 - self.b.1).abs() > 3.0
    }

    fn overlaps(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        self.left() < x1 && self.right() > x0 && self.top() < y1 && self.bottom() > y0
    }
}

/// How a list lays out its items.
#[derive(Clone, Copy, Debug)]
pub enum Layout {
    /// One full-width row per item (Full mode).
    Rows { row_height: f32, count: usize },
    /// Rows of `cols` cells (Thumbs mode). Cells start `pad` px from the left edge and
    /// keep `gap` px of margin on each side.
    Grid { cell_width: f32, cell_height: f32, cols: usize, pad: f32, gap: f32, count: usize },
    /// Columns of `rows` cells, filled top to bottom (Brief mode); `first` is the
    /// leftmost visible column.
    Columns { col_width: f32, row_height: f32, rows: usize, first: usize, pad: f32, count: usize },
}

/// Indices of the items the band touches, in order.
pub fn hits(layout: Layout, band: Band) -> Vec<usize> {
    match layout {
        Layout::Rows { row_height, count } => {
            if row_height <= 0.0 || band.bottom() <= 0.0 || band.top() >= row_height * count as f32 {
                return Vec::new();
            }
            let first = (band.top().max(0.0) / row_height).floor() as usize;
            let last = ((band.bottom() / row_height).ceil() as usize).min(count);
            (first..last).collect()
        }
        Layout::Grid { cell_width, cell_height, cols, pad, gap, count } => {
            let cols = cols.max(1);
            (0..count)
                .filter(|ix| {
                    let (row, col) = (ix / cols, ix % cols);
                    let x0 = pad + col as f32 * cell_width + gap;
                    let y0 = row as f32 * cell_height + gap;
                    band.overlaps(x0, y0, x0 + cell_width - 2.0 * gap, y0 + cell_height - 2.0 * gap)
                })
                .collect()
        }
        Layout::Columns { col_width, row_height, rows, first, pad, count } => {
            let rows = rows.max(1);
            (first * rows..count)
                .filter(|ix| {
                    let (col, row) = (ix / rows - first, ix % rows);
                    let x0 = pad + col as f32 * col_width;
                    let y0 = row as f32 * row_height;
                    band.overlaps(x0, y0, x0 + col_width, y0 + row_height)
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(a: (f32, f32), b: (f32, f32)) -> Band {
        Band { a, b }
    }

    #[test]
    fn rows_touched_vertically() {
        let rows = Layout::Rows { row_height: 20.0, count: 10 };
        assert_eq!(hits(rows, band((5.0, 25.0), (300.0, 61.0))), [1, 2, 3]);
        assert_eq!(hits(rows, band((300.0, 61.0), (5.0, 25.0))), [1, 2, 3], "corner order does not matter");
        assert_eq!(hits(rows, band((0.0, -40.0), (10.0, 10.0))), [0], "dragging above the list");
        assert_eq!(hits(rows, band((0.0, 190.0), (10.0, 900.0))), [9], "dragging below the last row");
        assert!(hits(rows, band((0.0, 205.0), (10.0, 900.0))).is_empty(), "empty space under the rows");
    }

    #[test]
    fn grid_cells_need_overlap() {
        let grid = Layout::Grid { cell_width: 100.0, cell_height: 100.0, cols: 3, pad: 4.0, gap: 3.0, count: 7 };
        assert_eq!(hits(grid, band((50.0, 50.0), (160.0, 60.0))), [0, 1]);
        assert_eq!(hits(grid, band((50.0, 50.0), (60.0, 160.0))), [0, 3]);
        assert!(hits(grid, band((104.5, 10.0), (106.0, 90.0))).is_empty(), "the gap between two cells");
        assert_eq!(hits(grid, band((0.0, 0.0), (1000.0, 1000.0))), (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn brief_columns_scroll_horizontally() {
        let cols = Layout::Columns { col_width: 200.0, row_height: 20.0, rows: 5, first: 1, pad: 4.0, count: 14 };
        // The first visible column holds items 5–9.
        assert_eq!(hits(cols, band((10.0, 5.0), (20.0, 45.0))), [5, 6, 7]);
        assert_eq!(hits(cols, band((150.0, 5.0), (250.0, 15.0))), [5, 10], "across two columns");
        assert_eq!(hits(cols, band((150.0, 85.0), (250.0, 95.0))), [9], "the last column is shorter");
    }

    #[test]
    fn click_versus_drag() {
        assert!(!band((10.0, 10.0), (12.0, 11.0)).is_drag());
        assert!(band((10.0, 10.0), (10.0, 20.0)).is_drag());
    }
}
