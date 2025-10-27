use std::collections::HashMap;

/// Direction an item wants to navigate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Which edge of the text field the cursor was at when navigation was requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    Top,
    Bottom,
    Left,
    Right,
}

/// Cursor state when leaving a block. Frontends map their framework-specific
/// cursor representation to this before delegating to the navigator.
#[derive(Debug, Clone, Copy)]
pub struct CursorHint {
    pub column: usize,
    pub boundary: Boundary,
}

/// Where to place the cursor in the target block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPlacement {
    FirstLine { column: usize },
    LastLine { column: usize },
    Start,
    End,
}

/// What the collection tells the frontend to focus next.
#[derive(Debug, Clone)]
pub struct NavTarget {
    pub block_id: String,
    pub placement: CursorPlacement,
}

/// Trait that each collection type implements.
/// All implementations live in holon-frontend — frontends only call `navigate()`.
pub trait CollectionNavigator: Send + Sync {
    fn navigate(
        &self,
        current_id: &str,
        direction: NavDirection,
        hint: &CursorHint,
    ) -> Option<NavTarget>;
}

// ---------------------------------------------------------------------------
// ListNavigator
// ---------------------------------------------------------------------------

/// Linear Up/Down navigation over an ordered list of block IDs.
pub struct ListNavigator {
    ordered_ids: Vec<String>,
    index: HashMap<String, usize>,
}

impl ListNavigator {
    pub fn new(ordered_ids: Vec<String>) -> Self {
        let index = ordered_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        Self { ordered_ids, index }
    }
}

impl CollectionNavigator for ListNavigator {
    fn navigate(
        &self,
        current_id: &str,
        direction: NavDirection,
        hint: &CursorHint,
    ) -> Option<NavTarget> {
        let &idx = self.index.get(current_id)?;
        let target_idx = match direction {
            NavDirection::Up => idx.checked_sub(1)?,
            NavDirection::Down => {
                let next = idx + 1;
                if next >= self.ordered_ids.len() {
                    return None;
                }
                next
            }
            NavDirection::Left | NavDirection::Right => return None,
        };
        let placement = match direction {
            NavDirection::Up => CursorPlacement::LastLine {
                column: hint.column,
            },
            _ => CursorPlacement::FirstLine {
                column: hint.column,
            },
        };
        Some(NavTarget {
            block_id: self.ordered_ids[target_idx].clone(),
            placement,
        })
    }
}

// ---------------------------------------------------------------------------
// TreeNavigator
// ---------------------------------------------------------------------------

/// Tree navigation: Up/Down follow DFS order, Left goes to parent,
/// Right goes to first child.
pub struct TreeNavigator {
    dfs_order: Vec<String>,
    dfs_index: HashMap<String, usize>,
    parent_of: HashMap<String, String>,
    first_child_of: HashMap<String, String>,
}

impl TreeNavigator {
    /// Build from a pre-computed DFS order and a child-of-parent map.
    ///
    /// `parent_map` maps child_id → parent_id. Pass the same parent_id column
    /// used by `OutlineTree` / `shared_tree_build`.
    pub fn from_dfs_and_parents(
        dfs_order: Vec<String>,
        parent_map: HashMap<String, String>,
    ) -> Self {
        let dfs_index = dfs_order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();

        // Derive first_child_of by walking DFS order: for each node, if its
        // parent doesn't already have a first-child entry, this node is it.
        let mut first_child_of: HashMap<String, String> = HashMap::new();
        for id in &dfs_order {
            if let Some(parent) = parent_map.get(id) {
                first_child_of.entry(parent.clone()).or_insert(id.clone());
            }
        }

        Self {
            dfs_order,
            dfs_index,
            parent_of: parent_map,
            first_child_of,
        }
    }

    /// Convenience: build from rows with `id`, `parent_id` columns,
    /// using the same DFS walk as `shared_tree_build`.
    pub fn from_rows(
        rows: &[HashMap<String, holon_api::Value>],
        id_col: &str,
        parent_id_col: &str,
        sort_col: &str,
    ) -> Self {
        use holon_api::render_eval::OutlineTree;

        let tree = OutlineTree::from_rows(rows, parent_id_col, sort_col);
        let mut dfs_order = Vec::new();
        let mut parent_map = HashMap::new();

        tree.walk_depth_first(|row, _depth| {
            if let Some(id) = row.get(id_col).and_then(|v| v.as_string()) {
                let id = id.to_string();
                dfs_order.push(id.clone());
                if let Some(pid) = row.get(parent_id_col).and_then(|v| v.as_string()) {
                    parent_map.insert(id, pid.to_string());
                }
            }
        });

        Self::from_dfs_and_parents(dfs_order, parent_map)
    }
}

impl CollectionNavigator for TreeNavigator {
    fn navigate(
        &self,
        current_id: &str,
        direction: NavDirection,
        hint: &CursorHint,
    ) -> Option<NavTarget> {
        match direction {
            NavDirection::Up | NavDirection::Down => {
                let &idx = self.dfs_index.get(current_id)?;
                let target_idx = match direction {
                    NavDirection::Up => idx.checked_sub(1)?,
                    _ => {
                        let next = idx + 1;
                        if next >= self.dfs_order.len() {
                            return None;
                        }
                        next
                    }
                };
                let placement = match direction {
                    NavDirection::Up => CursorPlacement::LastLine {
                        column: hint.column,
                    },
                    _ => CursorPlacement::FirstLine {
                        column: hint.column,
                    },
                };
                Some(NavTarget {
                    block_id: self.dfs_order[target_idx].clone(),
                    placement,
                })
            }
            NavDirection::Left => {
                let parent = self.parent_of.get(current_id)?;
                Some(NavTarget {
                    block_id: parent.clone(),
                    placement: CursorPlacement::End,
                })
            }
            NavDirection::Right => {
                let child = self.first_child_of.get(current_id)?;
                Some(NavTarget {
                    block_id: child.clone(),
                    placement: CursorPlacement::Start,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// TableNavigator
// ---------------------------------------------------------------------------

/// 2D grid navigation: Up/Down move between rows in the same column,
/// Left/Right move between columns in the same row.
pub struct TableNavigator {
    cells: HashMap<(usize, usize), String>,
    cell_positions: HashMap<String, (usize, usize)>,
    row_count: usize,
    col_count: usize,
}

impl TableNavigator {
    /// Build from a grid of cell IDs. `rows[r][c]` is the block ID at row r, column c.
    /// Cells that are `None` are skipped (not navigable).
    pub fn from_grid(rows: Vec<Vec<Option<String>>>) -> Self {
        let row_count = rows.len();
        let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut cells = HashMap::new();
        let mut cell_positions = HashMap::new();

        for (r, row) in rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if let Some(id) = cell {
                    cells.insert((r, c), id.clone());
                    cell_positions.insert(id.clone(), (r, c));
                }
            }
        }

        Self {
            cells,
            cell_positions,
            row_count,
            col_count,
        }
    }

    /// Build from a flat list of row IDs (single-column table, behaves like a list
    /// but allows extension to multi-column later).
    pub fn from_row_ids(row_ids: Vec<String>) -> Self {
        Self::from_grid(row_ids.into_iter().map(|id| vec![Some(id)]).collect())
    }
}

impl CollectionNavigator for TableNavigator {
    fn navigate(
        &self,
        current_id: &str,
        direction: NavDirection,
        hint: &CursorHint,
    ) -> Option<NavTarget> {
        let &(row, col) = self.cell_positions.get(current_id)?;
        let (target_row, target_col) = match direction {
            NavDirection::Up => (row.checked_sub(1)?, col),
            NavDirection::Down => {
                let next = row + 1;
                if next >= self.row_count {
                    return None;
                }
                (next, col)
            }
            NavDirection::Left => (row, col.checked_sub(1)?),
            NavDirection::Right => {
                let next = col + 1;
                if next >= self.col_count {
                    return None;
                }
                (row, next)
            }
        };
        let target = self.cells.get(&(target_row, target_col))?;
        let placement = match direction {
            NavDirection::Up => CursorPlacement::LastLine {
                column: hint.column,
            },
            NavDirection::Down => CursorPlacement::FirstLine {
                column: hint.column,
            },
            NavDirection::Left => CursorPlacement::End,
            NavDirection::Right => CursorPlacement::Start,
        };
        Some(NavTarget {
            block_id: target.clone(),
            placement,
        })
    }
}

// ---------------------------------------------------------------------------
// Cursor line/column helpers
// ---------------------------------------------------------------------------

/// Compute which line and column a character offset falls on.
pub fn offset_to_line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Check if a character offset is on the first line.
pub fn is_on_first_line(text: &str, offset: usize) -> bool {
    text[..offset.min(text.len())].find('\n').is_none()
}

/// Check if a character offset is on the last line.
pub fn is_on_last_line(text: &str, offset: usize) -> bool {
    text[offset.min(text.len())..].find('\n').is_none()
}

/// Compute the character offset for a given line and column,
/// clamping the column to the line's length.
pub fn line_col_to_offset(text: &str, target_line: usize, target_column: usize) -> usize {
    let mut offset = 0;
    for (i, line) in text.split('\n').enumerate() {
        if i == target_line {
            return offset + target_column.min(line.len());
        }
        offset += line.len() + 1; // +1 for the newline
    }
    // target_line beyond last line — clamp to end
    text.len()
}

/// Count of lines in text.
pub fn line_count(text: &str) -> usize {
    if text.is_empty() {
        1
    } else {
        text.split('\n').count()
    }
}

/// Apply a `CursorPlacement` to a text string, returning a character offset.
pub fn placement_to_offset(text: &str, placement: CursorPlacement) -> usize {
    match placement {
        CursorPlacement::Start => 0,
        CursorPlacement::End => text.len(),
        CursorPlacement::FirstLine { column } => line_col_to_offset(text, 0, column),
        CursorPlacement::LastLine { column } => {
            let last = line_count(text).saturating_sub(1);
            line_col_to_offset(text, last, column)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- cursor helpers --

    #[test]
    fn offset_to_line_col_single_line() {
        assert_eq!(offset_to_line_col("hello", 3), (0, 3));
    }

    #[test]
    fn offset_to_line_col_multi_line() {
        assert_eq!(offset_to_line_col("ab\ncd\nef", 5), (1, 2));
        assert_eq!(offset_to_line_col("ab\ncd\nef", 6), (2, 0));
    }

    #[test]
    fn is_first_last_line() {
        let text = "line1\nline2\nline3";
        assert!(is_on_first_line(text, 3));
        assert!(!is_on_first_line(text, 7));
        assert!(!is_on_last_line(text, 3));
        assert!(is_on_last_line(text, 14));
    }

    #[test]
    fn line_col_to_offset_basic() {
        let text = "abc\ndef\nghi";
        assert_eq!(line_col_to_offset(text, 0, 2), 2);
        assert_eq!(line_col_to_offset(text, 1, 1), 5);
        assert_eq!(line_col_to_offset(text, 2, 3), 11); // clamped to line len
    }

    #[test]
    fn placement_to_offset_all_variants() {
        let text = "abc\ndef\nghi";
        assert_eq!(placement_to_offset(text, CursorPlacement::Start), 0);
        assert_eq!(placement_to_offset(text, CursorPlacement::End), 11);
        assert_eq!(
            placement_to_offset(text, CursorPlacement::FirstLine { column: 2 }),
            2
        );
        assert_eq!(
            placement_to_offset(text, CursorPlacement::LastLine { column: 1 }),
            9
        );
    }

    // -- ListNavigator --

    #[test]
    fn list_nav_down_up() {
        let nav = ListNavigator::new(vec!["a".into(), "b".into(), "c".into()]);
        let hint = CursorHint {
            column: 5,
            boundary: Boundary::Bottom,
        };

        let target = nav.navigate("a", NavDirection::Down, &hint).unwrap();
        assert_eq!(target.block_id, "b");
        assert_eq!(target.placement, CursorPlacement::FirstLine { column: 5 });

        let target = nav.navigate("b", NavDirection::Up, &hint).unwrap();
        assert_eq!(target.block_id, "a");
        assert_eq!(target.placement, CursorPlacement::LastLine { column: 5 });
    }

    #[test]
    fn list_nav_at_boundary() {
        let nav = ListNavigator::new(vec!["a".into(), "b".into()]);
        let hint = CursorHint {
            column: 0,
            boundary: Boundary::Top,
        };
        assert!(nav.navigate("a", NavDirection::Up, &hint).is_none());
        assert!(nav.navigate("b", NavDirection::Down, &hint).is_none());
    }

    #[test]
    fn list_nav_left_right_returns_none() {
        let nav = ListNavigator::new(vec!["a".into(), "b".into()]);
        let hint = CursorHint {
            column: 0,
            boundary: Boundary::Right,
        };
        assert!(nav.navigate("a", NavDirection::Left, &hint).is_none());
        assert!(nav.navigate("a", NavDirection::Right, &hint).is_none());
    }

    // -- TreeNavigator --

    #[test]
    fn tree_nav_dfs_up_down() {
        //   root
        //   ├── a
        //   │   └── a1
        //   └── b
        let dfs = vec!["root".into(), "a".into(), "a1".into(), "b".into()];
        let parents: HashMap<String, String> = [
            ("a".into(), "root".into()),
            ("a1".into(), "a".into()),
            ("b".into(), "root".into()),
        ]
        .into();
        let nav = TreeNavigator::from_dfs_and_parents(dfs, parents);
        let hint = CursorHint {
            column: 0,
            boundary: Boundary::Bottom,
        };

        // Down from a → a1 (DFS order)
        let t = nav.navigate("a", NavDirection::Down, &hint).unwrap();
        assert_eq!(t.block_id, "a1");

        // Down from a1 → b
        let t = nav.navigate("a1", NavDirection::Down, &hint).unwrap();
        assert_eq!(t.block_id, "b");

        // Up from b → a1
        let t = nav.navigate("b", NavDirection::Up, &hint).unwrap();
        assert_eq!(t.block_id, "a1");
    }

    #[test]
    fn tree_nav_left_right() {
        let dfs = vec!["root".into(), "child".into()];
        let parents: HashMap<String, String> = [("child".into(), "root".into())].into();
        let nav = TreeNavigator::from_dfs_and_parents(dfs, parents);
        let hint = CursorHint {
            column: 0,
            boundary: Boundary::Left,
        };

        // Right from root → first child
        let t = nav.navigate("root", NavDirection::Right, &hint).unwrap();
        assert_eq!(t.block_id, "child");
        assert_eq!(t.placement, CursorPlacement::Start);

        // Left from child → parent
        let t = nav.navigate("child", NavDirection::Left, &hint).unwrap();
        assert_eq!(t.block_id, "root");
        assert_eq!(t.placement, CursorPlacement::End);

        // Left from root → None (no parent)
        assert!(nav.navigate("root", NavDirection::Left, &hint).is_none());
    }

    // -- TableNavigator --

    #[test]
    fn table_nav_2d() {
        // 2x3 grid:
        //   a0  a1  a2
        //   b0  b1  b2
        let nav = TableNavigator::from_grid(vec![
            vec![Some("a0".into()), Some("a1".into()), Some("a2".into())],
            vec![Some("b0".into()), Some("b1".into()), Some("b2".into())],
        ]);
        let hint = CursorHint {
            column: 3,
            boundary: Boundary::Bottom,
        };

        // Down from a1 → b1 (same column)
        let t = nav.navigate("a1", NavDirection::Down, &hint).unwrap();
        assert_eq!(t.block_id, "b1");

        // Right from a1 → a2
        let t = nav.navigate("a1", NavDirection::Right, &hint).unwrap();
        assert_eq!(t.block_id, "a2");
        assert_eq!(t.placement, CursorPlacement::Start);

        // Left from a1 → a0
        let t = nav.navigate("a1", NavDirection::Left, &hint).unwrap();
        assert_eq!(t.block_id, "a0");
        assert_eq!(t.placement, CursorPlacement::End);

        // Boundary: right from a2 → None
        assert!(nav.navigate("a2", NavDirection::Right, &hint).is_none());
        // Boundary: down from b1 → None
        assert!(nav.navigate("b1", NavDirection::Down, &hint).is_none());
    }

    #[test]
    fn table_nav_sparse_grid() {
        // Sparse: cell (0,1) is None
        let nav = TableNavigator::from_grid(vec![
            vec![Some("a0".into()), None, Some("a2".into())],
            vec![Some("b0".into()), Some("b1".into()), Some("b2".into())],
        ]);
        let hint = CursorHint {
            column: 0,
            boundary: Boundary::Bottom,
        };

        // Down from missing cell position doesn't crash
        // Right from a0 → skips None, returns None (no cell at (0,1))
        assert!(nav.navigate("a0", NavDirection::Right, &hint).is_none());
    }
}
