//! Table topology derived from generated CSS boxes.
//!
//! Buckram consumes only box identities and typed span inputs. HTML attribute
//! parsing belongs to the integrating adapter, so this model remains reusable
//! for CSS-display tables and non-HTML document languages.

use std::hash::Hash;

use crate::{BoxId, CssBoxTree, InternalTableRole};

mod automatic;
mod automatic_used;
mod fixed;
mod sizing;

pub use automatic::{
    TableAutomaticColumnGroupInput, TableAutomaticColumnInput, TableAutomaticColumnMeasureInput,
    TableAutomaticColumnMeasures, TableColumnMeasure, TableSpanMeasureDistribution,
    measure_automatic_columns,
};
pub use automatic_used::{
    TableAutomaticInlineSizingIndefinite, TableAutomaticInlineSizingInput,
    TableAutomaticInlineSizingOutcome, cache_automatic_table_grid_intrinsic_sizes,
    size_automatic_table_inline,
};
pub use fixed::{
    TableFixedColumnGroupInput, TableFixedColumnInput, TableFixedInlineSizingInput,
    TableFixedInlineSizingOutcome, TableFixedLayoutFallback, size_fixed_table_inline,
};
pub use sizing::{
    AffineLengthPercentage, CaptionMinContribution, CellInlineOffsets, InlineSizeConstraint,
    TableBoxSizing, TableCellInlineMeasure, TableCellInlineStyle, TableDeferral,
    TableInlineBorderMetrics, TableInlineConstraints, TableInlineProperty, TableInlineSizingError,
    TableInlineSizingInput, TableInlineSizingResult, TableIntrinsicMeasureProvider,
    TableSeparatedBorderMetrics, TableTrackVisibility, TableTrackVisibilityState,
    collect_table_cell_inline_measures, query_table_cell_inline_sizes,
};

/// The table topology needed by the temporary layout bridge and later table
/// sizing and fragment algorithms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableGrid {
    /// The anonymous outer box when this is a table root.
    pub wrapper: Option<BoxId>,
    /// The principal or anonymous table-grid box.
    pub grid: BoxId,
    /// Row groups in visual table order: header, body, then footer.
    pub row_groups: Vec<TableTrackGroup>,
    /// Column groups in source order.
    pub column_groups: Vec<TableTrackGroup>,
    /// Explicit and implicitly-created row tracks.
    pub rows: Vec<TableTrack>,
    /// Explicit and implicitly-created column tracks.
    pub columns: Vec<TableTrack>,
    /// Cells with their complete table-model placement.
    pub cells: Vec<TableCell>,
    /// Caption boxes, retained separately from the grid's topology.
    pub captions: Vec<BoxId>,
    /// One entry for every row/column slot after placement.
    pub slots: Vec<TableSlot>,
    /// Malformed model inputs retained as deterministic diagnostics.
    pub errors: Vec<TableGridError>,
}

/// A row or column track. Implicit columns deliberately have no CSS box.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableTrack {
    pub source: Option<BoxId>,
    pub index: usize,
    pub group: Option<usize>,
}

/// A contiguous run of row or column tracks with its source box.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableTrackGroup {
    pub source: BoxId,
    pub kind: TableTrackGroupKind,
    pub start: usize,
    pub span: usize,
}

/// The table role carried by a track group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableTrackGroupKind {
    Header,
    Body,
    Footer,
    Column,
}

/// A cell and its slot rectangle. Spans are table-model data, not Taffy grid
/// placements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableCell {
    pub source: BoxId,
    pub row: usize,
    pub row_span: usize,
    pub column: usize,
    pub column_span: usize,
}

/// Occupancy of one rectangular table slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableSlot {
    pub row: usize,
    pub column: usize,
    pub cell: Option<usize>,
}

/// The adapter-normalized row span. `ToEndOfGroup` is HTML's `rowspan="0"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableRowSpan {
    Count(usize),
    ToEndOfGroup,
}

/// Typed span input for one generated table-cell box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableCellInput {
    pub column_span: usize,
    pub row_span: TableRowSpan,
}

impl Default for TableCellInput {
    fn default() -> Self {
        Self {
            column_span: 1,
            row_span: TableRowSpan::Count(1),
        }
    }
}

/// Typed span input for a generated column or column-group box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableTrackInput {
    pub span: usize,
}

impl Default for TableTrackInput {
    fn default() -> Self {
        Self { span: 1 }
    }
}

/// Document-language inputs normalized at the Buckram boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableGridInputs {
    cell_inputs: Vec<(BoxId, TableCellInput)>,
    column_inputs: Vec<(BoxId, TableTrackInput)>,
    column_group_inputs: Vec<(BoxId, TableTrackInput)>,
}

impl TableGridInputs {
    pub fn set_cell(&mut self, source: BoxId, input: TableCellInput) {
        self.cell_inputs.push((source, input));
    }

    pub fn set_column(&mut self, source: BoxId, input: TableTrackInput) {
        self.column_inputs.push((source, input));
    }

    pub fn set_column_group(&mut self, source: BoxId, input: TableTrackInput) {
        self.column_group_inputs.push((source, input));
    }

    fn cell(&self, source: BoxId, errors: &mut Vec<TableGridError>) -> TableCellInput {
        lookup_input(&self.cell_inputs, source, errors, TableCellInput::default())
    }

    fn column(&self, source: BoxId, errors: &mut Vec<TableGridError>) -> TableTrackInput {
        lookup_input(
            &self.column_inputs,
            source,
            errors,
            TableTrackInput::default(),
        )
    }

    fn column_group(&self, source: BoxId, errors: &mut Vec<TableGridError>) -> TableTrackInput {
        lookup_input(
            &self.column_group_inputs,
            source,
            errors,
            TableTrackInput::default(),
        )
    }
}

/// Input errors are explicit so malformed documents do not make placement
/// nondeterministic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TableGridError {
    NotAGrid {
        source: BoxId,
    },
    InvalidSpan {
        source: BoxId,
    },
    OverlappingCellInput {
        source: BoxId,
    },
    SlotOverlap {
        row: usize,
        column: usize,
        existing: usize,
        incoming: usize,
    },
}

#[derive(Clone, Copy)]
struct PendingCell {
    source: BoxId,
    row: usize,
    group: usize,
}

impl TableGrid {
    /// Build table topology from the K4a-normalized CSS box tree.
    ///
    /// Header and footer groups are ordered as CSS table tracks require, but
    /// their source boxes remain unchanged. No DOM data is read here.
    pub fn from_box_tree<Id>(boxes: &CssBoxTree<Id>, grid: BoxId, inputs: &TableGridInputs) -> Self
    where
        Id: Copy + Eq + Hash,
    {
        let mut table = Self {
            wrapper: boxes[grid].parent().filter(|parent| {
                boxes[*parent].display.internal_table == Some(InternalTableRole::Wrapper)
            }),
            grid,
            row_groups: Vec::new(),
            column_groups: Vec::new(),
            rows: Vec::new(),
            columns: Vec::new(),
            cells: Vec::new(),
            captions: Vec::new(),
            slots: Vec::new(),
            errors: Vec::new(),
        };

        if boxes[grid].display.internal_table != Some(InternalTableRole::Grid) {
            table.errors.push(TableGridError::NotAGrid { source: grid });
            return table;
        }

        if let Some(wrapper) = table.wrapper {
            table
                .captions
                .extend(boxes[wrapper].children().iter().copied().filter(|child| {
                    boxes[*child].display.internal_table == Some(InternalTableRole::Caption)
                }));
        }

        let mut headers = Vec::new();
        let mut bodies = Vec::new();
        let mut footers = Vec::new();
        for child in boxes[grid].children().iter().copied() {
            match boxes[child].display.internal_table {
                Some(InternalTableRole::Caption) => table.captions.push(child),
                Some(InternalTableRole::ColumnGroup) => {
                    table.add_column_group(boxes, child, inputs);
                },
                Some(InternalTableRole::Column) => {
                    let span = inputs.column(child, &mut table.errors).span;
                    table.add_columns(child, None, span);
                },
                Some(InternalTableRole::HeaderGroup) => {
                    headers.push((child, TableTrackGroupKind::Header))
                },
                Some(InternalTableRole::RowGroup) => {
                    bodies.push((child, TableTrackGroupKind::Body))
                },
                Some(InternalTableRole::FooterGroup) => {
                    footers.push((child, TableTrackGroupKind::Footer))
                },
                Some(InternalTableRole::Row) => bodies.push((child, TableTrackGroupKind::Body)),
                _ => {},
            }
        }

        let mut pending = Vec::new();
        for (group, kind) in headers.into_iter().chain(bodies).chain(footers) {
            table.add_row_group(boxes, group, kind, &mut pending);
        }
        table.place_cells(inputs, pending);
        table
    }

    fn add_column_group<Id>(
        &mut self,
        boxes: &CssBoxTree<Id>,
        group: BoxId,
        inputs: &TableGridInputs,
    ) where
        Id: Copy + Eq + Hash,
    {
        let index = self.column_groups.len();
        let start = self.columns.len();
        let columns = boxes[group]
            .children()
            .iter()
            .copied()
            .filter(|child| boxes[*child].display.internal_table == Some(InternalTableRole::Column))
            .collect::<Vec<_>>();
        if columns.is_empty() {
            let span = inputs.column_group(group, &mut self.errors).span;
            self.add_columns(group, Some(index), span);
        } else {
            for column in columns {
                let span = inputs.column(column, &mut self.errors).span;
                self.add_columns(column, Some(index), span);
            }
        }
        self.column_groups.push(TableTrackGroup {
            source: group,
            kind: TableTrackGroupKind::Column,
            start,
            span: self.columns.len() - start,
        });
    }

    fn add_columns(&mut self, source: BoxId, group: Option<usize>, span: usize) {
        let span = self.valid_span(source, span);
        for _ in 0..span {
            let index = self.columns.len();
            self.columns.push(TableTrack {
                source: Some(source),
                index,
                group,
            });
        }
    }

    fn add_row_group<Id>(
        &mut self,
        boxes: &CssBoxTree<Id>,
        group: BoxId,
        kind: TableTrackGroupKind,
        pending: &mut Vec<PendingCell>,
    ) where
        Id: Copy + Eq + Hash,
    {
        let group_index = self.row_groups.len();
        let start = self.rows.len();
        let rows = if boxes[group].display.internal_table == Some(InternalTableRole::Row) {
            vec![group]
        } else {
            boxes[group]
                .children()
                .iter()
                .copied()
                .filter(|child| {
                    boxes[*child].display.internal_table == Some(InternalTableRole::Row)
                })
                .collect()
        };
        for row in rows {
            let index = self.rows.len();
            self.rows.push(TableTrack {
                source: Some(row),
                index,
                group: Some(group_index),
            });
            pending.extend(
                boxes[row]
                    .children()
                    .iter()
                    .copied()
                    .filter(|child| {
                        boxes[*child].display.internal_table == Some(InternalTableRole::Cell)
                    })
                    .map(|source| PendingCell {
                        source,
                        row: index,
                        group: group_index,
                    }),
            );
        }
        self.row_groups.push(TableTrackGroup {
            source: group,
            kind,
            start,
            span: self.rows.len() - start,
        });
    }

    fn place_cells(&mut self, inputs: &TableGridInputs, pending: Vec<PendingCell>) {
        let mut occupancy = vec![Vec::<Option<usize>>::new(); self.rows.len()];
        for pending in pending {
            let input = inputs.cell(pending.source, &mut self.errors);
            let row_span = match input.row_span {
                TableRowSpan::Count(span) => self.valid_span(pending.source, span),
                TableRowSpan::ToEndOfGroup => self.row_groups[pending.group]
                    .start
                    .saturating_add(self.row_groups[pending.group].span)
                    .saturating_sub(pending.row)
                    .max(1),
            };
            let group_end =
                self.row_groups[pending.group].start + self.row_groups[pending.group].span;
            let row_span = row_span.min(group_end.saturating_sub(pending.row).max(1));
            let column_span = self.valid_span(pending.source, input.column_span);
            let mut column = 0usize;
            loop {
                self.ensure_columns(&mut occupancy, column + column_span);
                if (pending.row..pending.row + row_span).all(|row| {
                    (column..column + column_span).all(|column| occupancy[row][column].is_none())
                }) {
                    break;
                }
                column += 1;
            }

            let cell_index = self.cells.len();
            for (row, slots) in occupancy
                .iter_mut()
                .enumerate()
                .skip(pending.row)
                .take(row_span)
            {
                for (slot_column, slot) in
                    slots.iter_mut().enumerate().skip(column).take(column_span)
                {
                    if let Some(existing) = slot.replace(cell_index) {
                        self.errors.push(TableGridError::SlotOverlap {
                            row,
                            column: slot_column,
                            existing,
                            incoming: cell_index,
                        });
                    }
                }
            }
            self.cells.push(TableCell {
                source: pending.source,
                row: pending.row,
                row_span,
                column,
                column_span,
            });
        }

        self.slots = occupancy
            .into_iter()
            .enumerate()
            .flat_map(|(row, columns)| {
                columns
                    .into_iter()
                    .enumerate()
                    .map(move |(column, cell)| TableSlot { row, column, cell })
            })
            .collect();
    }

    fn ensure_columns(&mut self, occupancy: &mut [Vec<Option<usize>>], count: usize) {
        while self.columns.len() < count {
            let index = self.columns.len();
            self.columns.push(TableTrack {
                source: None,
                index,
                group: None,
            });
        }
        for row in occupancy {
            row.resize(self.columns.len(), None);
        }
    }

    fn valid_span(&mut self, source: BoxId, span: usize) -> usize {
        if span == 0 {
            self.errors.push(TableGridError::InvalidSpan { source });
            1
        } else {
            span
        }
    }
}

fn lookup_input<T: Copy>(
    inputs: &[(BoxId, T)],
    source: BoxId,
    errors: &mut Vec<TableGridError>,
    default: T,
) -> T {
    let mut matches = inputs
        .iter()
        .filter_map(|(candidate, input)| (*candidate == source).then_some(*input));
    let Some(input) = matches.next() else {
        return default;
    };
    if matches.next().is_some() {
        errors.push(TableGridError::OverlappingCellInput { source });
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BoxGeneration, BoxOrigin, BoxTreeInput, DisplayInside, DisplayOutside, DisplayRole,
        FlowAxes, PositioningScheme, generate_box_tree,
    };

    fn table_role(role: InternalTableRole) -> DisplayRole {
        DisplayRole {
            generation: BoxGeneration::Normal,
            outside: None,
            inside: None,
            list_item: false,
            internal_table: Some(role),
        }
    }

    fn table(children: Vec<BoxTreeInput<u8>>) -> CssBoxTree<u8> {
        generate_box_tree([BoxTreeInput::new(
            BoxOrigin::Element(1),
            DisplayRole {
                generation: BoxGeneration::Normal,
                outside: Some(DisplayOutside::Block),
                inside: Some(DisplayInside::Table),
                list_item: false,
                internal_table: None,
            },
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            children,
        )])
    }

    fn node(id: u8, role: InternalTableRole, children: Vec<BoxTreeInput<u8>>) -> BoxTreeInput<u8> {
        BoxTreeInput::new(
            BoxOrigin::Element(id),
            table_role(role),
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            children,
        )
    }

    fn grid(tree: &CssBoxTree<u8>) -> BoxId {
        tree.principal_box(1).expect("table grid")
    }

    #[test]
    fn simple_rows_fill_next_unoccupied_slots() {
        let tree = table(vec![node(
            2,
            InternalTableRole::RowGroup,
            vec![
                node(
                    3,
                    InternalTableRole::Row,
                    vec![
                        node(4, InternalTableRole::Cell, vec![]),
                        node(5, InternalTableRole::Cell, vec![]),
                    ],
                ),
                node(
                    6,
                    InternalTableRole::Row,
                    vec![
                        node(7, InternalTableRole::Cell, vec![]),
                        node(8, InternalTableRole::Cell, vec![]),
                    ],
                ),
            ],
        )]);
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &TableGridInputs::default());

        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.columns.len(), 2);
        assert_eq!(
            grid.cells
                .iter()
                .map(|cell| (cell.row, cell.column))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 1), (1, 0), (1, 1)]
        );
    }

    #[test]
    fn spans_grow_columns_and_occupy_later_rows() {
        let tree = table(vec![node(
            2,
            InternalTableRole::RowGroup,
            vec![
                node(
                    3,
                    InternalTableRole::Row,
                    vec![
                        node(4, InternalTableRole::Cell, vec![]),
                        node(5, InternalTableRole::Cell, vec![]),
                    ],
                ),
                node(
                    6,
                    InternalTableRole::Row,
                    vec![node(7, InternalTableRole::Cell, vec![])],
                ),
            ],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(4).unwrap(),
            TableCellInput {
                column_span: 2,
                row_span: TableRowSpan::Count(2),
            },
        );
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &inputs);

        assert_eq!(grid.columns.len(), 3);
        assert_eq!(
            grid.cells
                .iter()
                .map(|cell| (cell.row, cell.column, cell.row_span, cell.column_span))
                .collect::<Vec<_>>(),
            vec![(0, 0, 2, 2), (0, 2, 1, 1), (1, 2, 1, 1)]
        );
    }

    #[test]
    fn zero_rowspan_reaches_the_end_of_its_group() {
        let tree = table(vec![node(
            2,
            InternalTableRole::RowGroup,
            vec![
                node(
                    3,
                    InternalTableRole::Row,
                    vec![node(4, InternalTableRole::Cell, vec![])],
                ),
                node(
                    5,
                    InternalTableRole::Row,
                    vec![node(6, InternalTableRole::Cell, vec![])],
                ),
                node(
                    7,
                    InternalTableRole::Row,
                    vec![node(8, InternalTableRole::Cell, vec![])],
                ),
            ],
        )]);
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(
            tree.principal_box(4).unwrap(),
            TableCellInput {
                column_span: 1,
                row_span: TableRowSpan::ToEndOfGroup,
            },
        );
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &inputs);

        assert_eq!(grid.cells[0].row_span, 3);
        assert_eq!(
            grid.cells
                .iter()
                .map(|cell| cell.column)
                .collect::<Vec<_>>(),
            vec![0, 1, 1]
        );
    }

    #[test]
    fn duplicate_cell_input_is_an_explicit_overlap_error() {
        let tree = table(vec![node(
            2,
            InternalTableRole::RowGroup,
            vec![node(
                3,
                InternalTableRole::Row,
                vec![node(4, InternalTableRole::Cell, vec![])],
            )],
        )]);
        let cell = tree.principal_box(4).unwrap();
        let mut inputs = TableGridInputs::default();
        inputs.set_cell(cell, TableCellInput::default());
        inputs.set_cell(cell, TableCellInput::default());
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &inputs);

        assert_eq!(
            grid.errors,
            vec![TableGridError::OverlappingCellInput { source: cell }]
        );
    }

    #[test]
    fn explicit_columns_and_column_groups_do_not_create_cells() {
        let tree = table(vec![
            node(
                2,
                InternalTableRole::ColumnGroup,
                vec![node(3, InternalTableRole::Column, vec![])],
            ),
            node(4, InternalTableRole::Column, vec![]),
            node(
                5,
                InternalTableRole::RowGroup,
                vec![node(
                    6,
                    InternalTableRole::Row,
                    vec![node(7, InternalTableRole::Cell, vec![])],
                )],
            ),
        ]);
        let mut inputs = TableGridInputs::default();
        inputs.set_column(tree.principal_box(3).unwrap(), TableTrackInput { span: 2 });
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &inputs);

        assert_eq!(grid.column_groups.len(), 1);
        assert_eq!(grid.columns.len(), 3);
        assert_eq!(grid.cells.len(), 1);
        assert!(grid.columns.iter().all(|column| column.source.is_some()));
    }

    #[test]
    fn header_body_footer_tracks_follow_table_order_without_rewriting_sources() {
        let tree = table(vec![
            node(
                2,
                InternalTableRole::FooterGroup,
                vec![node(
                    3,
                    InternalTableRole::Row,
                    vec![node(4, InternalTableRole::Cell, vec![])],
                )],
            ),
            node(
                5,
                InternalTableRole::RowGroup,
                vec![node(
                    6,
                    InternalTableRole::Row,
                    vec![node(7, InternalTableRole::Cell, vec![])],
                )],
            ),
            node(
                8,
                InternalTableRole::HeaderGroup,
                vec![node(
                    9,
                    InternalTableRole::Row,
                    vec![node(10, InternalTableRole::Cell, vec![])],
                )],
            ),
        ]);
        let grid = TableGrid::from_box_tree(&tree, grid(&tree), &TableGridInputs::default());

        assert_eq!(
            grid.row_groups
                .iter()
                .map(|group| group.kind)
                .collect::<Vec<_>>(),
            vec![
                TableTrackGroupKind::Header,
                TableTrackGroupKind::Body,
                TableTrackGroupKind::Footer,
            ]
        );
        assert_eq!(
            grid.cells
                .iter()
                .map(|cell| tree.origin_node(cell.source))
                .collect::<Vec<_>>(),
            vec![Some(10), Some(7), Some(4)]
        );
    }
}
