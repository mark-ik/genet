//! K4g1: every border that can meet at every atomic table grid edge.
//!
//! This module deliberately performs no conflict resolution. It answers one
//! question - *which borders meet here* - and answers it for every atomic
//! segment of the grid, so that K4g2 can pick a winner per segment without
//! rediscovering topology and without geometry.
//!
//! An **atomic segment** is the span between two adjacent grid intersections
//! along one grid line. A cell's side may cover several of them: a cell
//! spanning three columns has three atomic segments along its block-start
//! side, and CSS 2.1 resolves each against whatever meets it from the other
//! side. Reducing that side to one edge before K4g3 has chosen a
//! harmonization would decide the question by accident, so nothing here does.
//!
//! Physical computed sides are mapped into the table's logical axes at the
//! adapter boundary, once, before a candidate reaches this module.

use std::cmp::Ordering;

use crate::BoxId;

use super::{TableGrid, TableTrackVisibility};

/// Which table role a border candidate came from.
///
/// CSS 2.1 section 17.6.2 uses this directly: where style and width tie, the
/// border of the box *further* from the table wins, and this is that order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TableBorderOrigin {
    /// Nearest the content, and the first to win a tie.
    Cell,
    Row,
    RowGroup,
    Column,
    ColumnGroup,
    /// Furthest from the content, and the last to win a tie.
    Table,
}

/// Whether a grid line runs along the inline axis or the block axis.
///
/// An inline-running line separates two rows; a block-running line separates
/// two columns. Named for the direction the border is *drawn*, not the axis it
/// divides, because that is how CSS 2.1 talks about them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum GridEdgeOrientation {
    /// Runs along the inline axis, separating one row from the next.
    InlineRunning,
    /// Runs along the block axis, separating one column from the next.
    BlockRunning,
}

/// One atomic segment of one grid line.
///
/// `line` counts intersections in the axis the line divides, so an
/// inline-running line at `line: 0` is the table's block-start edge and at
/// `line: rows.len()` its block-end edge. `segment` indexes the track the
/// segment crosses in the other axis.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TableGridEdge {
    pub orientation: GridEdgeOrientation,
    pub line: usize,
    pub segment: usize,
}

/// Border styles as CSS 2.1 section 17.6.2 orders them for conflict
/// resolution. The discriminant order *is* the precedence order, strongest
/// first, so `Hidden` sorting below everything is the rule rather than a
/// coincidence of naming.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TableBorderStyle {
    /// Wins over every other style regardless of width, and suppresses the
    /// border entirely. Retained rather than dropped: its painted width is
    /// zero, but "hidden here" is not the same as "nothing here".
    Hidden,
    Double,
    Solid,
    Dashed,
    Dotted,
    Ridge,
    Outset,
    Groove,
    Inset,
    /// Loses to every other style, and is not the same as an absent candidate.
    None,
}

impl TableBorderStyle {
    /// Whether this style suppresses the border rather than drawing one.
    pub fn suppresses(self) -> bool {
        self == Self::Hidden
    }

    /// Whether this style draws nothing but does not suppress.
    pub fn draws_nothing(self) -> bool {
        matches!(self, Self::None | Self::Hidden)
    }

    /// CSS 2.1 resolves `inset` and `outset` as `ridge` and `groove` in the
    /// collapsed model. Keep the original winner style for diagnostics; the
    /// paint phase asks for this mapped value instead.
    pub fn collapsed_paint_style(self) -> Self {
        match self {
            Self::Inset => Self::Ridge,
            Self::Outset => Self::Groove,
            other => other,
        }
    }
}

/// A stable tiebreak for two candidates that agree on style, width, and
/// origin. CSS 2.1 breaks that tie by document order, taking the leftmost and
/// topmost in a left-to-right table and the rightmost and topmost in a
/// right-to-left one, so the adapter supplies a direction-corrected index
/// rather than a raw DOM position.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TableBorderOrderKey(pub u32);

/// One border that meets one atomic segment.
///
/// The color payload is generic: conflict resolution compares style, width,
/// origin, and order, and only carries the color of whichever candidate wins.
/// Buckram never inspects it, which is what keeps Livery's color model out of
/// the table model.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderCandidate<Color> {
    pub edge: TableGridEdge,
    pub source: BoxId,
    pub source_side: TableBorderSide,
    pub origin: TableBorderOrigin,
    pub style: TableBorderStyle,
    /// The used border width in CSS pixels. Device-pixel snapping is a paint
    /// decision and does not happen here.
    pub width: f32,
    pub color: Color,
    pub order: TableBorderOrderKey,
}

/// Which of a box's own four logical sides a candidate came from.
///
/// Retained through resolution: K4g5 needs to know that a segment's winner was
/// some row's block-end rather than the next row's block-start, because the
/// two are painted from different boxes even where they land on one line.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TableBorderSide {
    InlineStart,
    InlineEnd,
    BlockStart,
    BlockEnd,
}

/// One box's four logical border sides, as the adapter lowered them.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderSides<Color> {
    pub inline_start: (TableBorderStyle, f32, Color),
    pub inline_end: (TableBorderStyle, f32, Color),
    pub block_start: (TableBorderStyle, f32, Color),
    pub block_end: (TableBorderStyle, f32, Color),
}

/// A source box's borders together with the identity conflict resolution
/// needs. The adapter builds one of these per participating box; this module
/// decides which atomic segments each one reaches.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderSource<Color> {
    pub source: BoxId,
    pub origin: TableBorderOrigin,
    pub sides: TableBorderSides<Color>,
    pub order: TableBorderOrderKey,
}

/// All border-owning table roles lowered from one normalized table grid.
///
/// Group, track, and cell vectors are intentionally distinct: borrowing the
/// table's computed sides for an implicit track would fabricate a CSS source.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderSources<Color> {
    pub table: TableBorderSource<Color>,
    pub row_groups: Vec<TableBorderSource<Color>>,
    pub rows: Vec<TableBorderSource<Color>>,
    pub column_groups: Vec<TableBorderSource<Color>>,
    pub columns: Vec<TableBorderSource<Color>>,
    pub cells: Vec<TableBorderSource<Color>>,
}

/// Why a candidate set could not be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBorderError {
    /// A width was negative, infinite, or NaN.
    InvalidWidth { source: BoxId },
    /// A track range named a track the grid does not have.
    TrackOutOfRange {
        source: BoxId,
        start: usize,
        span: usize,
    },
    /// The visibility mask does not describe this grid.
    TrackVisibilityShape,
    /// One table-role input did not cover the corresponding K4b topology.
    /// Silently zipping it would omit candidates before conflict resolution.
    SourceShape {
        origin: TableBorderOrigin,
        expected: usize,
        actual: usize,
    },
}

/// The first CSS2 comparison that distinguished two candidates.
///
/// `SourceIdentity` and `SourceSide` are not CSS precedence. They only make
/// a malformed adapter input with equal order keys deterministic. A valid
/// lowering supplies direction-corrected order keys, so ordinary CSS ties
/// end at [`Self::Order`]. `DuplicateIdentity` records a repeated identical
/// candidate; it is not a CSS comparison and does not select a second winner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBorderPrecedence {
    Hidden,
    None,
    Width,
    Style,
    Origin,
    Order,
    SourceIdentity,
    SourceSide,
    DuplicateIdentity,
}

/// The resolution status of one candidate in an atomic-segment ledger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBorderDisposition {
    Winner,
    /// `none` was the only style present, so the segment has no winner.
    OmittedNone,
    /// This candidate lost directly to the selected winner at this step.
    Lost {
        winner: BoxId,
        at: TableBorderPrecedence,
    },
}

/// One candidate plus the exact outcome it received at an atomic segment.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderLedgerEntry<Color> {
    pub candidate: TableBorderCandidate<Color>,
    pub disposition: TableBorderDisposition,
}

/// One resolved atomic segment. A hidden winner remains present as an
/// identity and diagnostic, but suppresses its painted output. An all-`none`
/// or empty segment has no winner.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTableBorder<Color> {
    pub edge: TableGridEdge,
    pub winner: Option<TableBorderCandidate<Color>>,
    pub suppressed_by_hidden: bool,
    pub ledger: Vec<TableBorderLedgerEntry<Color>>,
}

/// The one resolved winner (or explicit omission) for every atomic table
/// edge. K4g3 keeps these atomic answers before any spanning-side rule can
/// harmonize connected segments.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedTableBorderGrid<Color> {
    pub segments: Vec<ResolvedTableBorder<Color>>,
}

/// An adapter gave two distinct colors the same complete CSS conflict
/// identity. Color does not participate in CSS2 ranking, so selecting either
/// would make the result depend on candidate-vector order. Normal candidate
/// extraction cannot produce this shape; retain it as a diagnostic instead
/// of making a silent choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBorderResolutionError {
    IndistinguishableCandidates {
        edge: TableGridEdge,
        source: BoxId,
        side: TableBorderSide,
    },
}

/// Every candidate that meets every atomic segment, in a stable order.
///
/// The ledger is deliberately the whole of K4g1's output: winners are K4g2's,
/// and a fixture that wants to know what *could* have won reads this.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBorderCandidates<Color> {
    pub candidates: Vec<TableBorderCandidate<Color>>,
    /// Atomic segment counts, so a fixture can assert coverage without
    /// re-deriving the topology it is checking.
    pub inline_running_lines: usize,
    pub block_running_lines: usize,
}

impl<Color> TableBorderCandidates<Color> {
    /// Every candidate meeting one atomic segment, in collection order.
    pub fn at(&self, edge: TableGridEdge) -> impl Iterator<Item = &TableBorderCandidate<Color>> {
        self.candidates
            .iter()
            .filter(move |candidate| candidate.edge == edge)
    }

    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Resolve every atomic edge in this candidate grid. Empty edges and
    /// all-`none` conflicts remain visible as omitted segments.
    pub fn resolve(&self) -> Result<ResolvedTableBorderGrid<Color>, TableBorderResolutionError>
    where
        Color: Clone + PartialEq,
    {
        resolve_table_border_candidates(self)
    }
}

/// Compare two candidates by CSS 2.1 collapsed-border precedence.
///
/// A greater result means `left` wins. The returned step is the first rule
/// that decided the result. Color is intentionally absent: it travels with
/// the winner but never ranks it.
pub fn compare_table_border_candidates<Color>(
    left: &TableBorderCandidate<Color>,
    right: &TableBorderCandidate<Color>,
) -> (Ordering, Option<TableBorderPrecedence>) {
    let compare_style = |style: TableBorderStyle| match style {
        TableBorderStyle::Hidden => 2u8,
        TableBorderStyle::None => 0,
        _ => 1,
    };
    let hidden = compare_style(left.style).cmp(&compare_style(right.style));
    if hidden != Ordering::Equal
        && (left.style == TableBorderStyle::Hidden || right.style == TableBorderStyle::Hidden)
    {
        return (hidden, Some(TableBorderPrecedence::Hidden));
    }
    if hidden != Ordering::Equal {
        return (hidden, Some(TableBorderPrecedence::None));
    }

    let width = left.width.total_cmp(&right.width);
    if width != Ordering::Equal {
        return (width, Some(TableBorderPrecedence::Width));
    }
    // The enums intentionally run strongest to weakest, so reverse their
    // natural order when the larger `Ordering` value means "wins".
    let style = right.style.cmp(&left.style);
    if style != Ordering::Equal {
        return (style, Some(TableBorderPrecedence::Style));
    }
    let origin = right.origin.cmp(&left.origin);
    if origin != Ordering::Equal {
        return (origin, Some(TableBorderPrecedence::Origin));
    }
    // The adapter supplies a direction-corrected key whose smaller value is
    // closer to logical block-start and inline-start.
    let order = right.order.cmp(&left.order);
    if order != Ordering::Equal {
        return (order, Some(TableBorderPrecedence::Order));
    }
    let source = right.source.cmp(&left.source);
    if source != Ordering::Equal {
        return (source, Some(TableBorderPrecedence::SourceIdentity));
    }
    let side = right.source_side.cmp(&left.source_side);
    if side != Ordering::Equal {
        return (side, Some(TableBorderPrecedence::SourceSide));
    }
    (Ordering::Equal, None)
}

/// Resolve one winner (or omission) for every atomic segment in a candidate
/// ledger. This is intentionally free of layout geometry and paint order.
pub fn resolve_table_border_candidates<Color>(
    candidates: &TableBorderCandidates<Color>,
) -> Result<ResolvedTableBorderGrid<Color>, TableBorderResolutionError>
where
    Color: Clone + PartialEq,
{
    let mut segments = Vec::new();
    for line in 0..candidates.inline_running_lines {
        for segment in 0..candidates.block_running_lines.saturating_sub(1) {
            let edge = TableGridEdge {
                orientation: GridEdgeOrientation::InlineRunning,
                line,
                segment,
            };
            segments.push(resolve_edge(edge, candidates.at(edge))?);
        }
    }
    for line in 0..candidates.block_running_lines {
        for segment in 0..candidates.inline_running_lines.saturating_sub(1) {
            let edge = TableGridEdge {
                orientation: GridEdgeOrientation::BlockRunning,
                line,
                segment,
            };
            segments.push(resolve_edge(edge, candidates.at(edge))?);
        }
    }
    Ok(ResolvedTableBorderGrid { segments })
}

fn resolve_edge<'a, Color>(
    edge: TableGridEdge,
    candidates: impl Iterator<Item = &'a TableBorderCandidate<Color>>,
) -> Result<ResolvedTableBorder<Color>, TableBorderResolutionError>
where
    Color: Clone + PartialEq + 'a,
{
    let candidates = candidates.collect::<Vec<_>>();
    let Some(_) = candidates.first() else {
        return Ok(ResolvedTableBorder {
            edge,
            winner: None,
            suppressed_by_hidden: false,
            ledger: Vec::new(),
        });
    };

    let mut winner_index = 0;
    for (index, candidate) in candidates.iter().enumerate().skip(1) {
        let winner = candidates[winner_index];
        let (ordering, _) = compare_table_border_candidates(candidate, winner);
        match ordering {
            Ordering::Greater => winner_index = index,
            Ordering::Less => {},
            Ordering::Equal if candidate.color != winner.color => {
                return Err(TableBorderResolutionError::IndistinguishableCandidates {
                    edge,
                    source: candidate.source,
                    side: candidate.source_side,
                });
            },
            Ordering::Equal => {},
        }
    }

    let all_none = candidates
        .iter()
        .all(|candidate| candidate.style == TableBorderStyle::None);
    let winner = candidates[winner_index];
    let ledger = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let disposition = if all_none {
                TableBorderDisposition::OmittedNone
            } else if index == winner_index {
                TableBorderDisposition::Winner
            } else {
                let (ordering, at) = compare_table_border_candidates(candidate, winner);
                match ordering {
                    Ordering::Equal => TableBorderDisposition::Lost {
                        winner: winner.source,
                        at: TableBorderPrecedence::DuplicateIdentity,
                    },
                    Ordering::Less => TableBorderDisposition::Lost {
                        winner: winner.source,
                        at: at.expect("a non-equal border comparison names its rule"),
                    },
                    Ordering::Greater => {
                        unreachable!("the selected winner outranks every candidate")
                    },
                }
            };
            TableBorderLedgerEntry {
                candidate: (*candidate).clone(),
                disposition,
            }
        })
        .collect();
    let suppressed_by_hidden = winner.style.suppresses();
    Ok(ResolvedTableBorder {
        edge,
        winner: (!all_none).then(|| winner.clone()),
        suppressed_by_hidden,
        ledger,
    })
}

/// A rectangular range of the grid, in tracks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TrackRange {
    row: usize,
    row_span: usize,
    column: usize,
    column_span: usize,
}

impl TrackRange {
    fn validate(self, source: BoxId, grid: &TableGrid) -> Result<Self, TableBorderError> {
        let rows_fit = self
            .row
            .checked_add(self.row_span)
            .is_some_and(|end| end <= grid.rows.len());
        let columns_fit = self
            .column
            .checked_add(self.column_span)
            .is_some_and(|end| end <= grid.columns.len());
        if !rows_fit {
            return Err(TableBorderError::TrackOutOfRange {
                source,
                start: self.row,
                span: self.row_span,
            });
        }
        if !columns_fit {
            return Err(TableBorderError::TrackOutOfRange {
                source,
                start: self.column,
                span: self.column_span,
            });
        }
        Ok(self)
    }
}

/// Collect every border candidate meeting every atomic segment of one grid.
///
/// The four sides of each source are projected over the perimeter of its own
/// track range: a cell over its normalized span, a row over the whole row, a
/// column over the whole column, a group over its range, and the table over
/// its whole perimeter. Every candidate lands on at least one atomic segment,
/// and no candidate is reduced to a single edge on the way.
///
/// K4f's collapsed tracks stay in the topology. A collapsed track still has
/// intersections and still has borders meeting them; removing it here would
/// delete the model rather than the rendering, which is the wrong layer.
pub fn collect_table_border_candidates<Color: Clone>(
    grid: &TableGrid,
    visibility: &TableTrackVisibility,
    sources: TableBorderSources<Color>,
) -> Result<TableBorderCandidates<Color>, TableBorderError> {
    let TableBorderSources {
        table,
        row_groups,
        rows,
        column_groups,
        columns,
        cells,
    } = sources;
    if visibility.rows.len() != grid.rows.len() || visibility.columns.len() != grid.columns.len() {
        return Err(TableBorderError::TrackVisibilityShape);
    }
    for (origin, expected, actual) in [
        (
            TableBorderOrigin::RowGroup,
            grid.row_groups.len(),
            row_groups.len(),
        ),
        (TableBorderOrigin::Row, grid.rows.len(), rows.len()),
        (
            TableBorderOrigin::ColumnGroup,
            grid.column_groups.len(),
            column_groups.len(),
        ),
        (TableBorderOrigin::Column, grid.columns.len(), columns.len()),
        (TableBorderOrigin::Cell, grid.cells.len(), cells.len()),
    ] {
        if expected != actual {
            return Err(TableBorderError::SourceShape {
                origin,
                expected,
                actual,
            });
        }
    }
    let mut candidates = Vec::new();

    // The table's own perimeter, then the groups, then the tracks, then the
    // cells. Collection order is fixed by role rather than by traversal, which
    // is what makes the ledger invariant under paint order.
    let whole = TrackRange {
        row: 0,
        row_span: grid.rows.len(),
        column: 0,
        column_span: grid.columns.len(),
    };
    project(&mut candidates, grid, table, whole)?;

    for (group, source) in grid.column_groups.iter().zip(column_groups) {
        let range = TrackRange {
            row: 0,
            row_span: grid.rows.len(),
            column: group.start,
            column_span: group.span,
        };
        project(&mut candidates, grid, source.clone(), range)?;
    }
    for (group, source) in grid.row_groups.iter().zip(row_groups) {
        let range = TrackRange {
            row: group.start,
            row_span: group.span,
            column: 0,
            column_span: grid.columns.len(),
        };
        project(&mut candidates, grid, source.clone(), range)?;
    }

    for (index, row) in rows.iter().enumerate() {
        project(
            &mut candidates,
            grid,
            row.clone(),
            TrackRange {
                row: index,
                row_span: 1,
                column: 0,
                column_span: grid.columns.len(),
            },
        )?;
    }
    for (index, column) in columns.iter().enumerate() {
        project(
            &mut candidates,
            grid,
            column.clone(),
            TrackRange {
                row: 0,
                row_span: grid.rows.len(),
                column: index,
                column_span: 1,
            },
        )?;
    }
    for (cell, source) in grid.cells.iter().zip(cells) {
        project(
            &mut candidates,
            grid,
            source.clone(),
            TrackRange {
                row: cell.row,
                row_span: cell.row_span,
                column: cell.column,
                column_span: cell.column_span,
            },
        )?;
    }

    Ok(TableBorderCandidates {
        candidates,
        inline_running_lines: grid.rows.len() + 1,
        block_running_lines: grid.columns.len() + 1,
    })
}

/// Spread one source's four sides over the perimeter of its track range, one
/// candidate per atomic segment.
fn project<Color: Clone>(
    out: &mut Vec<TableBorderCandidate<Color>>,
    grid: &TableGrid,
    source: TableBorderSource<Color>,
    range: TrackRange,
) -> Result<(), TableBorderError> {
    let range = range.validate(source.source, grid)?;
    let sides = [
        (
            TableBorderSide::BlockStart,
            source.sides.block_start,
            GridEdgeOrientation::InlineRunning,
            range.row,
        ),
        (
            TableBorderSide::BlockEnd,
            source.sides.block_end,
            GridEdgeOrientation::InlineRunning,
            range.row + range.row_span,
        ),
        (
            TableBorderSide::InlineStart,
            source.sides.inline_start,
            GridEdgeOrientation::BlockRunning,
            range.column,
        ),
        (
            TableBorderSide::InlineEnd,
            source.sides.inline_end,
            GridEdgeOrientation::BlockRunning,
            range.column + range.column_span,
        ),
    ];
    for (side, (style, width, color), orientation, line) in sides {
        if !width.is_finite() || width < 0.0 {
            return Err(TableBorderError::InvalidWidth {
                source: source.source,
            });
        }
        let segments = match orientation {
            GridEdgeOrientation::InlineRunning => range.column..range.column + range.column_span,
            GridEdgeOrientation::BlockRunning => range.row..range.row + range.row_span,
        };
        for segment in segments {
            out.push(TableBorderCandidate {
                edge: TableGridEdge {
                    orientation,
                    line,
                    segment,
                },
                source: source.source,
                source_side: side,
                origin: source.origin,
                style,
                width,
                color: color.clone(),
                order: source.order,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside, DisplayOutside,
        DisplayRole, FlowAxes, InternalTableRole, PositioningScheme, TableCellInput, TableGrid,
        TableGridInputs, generate_box_tree,
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

    /// A two-by-two table, with `spans` applied to the cells that carry one.
    fn table_2x2(spans: &[(u8, TableCellInput)]) -> TableGrid {
        let cell = |id| node(id, InternalTableRole::Cell, vec![]);
        let tree: CssBoxTree<u8> = generate_box_tree([BoxTreeInput::new(
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
            vec![node(
                2,
                InternalTableRole::RowGroup,
                vec![
                    node(3, InternalTableRole::Row, vec![cell(10), cell(11)]),
                    node(4, InternalTableRole::Row, vec![cell(12), cell(13)]),
                ],
            )],
        )]);
        let mut inputs = TableGridInputs::default();
        for (id, input) in spans {
            inputs.set_cell(tree.principal_box(*id).expect("cell"), *input);
        }
        TableGrid::from_box_tree(&tree, tree.principal_box(1).expect("grid"), &inputs)
    }

    fn sides(style: TableBorderStyle, width: f32) -> TableBorderSides<u8> {
        TableBorderSides {
            inline_start: (style, width, 0),
            inline_end: (style, width, 0),
            block_start: (style, width, 0),
            block_end: (style, width, 0),
        }
    }

    fn source(id: BoxId, origin: TableBorderOrigin, width: f32) -> TableBorderSource<u8> {
        TableBorderSource {
            source: id,
            origin,
            sides: sides(TableBorderStyle::Solid, width),
            order: TableBorderOrderKey(id.index() as u32),
        }
    }

    /// Every participating box's borders, one per role, so a test can adjust
    /// exactly the one it is about.
    struct Scene {
        table: TableBorderSource<u8>,
        row_groups: Vec<TableBorderSource<u8>>,
        rows: Vec<TableBorderSource<u8>>,
        column_groups: Vec<TableBorderSource<u8>>,
        columns: Vec<TableBorderSource<u8>>,
        cells: Vec<TableBorderSource<u8>>,
    }

    fn scene(grid: &TableGrid) -> Scene {
        let track = |track: &crate::TableTrack, origin| {
            source(track.source.unwrap_or(grid.grid), origin, 1.0)
        };
        Scene {
            table: source(grid.grid, TableBorderOrigin::Table, 1.0),
            row_groups: grid
                .row_groups
                .iter()
                .map(|group| source(group.source, TableBorderOrigin::RowGroup, 1.0))
                .collect(),
            rows: grid
                .rows
                .iter()
                .map(|row| track(row, TableBorderOrigin::Row))
                .collect(),
            column_groups: grid
                .column_groups
                .iter()
                .map(|group| source(group.source, TableBorderOrigin::ColumnGroup, 1.0))
                .collect(),
            columns: grid
                .columns
                .iter()
                .map(|column| track(column, TableBorderOrigin::Column))
                .collect(),
            cells: grid
                .cells
                .iter()
                .map(|cell| source(cell.source, TableBorderOrigin::Cell, 2.0))
                .collect(),
        }
    }

    impl Scene {
        fn collect(&self, grid: &TableGrid) -> TableBorderCandidates<u8> {
            self.collect_with(grid, &TableTrackVisibility::all_visible(grid))
                .expect("candidates")
        }

        fn collect_with(
            &self,
            grid: &TableGrid,
            visibility: &TableTrackVisibility,
        ) -> Result<TableBorderCandidates<u8>, TableBorderError> {
            collect_table_border_candidates(
                grid,
                visibility,
                TableBorderSources {
                    table: self.table.clone(),
                    row_groups: self.row_groups.clone(),
                    rows: self.rows.clone(),
                    column_groups: self.column_groups.clone(),
                    columns: self.columns.clone(),
                    cells: self.cells.clone(),
                },
            )
        }
    }

    #[test]
    fn every_atomic_segment_of_an_interior_line_is_reached() {
        let grid = table_2x2(&[]);
        let candidates = scene(&grid).collect(&grid);

        for segment in 0..2 {
            let edge = TableGridEdge {
                orientation: GridEdgeOrientation::InlineRunning,
                line: 1,
                segment,
            };
            let origins = candidates
                .at(edge)
                .map(|candidate| candidate.origin)
                .collect::<Vec<_>>();
            let count = |origin| origins.iter().filter(|each| **each == origin).count();
            assert_eq!(
                count(TableBorderOrigin::Cell),
                2,
                "both cells meet the interior line at segment {segment}: {origins:?}"
            );
            assert_eq!(
                count(TableBorderOrigin::Row),
                2,
                "both rows meet it too: {origins:?}"
            );
        }
    }

    /// A spanning cell's side covers several atomic segments and is not
    /// reduced to one edge, which is K4g1's stop rule about spanning sides.
    #[test]
    fn a_spanning_cells_side_reaches_one_segment_per_track() {
        let grid = table_2x2(&[(
            10,
            TableCellInput {
                column_span: 2,
                ..TableCellInput::default()
            },
        )]);
        assert_eq!(grid.cells[0].column_span, 2);
        let spanning = grid.cells[0].source;
        let candidates = scene(&grid).collect(&grid);

        let along = candidates
            .candidates
            .iter()
            .filter(|candidate| candidate.source == spanning)
            .filter(|candidate| candidate.source_side == TableBorderSide::BlockEnd)
            .collect::<Vec<_>>();
        assert_eq!(along.len(), 2, "one per column it spans: {along:?}");
        assert_eq!(along[0].edge.segment, 0);
        assert_eq!(along[1].edge.segment, 1);
        assert!(along.iter().all(|candidate| candidate.edge.line == 1));
    }

    /// `hidden` and `none` are kept. A resolved `hidden` paints nothing, but
    /// "suppressed here" and "no candidate here" are different answers and
    /// K4g2 needs to tell them apart.
    #[test]
    fn hidden_and_none_survive_collection() {
        let grid = table_2x2(&[]);
        let mut scene = scene(&grid);
        scene.cells[0].sides.block_start = (TableBorderStyle::Hidden, 0.0, 0);
        scene.cells[1].sides.block_start = (TableBorderStyle::None, 0.0, 0);
        let candidates = scene.collect(&grid);

        let style_of = |source: BoxId| {
            candidates
                .candidates
                .iter()
                .find(|candidate| {
                    candidate.source == source
                        && candidate.source_side == TableBorderSide::BlockStart
                })
                .map(|candidate| candidate.style)
        };
        assert_eq!(
            style_of(grid.cells[0].source),
            Some(TableBorderStyle::Hidden)
        );
        assert_eq!(style_of(grid.cells[1].source), Some(TableBorderStyle::None));
    }

    /// A row group projects over exactly its range: its own outer sides land
    /// on the lines bounding that range, never on an interior line.
    #[test]
    fn a_row_group_projects_over_exactly_its_range() {
        let grid = table_2x2(&[]);
        let group = grid.row_groups[0].source;
        assert_eq!(grid.row_groups[0].span, 2);
        let candidates = scene(&grid).collect(&grid);

        let lines = candidates
            .candidates
            .iter()
            .filter(|candidate| candidate.source == group)
            .filter(|candidate| candidate.edge.orientation == GridEdgeOrientation::InlineRunning)
            .map(|candidate| candidate.edge.line)
            .collect::<Vec<_>>();
        assert!(lines.contains(&0) && lines.contains(&2), "{lines:?}");
        assert!(
            !lines.contains(&1),
            "an interior line is not a group edge: {lines:?}"
        );
    }

    /// K4f's collapsed tracks keep their topology: the model still has the
    /// intersections and the borders that meet them, and only rendering
    /// decides what becomes of them.
    #[test]
    fn a_collapsed_track_keeps_its_edges() {
        let grid = table_2x2(&[]);
        let scene = scene(&grid);
        let mut visibility = TableTrackVisibility::all_visible(&grid);
        visibility.columns[0] = crate::TableTrackVisibilityState::Collapsed;

        let collapsed = scene
            .collect_with(&grid, &visibility)
            .expect("collapsed tracks keep their candidates");
        assert_eq!(collapsed.len(), scene.collect(&grid).len());
    }

    #[test]
    fn a_negative_border_width_is_rejected_rather_than_clamped() {
        let grid = table_2x2(&[]);
        let mut scene = scene(&grid);
        scene.cells[2].sides.inline_start = (TableBorderStyle::Solid, -1.0, 0);
        assert_eq!(
            scene.collect_with(&grid, &TableTrackVisibility::all_visible(&grid)),
            Err(TableBorderError::InvalidWidth {
                source: grid.cells[2].source
            })
        );
    }

    /// CSS 2.1 section 17.6.2 orders origins from the cell outward and styles
    /// from `hidden` down to `none`. Both are the enums' own order, so a later
    /// gate compares them directly instead of writing the table out again.
    #[test]
    fn precedence_order_is_the_enum_order() {
        assert!(TableBorderOrigin::Cell < TableBorderOrigin::Table);
        assert!(TableBorderOrigin::Row < TableBorderOrigin::RowGroup);
        assert!(TableBorderStyle::Hidden < TableBorderStyle::Double);
        assert!(TableBorderStyle::Double < TableBorderStyle::Solid);
        assert!(TableBorderStyle::Inset < TableBorderStyle::None);
        assert!(TableBorderStyle::Hidden.suppresses());
        assert!(TableBorderStyle::None.draws_nothing());
        assert!(!TableBorderStyle::None.suppresses());
    }

    fn candidate(
        _grid: &TableGrid,
        source: BoxId,
        style: TableBorderStyle,
        width: f32,
        origin: TableBorderOrigin,
        order: u32,
    ) -> TableBorderCandidate<u8> {
        TableBorderCandidate {
            edge: TableGridEdge {
                orientation: GridEdgeOrientation::InlineRunning,
                line: 1,
                segment: 0,
            },
            source,
            source_side: TableBorderSide::BlockEnd,
            origin,
            style,
            width,
            color: source.index() as u8,
            order: TableBorderOrderKey(order),
        }
    }

    fn resolved(candidates: Vec<TableBorderCandidate<u8>>) -> ResolvedTableBorder<u8> {
        let edge = candidates[0].edge;
        let grid = TableBorderCandidates {
            candidates,
            inline_running_lines: 2,
            block_running_lines: 2,
        }
        .resolve()
        .expect("resolves");
        grid.segments
            .into_iter()
            .find(|segment| segment.edge == edge)
            .expect("edge result")
    }

    #[test]
    fn css2_comparator_checks_hidden_none_width_style_origin_and_order_in_order() {
        let grid = table_2x2(&[]);
        let cell = grid.cells[0].source;
        let row = grid.rows[0].source.expect("row source");
        let left = candidate(
            &grid,
            cell,
            TableBorderStyle::Solid,
            1.0,
            TableBorderOrigin::Cell,
            2,
        );
        let mut right = candidate(
            &grid,
            row,
            TableBorderStyle::Solid,
            9.0,
            TableBorderOrigin::Row,
            1,
        );

        right.style = TableBorderStyle::Hidden;
        assert_eq!(
            compare_table_border_candidates(&right, &left),
            (Ordering::Greater, Some(TableBorderPrecedence::Hidden))
        );
        right.style = TableBorderStyle::None;
        assert_eq!(
            compare_table_border_candidates(&left, &right),
            (Ordering::Greater, Some(TableBorderPrecedence::None))
        );
        right.style = TableBorderStyle::Solid;
        assert_eq!(
            compare_table_border_candidates(&right, &left),
            (Ordering::Greater, Some(TableBorderPrecedence::Width))
        );
        right.width = left.width;
        right.style = TableBorderStyle::Double;
        assert_eq!(
            compare_table_border_candidates(&right, &left),
            (Ordering::Greater, Some(TableBorderPrecedence::Style))
        );
        right.style = left.style;
        assert_eq!(
            compare_table_border_candidates(&left, &right),
            (Ordering::Greater, Some(TableBorderPrecedence::Origin))
        );
        right.origin = left.origin;
        assert_eq!(
            compare_table_border_candidates(&right, &left),
            (Ordering::Greater, Some(TableBorderPrecedence::Order))
        );
    }

    #[test]
    fn resolution_is_permutation_invariant_and_its_ledger_names_the_loss() {
        let grid = table_2x2(&[]);
        let cell = grid.cells[0].source;
        let row = grid.rows[0].source.expect("row source");
        let table = grid.grid;
        let candidates = vec![
            candidate(
                &grid,
                table,
                TableBorderStyle::Solid,
                3.0,
                TableBorderOrigin::Table,
                3,
            ),
            candidate(
                &grid,
                row,
                TableBorderStyle::Solid,
                3.0,
                TableBorderOrigin::Row,
                2,
            ),
            candidate(
                &grid,
                cell,
                TableBorderStyle::Solid,
                4.0,
                TableBorderOrigin::Cell,
                1,
            ),
        ];
        let winner = resolved(candidates.clone());
        let mut reversed = candidates;
        reversed.reverse();
        let permuted = resolved(reversed);
        assert_eq!(winner.winner, permuted.winner);
        assert_eq!(
            winner.winner.as_ref().map(|winner| winner.source),
            Some(cell)
        );
        assert!(winner.ledger.iter().any(|entry| {
            entry.candidate.source == row
                && entry.disposition
                    == TableBorderDisposition::Lost {
                        winner: cell,
                        at: TableBorderPrecedence::Width,
                    }
        }));
    }

    #[test]
    fn an_exact_duplicate_has_one_winner_and_a_diagnostic_loss() {
        let grid = table_2x2(&[]);
        let candidate = candidate(
            &grid,
            grid.cells[0].source,
            TableBorderStyle::Solid,
            3.0,
            TableBorderOrigin::Cell,
            0,
        );
        let resolved = resolved(vec![candidate.clone(), candidate]);
        assert_eq!(
            resolved
                .ledger
                .iter()
                .filter(|entry| entry.disposition == TableBorderDisposition::Winner)
                .count(),
            1
        );
        assert!(resolved.ledger.iter().any(|entry| {
            entry.disposition
                == TableBorderDisposition::Lost {
                    winner: grid.cells[0].source,
                    at: TableBorderPrecedence::DuplicateIdentity,
                }
        }));
    }

    #[test]
    fn hidden_and_all_none_have_distinct_resolved_results() {
        let grid = table_2x2(&[]);
        let cell = grid.cells[0].source;
        let row = grid.rows[0].source.expect("row source");
        let hidden = resolved(vec![
            candidate(
                &grid,
                cell,
                TableBorderStyle::Hidden,
                0.0,
                TableBorderOrigin::Cell,
                1,
            ),
            candidate(
                &grid,
                row,
                TableBorderStyle::Double,
                12.0,
                TableBorderOrigin::Row,
                0,
            ),
        ]);
        assert!(hidden.suppressed_by_hidden);
        assert_eq!(
            hidden.winner.as_ref().map(|winner| winner.style),
            Some(TableBorderStyle::Hidden)
        );

        let none = resolved(vec![
            candidate(
                &grid,
                cell,
                TableBorderStyle::None,
                0.0,
                TableBorderOrigin::Cell,
                1,
            ),
            candidate(
                &grid,
                row,
                TableBorderStyle::None,
                3.0,
                TableBorderOrigin::Row,
                0,
            ),
        ]);
        assert!(!none.suppressed_by_hidden);
        assert_eq!(none.winner, None);
        assert!(
            none.ledger
                .iter()
                .all(|entry| entry.disposition == TableBorderDisposition::OmittedNone)
        );
    }

    #[test]
    fn collapsed_paint_style_keeps_the_diagnostic_winner_but_maps_relief_styles() {
        assert_eq!(
            TableBorderStyle::Inset.collapsed_paint_style(),
            TableBorderStyle::Ridge
        );
        assert_eq!(
            TableBorderStyle::Outset.collapsed_paint_style(),
            TableBorderStyle::Groove
        );
        assert_eq!(
            TableBorderStyle::Double.collapsed_paint_style(),
            TableBorderStyle::Double
        );
    }

    #[test]
    fn group_sources_supply_their_own_candidate_sides() {
        let grid = table_2x2(&[]);
        let mut scene = scene(&grid);
        scene.row_groups[0].sides.block_start = (TableBorderStyle::Dotted, 7.0, 9);
        let candidates = scene.collect(&grid);
        assert!(candidates.candidates.iter().any(|candidate| {
            candidate.source == grid.row_groups[0].source
                && candidate.source_side == TableBorderSide::BlockStart
                && candidate.style == TableBorderStyle::Dotted
                && candidate.width == 7.0
                && candidate.color == 9
        }));
    }
}
