//! Logical-axis table inline-sizing contracts.
//!
//! This module deliberately owns inputs and intrinsic cell measurements only.
//! K4c2 and later select fixed and automatic column algorithms; no completed
//! fragment, Taffy track, DOM node, or HTML attribute enters this boundary.

use crate::{
    BoxId, IntrinsicQueryError, IntrinsicQueryState, IntrinsicSizeCache, IntrinsicSizeKind,
    IntrinsicSizeQuery, IntrinsicSizes, LogicalAxis,
};

use super::TableGrid;

/// A finite affine CSS length-percentage retained until its percentage basis is
/// known. `percentage: 1.0` means `100%`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AffineLengthPercentage {
    pub absolute: f32,
    pub percentage: f32,
}

impl AffineLengthPercentage {
    pub const ZERO: Self = Self {
        absolute: 0.0,
        percentage: 0.0,
    };

    /// An absolute length with no percentage component.
    pub const fn px(absolute: f32) -> Self {
        Self {
            absolute,
            percentage: 0.0,
        }
    }

    pub fn new(absolute: f32, percentage: f32) -> Option<Self> {
        (absolute.is_finite() && percentage.is_finite()).then_some(Self {
            absolute,
            percentage,
        })
    }

    pub const fn needs_percentage_basis(self) -> bool {
        self.percentage != 0.0
    }

    pub fn resolve(self, percentage_basis: f32) -> Option<f32> {
        (percentage_basis.is_finite())
            .then_some(self.absolute + self.percentage * percentage_basis)
            .filter(|value| value.is_finite())
    }
}

/// A CSS inline-size value before table sizing has an applicable basis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InlineSizeConstraint {
    Auto,
    None,
    MinContent,
    MaxContent,
    FitContent(AffineLengthPercentage),
    Value(AffineLengthPercentage),
    /// A computed expression which cannot yet reduce to an affine
    /// length-percentage without losing CSS semantics.
    Unreduced,
}

impl InlineSizeConstraint {
    pub const fn needs_percentage_basis(self) -> bool {
        match self {
            Self::FitContent(value) | Self::Value(value) => value.needs_percentage_basis(),
            Self::Auto | Self::None | Self::MinContent | Self::MaxContent | Self::Unreduced => {
                false
            },
        }
    }

    pub(super) fn resolve_definite(
        self,
        percentage_basis: Option<f32>,
        box_id: Option<BoxId>,
        property: TableInlineProperty,
    ) -> Result<Option<f32>, TableInlineSizingError> {
        let Self::Value(value) = self else {
            return match self {
                Self::Unreduced => {
                    Err(TableInlineSizingError::UnreducedConstraint { box_id, property })
                },
                Self::Auto
                | Self::None
                | Self::MinContent
                | Self::MaxContent
                | Self::FitContent(_) => Ok(None),
                Self::Value(_) => unreachable!(),
            };
        };
        let Some(basis) = percentage_basis.or((!value.needs_percentage_basis()).then_some(0.0))
        else {
            return Err(TableInlineSizingError::UnresolvedPercentageBasis { box_id, property });
        };
        value
            .resolve(basis)
            .map(|resolved| Some(resolved.max(0.0)))
            .ok_or(TableInlineSizingError::InvalidConstraint { box_id, property })
    }
}

/// The box edge selected by `box-sizing` while retaining the logical offsets
/// that convert between a cell content and border box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableBoxSizing {
    ContentBox,
    BorderBox,
}

/// A table or cell's logical inline-size constraints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableInlineConstraints {
    pub preferred: InlineSizeConstraint,
    pub minimum: InlineSizeConstraint,
    pub maximum: InlineSizeConstraint,
    pub box_sizing: TableBoxSizing,
}

impl Default for TableInlineConstraints {
    fn default() -> Self {
        Self {
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Auto,
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::ContentBox,
        }
    }
}

/// The logical inline padding and border between one cell's content and border
/// edges. They remain separate from content contributions so K4g can replace
/// the border portion with collapsed-border winners.
///
/// Padding retains its percentage. CSS resolves a padding percentage against
/// the containing block's inline size, which is the used grid width for a cell
/// and the table's own containing block for the table box. Neither basis exists
/// when Livery lowers computed style, so the adapter must not choose one.
/// A border cannot be a percentage, so it stays absolute.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellInlineOffsets {
    pub padding_start: AffineLengthPercentage,
    pub padding_end: AffineLengthPercentage,
    pub border_start: f32,
    pub border_end: f32,
}

impl CellInlineOffsets {
    pub const ZERO: Self = Self {
        padding_start: AffineLengthPercentage::ZERO,
        padding_end: AffineLengthPercentage::ZERO,
        border_start: 0.0,
        border_end: 0.0,
    };

    pub const fn needs_percentage_basis(self) -> bool {
        self.padding_start.needs_percentage_basis() || self.padding_end.needs_percentage_basis()
    }

    pub fn is_valid(self) -> bool {
        [
            self.padding_start.absolute,
            self.padding_start.percentage,
            self.padding_end.absolute,
            self.padding_end.percentage,
            self.border_start,
            self.border_end,
        ]
        .into_iter()
        .all(|value| value.is_finite() && value >= 0.0)
    }

    /// The total inline offset against a known padding percentage basis.
    pub fn total(self, percentage_basis: f32) -> Option<f32> {
        if !self.is_valid() {
            return None;
        }
        let total = self.padding_start.resolve(percentage_basis)?
            + self.padding_end.resolve(percentage_basis)?
            + self.border_start
            + self.border_end;
        total.is_finite().then_some(total)
    }

    /// The total inline offset where no basis exists yet. `None` when a padding
    /// percentage is present, so a caller must defer rather than silently
    /// sample the percentage at zero.
    pub fn absolute_total(self) -> Option<f32> {
        (!self.needs_percentage_basis())
            .then(|| self.total(0.0))
            .flatten()
    }
}

/// Separated-model table geometry that does not belong to a distributable
/// column width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableSeparatedBorderMetrics {
    pub table_offsets: CellInlineOffsets,
    pub inline_spacing: f32,
}

/// Collapsed-model table geometry outside distributable column tracks.
///
/// K4g3 projects the resolved winner grid into half-width outer edges. The
/// table's declared borders do not participate here: the accepted winner is
/// the only border at a collapsed outer edge. Padding remains the table's own
/// property and resolves against its containing block just as it does in the
/// separated model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TableCollapsedBorderMetrics {
    pub table_padding: CellInlineOffsets,
    pub outer_start: f32,
    pub outer_end: f32,
}

impl TableCollapsedBorderMetrics {
    /// Table padding plus the accepted half-width outer winners. Collapsed
    /// borders have neither `border-spacing` nor a second declared table
    /// border contribution.
    pub fn undistributable_inline_size(self, percentage_basis: f32) -> Option<f32> {
        let padding = self.table_padding.total(percentage_basis)?;
        [padding, self.outer_start, self.outer_end]
            .into_iter()
            .all(|value| value.is_finite() && value >= 0.0)
            .then_some(padding + self.outer_start + self.outer_end)
            .filter(|total| total.is_finite())
    }
}

impl TableSeparatedBorderMetrics {
    /// The two table edges plus one spacing interval before, after, and between
    /// every K4b column. The basis resolves a table padding percentage and is
    /// the table's own containing block, never its used width.
    pub fn undistributable_inline_size(
        self,
        column_count: usize,
        percentage_basis: f32,
    ) -> Option<f32> {
        let offsets = self.table_offsets.total(percentage_basis)?;
        if !self.inline_spacing.is_finite() || self.inline_spacing < 0.0 {
            return None;
        }
        let gaps = column_count.checked_add(1)? as f32;
        (offsets + self.inline_spacing * gaps)
            .is_finite()
            .then_some(offsets + self.inline_spacing * gaps)
    }
}

/// Border-model geometry supplied to K4c. Declared cell borders are not an
/// acceptable stand-in for collapsed-border winners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableInlineBorderMetrics {
    Separated(TableSeparatedBorderMetrics),
    Collapsed(TableCollapsedBorderMetrics),
    CollapsedPendingK4g,
}

/// Caption minimum information deliberately held apart from table-grid
/// topology and column arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptionMinContribution {
    NoCaption,
    Measured(f32),
    PendingK4e,
}

impl CaptionMinContribution {
    pub fn measured(self) -> Result<Option<f32>, TableInlineSizingError> {
        match self {
            Self::NoCaption => Ok(None),
            Self::Measured(value) if value.is_finite() && value >= 0.0 => Ok(Some(value)),
            Self::Measured(_) => Err(TableInlineSizingError::InvalidCaptionMinimum),
            Self::PendingK4e => Err(TableInlineSizingError::Deferral(
                TableDeferral::CaptionMinPendingK4e,
            )),
        }
    }
}

/// A future K4f visibility pass can mark tracks collapsed without dropping the
/// constraints which established their pre-collapse measures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableTrackVisibilityState {
    Visible,
    Collapsed,
}

/// Visibility is shaped like K4b's rows and columns, not like rendered
/// fragments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableTrackVisibility {
    pub rows: Vec<TableTrackVisibilityState>,
    pub columns: Vec<TableTrackVisibilityState>,
}

impl TableTrackVisibility {
    pub fn all_visible(grid: &TableGrid) -> Self {
        Self {
            rows: vec![TableTrackVisibilityState::Visible; grid.rows.len()],
            columns: vec![TableTrackVisibilityState::Visible; grid.columns.len()],
        }
    }

    fn matches_grid(&self, grid: &TableGrid) -> bool {
        self.rows.len() == grid.rows.len() && self.columns.len() == grid.columns.len()
    }

    pub fn has_collapsed(&self) -> bool {
        self.rows
            .iter()
            .chain(self.columns.iter())
            .any(|state| *state == TableTrackVisibilityState::Collapsed)
    }

    pub fn column_is_collapsed(&self, index: usize) -> bool {
        self.columns.get(index) == Some(&TableTrackVisibilityState::Collapsed)
    }

    pub fn row_is_collapsed(&self, index: usize) -> bool {
        self.rows.get(index) == Some(&TableTrackVisibilityState::Collapsed)
    }

    /// Whether any cell spans across a collapsed track boundary.
    ///
    /// CSS Tables 3 does not merely narrow such a cell: it clips the cell's
    /// content at the collapsed track's edge, which is a rendering rule rather
    /// than a sizing one and has no seam yet. A cell wholly inside collapsed
    /// tracks, or wholly outside them, needs no clip - only one that straddles
    /// the boundary does.
    pub fn spans_a_collapsed_boundary(&self, grid: &TableGrid) -> bool {
        grid.cells.iter().any(|cell| {
            let straddles = |collapsed: &dyn Fn(usize) -> bool, start: usize, span: usize| {
                let mut tracks = start..start.saturating_add(span);
                tracks.any(collapsed) && (start..start.saturating_add(span)).any(|i| !collapsed(i))
            };
            straddles(
                &|index| self.column_is_collapsed(index),
                cell.column,
                cell.column_span,
            ) || straddles(
                &|index| self.row_is_collapsed(index),
                cell.row,
                cell.row_span,
            )
        })
    }
}

/// Remove collapsed columns from a distribution that was computed as if every
/// track were visible.
///
/// CSS 2.1 section 17.5.5: a collapsed column is not rendered and the table's
/// width is reduced by exactly what that column occupied, while the other
/// columns keep the widths they were given. So the collapse is a subtraction
/// after the distribution rather than an input to it, which is also what keeps
/// K4f's stop rule - the constraints that produced the widths are still the
/// constraints, and nothing was deleted before they were consulted.
pub fn collapse_columns(
    visibility: &TableTrackVisibility,
    column_sizes: &mut [f32],
    used_grid_inline_size: &mut f32,
    used_table_inline_size: &mut f32,
) {
    let mut removed = 0.0;
    for (index, size) in column_sizes.iter_mut().enumerate() {
        if visibility.column_is_collapsed(index) {
            removed += *size;
            *size = 0.0;
        }
    }
    *used_grid_inline_size = (*used_grid_inline_size - removed).max(0.0);
    *used_table_inline_size = (*used_table_inline_size - removed).max(0.0);
}

/// Named distinctions deferred to later K4 gates. These names describe CSS
/// work, never a DOM tag pattern or a backend limitation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableDeferral {
    CaptionMinPendingK4e,
    TrackVisibilityPendingK4f,
    CollapsedBorderMetricsPendingK4g,
    /// A padding percentage whose containing-block basis is not yet available.
    /// Automatic sizing measures cells before the table width exists, so the
    /// dependency is genuinely circular there and stays explicit.
    PercentagePaddingPendingBasis,
}

/// One complete table-sizing input, all in the table's logical inline axis.
#[derive(Clone, Debug, PartialEq)]
pub struct TableInlineSizingInput<'a> {
    pub grid: &'a TableGrid,
    pub available_inline_size: Option<f32>,
    pub table_constraints: TableInlineConstraints,
    pub border_metrics: TableInlineBorderMetrics,
    pub caption_min: CaptionMinContribution,
    pub track_visibility: TableTrackVisibility,
}

impl<'a> TableInlineSizingInput<'a> {
    fn collapsed_outer_inline_overflow(&self) -> (f32, f32) {
        match self.border_metrics {
            TableInlineBorderMetrics::Collapsed(metrics) => {
                (metrics.outer_start, metrics.outer_end)
            },
            TableInlineBorderMetrics::Separated(_)
            | TableInlineBorderMetrics::CollapsedPendingK4g => (0.0, 0.0),
        }
    }

    /// The basis for the table box's own padding percentage. CSS resolves it
    /// against the table's containing block, never against its used width.
    pub fn table_padding_basis(&self) -> Result<f32, TableInlineSizingError> {
        let offsets = match self.border_metrics {
            TableInlineBorderMetrics::Separated(metrics) => metrics.table_offsets,
            TableInlineBorderMetrics::Collapsed(metrics) => metrics.table_padding,
            TableInlineBorderMetrics::CollapsedPendingK4g => {
                return Err(TableInlineSizingError::Deferral(
                    TableDeferral::CollapsedBorderMetricsPendingK4g,
                ));
            },
        };
        match self.available_inline_size {
            Some(basis) => Ok(basis),
            None if !offsets.needs_percentage_basis() => Ok(0.0),
            None => Err(TableInlineSizingError::Deferral(
                TableDeferral::PercentagePaddingPendingBasis,
            )),
        }
    }

    /// The table geometry outside distributable column tracks under the
    /// selected border model.
    pub fn undistributable_inline_size(&self) -> Result<f32, TableInlineSizingError> {
        if !self.track_visibility.matches_grid(self.grid) {
            return Err(TableInlineSizingError::TrackVisibilityShape);
        }
        let basis = self.table_padding_basis()?;
        match self.border_metrics {
            TableInlineBorderMetrics::Separated(metrics) => metrics
                .undistributable_inline_size(self.grid.columns.len(), basis)
                .ok_or(TableInlineSizingError::InvalidBorderMetrics),
            TableInlineBorderMetrics::Collapsed(metrics) => metrics
                .undistributable_inline_size(basis)
                .ok_or(TableInlineSizingError::InvalidBorderMetrics),
            TableInlineBorderMetrics::CollapsedPendingK4g => Err(TableInlineSizingError::Deferral(
                TableDeferral::CollapsedBorderMetricsPendingK4g,
            )),
        }
    }
}

/// Style-lowered cell constraints supplied by the Livery adapter. The adapter
/// maps physical edges to these logical fields before Buckram sees them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellInlineStyle {
    pub constraints: TableInlineConstraints,
    pub offsets: CellInlineOffsets,
}

/// One cell's intrinsic content pair and the CSS constraints that apply to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableCellInlineMeasure {
    pub box_id: BoxId,
    pub content: IntrinsicSizes,
    pub preferred: InlineSizeConstraint,
    pub minimum: InlineSizeConstraint,
    pub maximum: InlineSizeConstraint,
    pub box_sizing: TableBoxSizing,
    pub offsets: CellInlineOffsets,
}

impl TableCellInlineMeasure {
    pub fn outer_content_sizes(self) -> Result<IntrinsicSizes, TableInlineSizingError> {
        // An intrinsic contribution is measured before any table width exists.
        if self.offsets.needs_percentage_basis() {
            return Err(TableInlineSizingError::Deferral(
                TableDeferral::PercentagePaddingPendingBasis,
            ));
        }
        let Some(offsets) = self.offsets.absolute_total() else {
            return Err(TableInlineSizingError::InvalidOffsets {
                box_id: self.box_id,
            });
        };
        IntrinsicSizes::new(
            self.content.min_content + offsets,
            self.content.max_content + offsets,
        )
        .ok_or(TableInlineSizingError::InvalidIntrinsicPair {
            box_id: self.box_id,
        })
    }

    /// Apply the currently-definite min/max constraints without resolving a
    /// percentage against a guessed table width. K4c3 decides how intrinsic
    /// keywords and `fit-content()` affect column measures.
    pub fn clamp_content_contribution(
        self,
        content: f32,
        percentage_basis: Option<f32>,
    ) -> Result<f32, TableInlineSizingError> {
        if !content.is_finite() || content < 0.0 {
            return Err(TableInlineSizingError::InvalidContentContribution {
                box_id: self.box_id,
            });
        }
        let minimum = self.minimum.resolve_definite(
            percentage_basis,
            Some(self.box_id),
            TableInlineProperty::MinWidth,
        )?;
        let maximum = self.maximum.resolve_definite(
            percentage_basis,
            Some(self.box_id),
            TableInlineProperty::MaxWidth,
        )?;
        let minimum = minimum.unwrap_or(0.0);
        let maximum = maximum.unwrap_or(f32::INFINITY).max(minimum);
        Ok(content.max(minimum).min(maximum))
    }
}

/// A later K4c gate supplies one result per K4b column. This constructor keeps
/// the aggregate invariant observable before the algorithms arrive.
#[derive(Clone, Debug, PartialEq)]
pub struct TableInlineSizingResult {
    pub intrinsic_sizes: IntrinsicSizes,
    pub used_table_inline_size: f32,
    pub used_grid_inline_size: f32,
    /// The portion of the used grid width assigned to K4b column tracks.
    /// This is intentionally distinct from table borders, padding, and
    /// separated border spacing.
    pub assignable_column_inline_size: f32,
    /// Table border, padding, and separated spacing outside the column tracks.
    pub undistributable_inline_size: f32,
    /// K4g3's accepted outer inline winners spill half their width outside
    /// the grid border box. Separated tables have zero spill.
    pub overflow_inline_start: f32,
    pub overflow_inline_end: f32,
    pub column_sizes: Vec<f32>,
}

impl TableInlineSizingResult {
    pub const SUBPIXEL_TOLERANCE: f32 = 0.01;

    pub fn new(
        input: &TableInlineSizingInput<'_>,
        intrinsic_sizes: IntrinsicSizes,
        used_table_inline_size: f32,
        used_grid_inline_size: f32,
        column_sizes: Vec<f32>,
    ) -> Result<Self, TableInlineSizingError> {
        if column_sizes.len() != input.grid.columns.len() {
            return Err(TableInlineSizingError::ColumnCountMismatch {
                expected: input.grid.columns.len(),
                actual: column_sizes.len(),
            });
        }
        let (overflow_inline_start, overflow_inline_end) = input.collapsed_outer_inline_overflow();
        if !intrinsic_sizes.min_content.is_finite()
            || !intrinsic_sizes.max_content.is_finite()
            || intrinsic_sizes.min_content < 0.0
            || intrinsic_sizes.max_content < intrinsic_sizes.min_content
            || !used_table_inline_size.is_finite()
            || used_table_inline_size < 0.0
            || !used_grid_inline_size.is_finite()
            || used_grid_inline_size < 0.0
            || column_sizes
                .iter()
                .any(|size| !size.is_finite() || *size < 0.0)
            || !overflow_inline_start.is_finite()
            || overflow_inline_start < 0.0
            || !overflow_inline_end.is_finite()
            || overflow_inline_end < 0.0
        {
            return Err(TableInlineSizingError::InvalidResultSize);
        }
        let assignable_column_inline_size = column_sizes.iter().sum::<f32>();
        let undistributable_inline_size = input.undistributable_inline_size()?;
        let expected_grid = assignable_column_inline_size + undistributable_inline_size;
        // The columns and the undistributable remainder account for the whole
        // used grid width only when the grid has tracks. A table with no
        // columns still has whatever width its own `width` asked for, and
        // there is no track that could have absorbed the difference, so
        // requiring the two to agree would reject an empty table outright.
        if !input.grid.columns.is_empty()
            && (expected_grid - used_grid_inline_size).abs() > Self::SUBPIXEL_TOLERANCE
        {
            return Err(TableInlineSizingError::GridSizeMismatch {
                expected: expected_grid,
                actual: used_grid_inline_size,
            });
        }
        Ok(Self {
            intrinsic_sizes,
            used_table_inline_size,
            used_grid_inline_size,
            assignable_column_inline_size,
            undistributable_inline_size,
            overflow_inline_start,
            overflow_inline_end,
            column_sizes,
        })
    }
}

/// An adapter measures one complete intrinsic pair for a Buckram box identity.
/// It cannot receive a DOM node, Taffy node, table track, or completed fragment.
pub trait TableIntrinsicMeasureProvider {
    fn measure_intrinsic_inline(
        &mut self,
        query: IntrinsicSizeQuery,
    ) -> Result<IntrinsicSizes, IntrinsicQueryError>;
}

/// Fetch both intrinsic inline sizes through the shared box-keyed cache. A
/// failure always clears the pending reservation without entering the cache.
pub fn query_table_cell_inline_sizes(
    cache: &mut IntrinsicSizeCache,
    box_id: BoxId,
    provider: &mut impl TableIntrinsicMeasureProvider,
) -> Result<IntrinsicSizes, TableInlineSizingError> {
    let min_query =
        IntrinsicSizeQuery::new(box_id, LogicalAxis::Inline, IntrinsicSizeKind::MinContent);
    let max_query =
        IntrinsicSizeQuery::new(box_id, LogicalAxis::Inline, IntrinsicSizeKind::MaxContent);
    match cache
        .begin_query(min_query)
        .map_err(TableInlineSizingError::Intrinsic)?
    {
        IntrinsicQueryState::Cached(min_content) => {
            let max_content = cache
                .get(max_query)
                .ok_or(TableInlineSizingError::InvalidIntrinsicPair { box_id })?;
            IntrinsicSizes::new(min_content, max_content)
                .ok_or(TableInlineSizingError::InvalidIntrinsicPair { box_id })
        },
        IntrinsicQueryState::Pending => {
            let sizes = provider.measure_intrinsic_inline(min_query);
            cache
                .finish_query(min_query, sizes)
                .map_err(TableInlineSizingError::Intrinsic)?;
            let min_content = cache
                .get(min_query)
                .ok_or(TableInlineSizingError::InvalidIntrinsicPair { box_id })?;
            let max_content = cache
                .get(max_query)
                .ok_or(TableInlineSizingError::InvalidIntrinsicPair { box_id })?;
            IntrinsicSizes::new(min_content, max_content)
                .ok_or(TableInlineSizingError::InvalidIntrinsicPair { box_id })
        },
    }
}

/// Collect K4b cells in topology order. The style callback is intentionally
/// keyed only by `BoxId`, which makes the adapter boundary testable without
/// backend layout state.
pub fn collect_table_cell_inline_measures(
    input: &TableInlineSizingInput<'_>,
    cache: &mut IntrinsicSizeCache,
    provider: &mut impl TableIntrinsicMeasureProvider,
    mut style_for: impl FnMut(BoxId) -> Result<TableCellInlineStyle, TableInlineSizingError>,
) -> Result<Vec<TableCellInlineMeasure>, TableInlineSizingError> {
    if !input.track_visibility.matches_grid(input.grid) {
        return Err(TableInlineSizingError::TrackVisibilityShape);
    }
    input.caption_min.measured()?;
    let mut measures = Vec::with_capacity(input.grid.cells.len());
    for cell in &input.grid.cells {
        let content = query_table_cell_inline_sizes(cache, cell.source, provider)?;
        let style = style_for(cell.source)?;
        if !style.offsets.is_valid() {
            return Err(TableInlineSizingError::InvalidOffsets {
                box_id: cell.source,
            });
        }
        measures.push(TableCellInlineMeasure {
            box_id: cell.source,
            content,
            preferred: style.constraints.preferred,
            minimum: style.constraints.minimum,
            maximum: style.constraints.maximum,
            box_sizing: style.constraints.box_sizing,
            offsets: style.offsets,
        });
    }
    Ok(measures)
}

/// The property which made a table-sizing outcome unresolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableInlineProperty {
    Width,
    MinWidth,
    MaxWidth,
    PaddingInlineStart,
    PaddingInlineEnd,
}

/// K4c errors name the missing CSS information instead of fabricating a width.
#[derive(Clone, Debug, PartialEq)]
pub enum TableInlineSizingError {
    Intrinsic(IntrinsicQueryError),
    UnresolvedPercentageBasis {
        box_id: Option<BoxId>,
        property: TableInlineProperty,
    },
    UnreducedConstraint {
        box_id: Option<BoxId>,
        property: TableInlineProperty,
    },
    InvalidConstraint {
        box_id: Option<BoxId>,
        property: TableInlineProperty,
    },
    InvalidOffsets {
        box_id: BoxId,
    },
    InvalidCaptionMinimum,
    InvalidContentContribution {
        box_id: BoxId,
    },
    InvalidIntrinsicPair {
        box_id: BoxId,
    },
    InvalidBorderMetrics,
    TrackVisibilityShape,
    FixedColumnInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    FixedColumnSourceMismatch {
        index: usize,
        expected: Option<BoxId>,
        actual: Option<BoxId>,
    },
    FixedColumnGroupInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    FixedColumnGroupSourceMismatch {
        index: usize,
        expected: BoxId,
        actual: BoxId,
    },
    FixedCellInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    FixedCellSourceMismatch {
        index: usize,
        expected: BoxId,
        actual: BoxId,
    },
    AutomaticColumnInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    AutomaticColumnSourceMismatch {
        index: usize,
        expected: Option<BoxId>,
        actual: Option<BoxId>,
    },
    AutomaticColumnGroupInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    AutomaticColumnGroupSourceMismatch {
        index: usize,
        expected: BoxId,
        actual: BoxId,
    },
    AutomaticCellInputCountMismatch {
        expected: usize,
        actual: usize,
    },
    AutomaticCellSourceMismatch {
        index: usize,
        expected: BoxId,
        actual: BoxId,
    },
    InvalidColumnGroupRange {
        start: usize,
        span: usize,
    },
    FixedLayoutWithoutColumns,
    InvalidColumnMeasure,
    ColumnCountMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidResultSize,
    GridSizeMismatch {
        expected: f32,
        actual: f32,
    },
    Deferral(TableDeferral),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside, DisplayOutside,
        DisplayRole, FlowAxes, InternalTableRole, PositioningScheme, generate_box_tree,
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

    fn sample_grid() -> (TableGrid, BoxId) {
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
                InternalTableRole::Row,
                vec![node(3, InternalTableRole::Cell, vec![])],
            )],
        )]);
        let grid = tree.principal_box(1).expect("table grid");
        let cell = tree.principal_box(3).expect("cell");
        (
            TableGrid::from_box_tree(&tree, grid, &super::super::TableGridInputs::default()),
            cell,
        )
    }

    fn input(grid: &TableGrid) -> TableInlineSizingInput<'_> {
        TableInlineSizingInput {
            grid,
            available_inline_size: None,
            table_constraints: TableInlineConstraints::default(),
            border_metrics: TableInlineBorderMetrics::Separated(
                TableSeparatedBorderMetrics::default(),
            ),
            caption_min: CaptionMinContribution::NoCaption,
            track_visibility: TableTrackVisibility::all_visible(grid),
        }
    }

    fn style() -> TableCellInlineStyle {
        TableCellInlineStyle {
            constraints: TableInlineConstraints::default(),
            offsets: CellInlineOffsets::ZERO,
        }
    }

    #[test]
    fn affine_constraints_keep_percentages_unresolved_until_a_basis_exists() {
        let affine = AffineLengthPercentage::new(12.0, 0.4).expect("finite affine value");
        let measure = TableCellInlineMeasure {
            box_id: sample_grid().1,
            content: IntrinsicSizes::new(10.0, 20.0).expect("valid intrinsic pair"),
            preferred: InlineSizeConstraint::Value(affine),
            minimum: InlineSizeConstraint::Value(affine),
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::ContentBox,
            offsets: CellInlineOffsets::ZERO,
        };

        assert!(measure.minimum.needs_percentage_basis());
        assert_eq!(
            measure.clamp_content_contribution(10.0, None),
            Err(TableInlineSizingError::UnresolvedPercentageBasis {
                box_id: Some(measure.box_id),
                property: TableInlineProperty::MinWidth,
            })
        );
        assert_eq!(
            measure.clamp_content_contribution(10.0, Some(50.0)),
            Ok(32.0)
        );
    }

    #[test]
    fn content_and_border_box_offsets_remain_distinct() {
        let offsets = CellInlineOffsets {
            padding_start: AffineLengthPercentage::px(2.0),
            padding_end: AffineLengthPercentage::px(3.0),
            border_start: 4.0,
            border_end: 5.0,
        };
        let measure = TableCellInlineMeasure {
            box_id: sample_grid().1,
            content: IntrinsicSizes::new(0.0, 20.0).expect("valid zero-width pair"),
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Auto,
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::BorderBox,
            offsets,
        };

        assert_eq!(
            measure.outer_content_sizes(),
            Ok(IntrinsicSizes::new(14.0, 34.0).expect("valid outer pair"))
        );
        assert_eq!(measure.box_sizing, TableBoxSizing::BorderBox);
    }

    #[test]
    fn min_and_max_constraints_clamp_without_swapping_intrinsic_sizes() {
        let box_id = sample_grid().1;
        let measure = TableCellInlineMeasure {
            box_id,
            content: IntrinsicSizes::new(10.0, 60.0).expect("ordered intrinsic pair"),
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Value(
                AffineLengthPercentage::new(30.0, 0.0).expect("finite minimum"),
            ),
            maximum: InlineSizeConstraint::Value(
                AffineLengthPercentage::new(50.0, 0.0).expect("finite maximum"),
            ),
            box_sizing: TableBoxSizing::ContentBox,
            offsets: CellInlineOffsets::ZERO,
        };

        assert_eq!(measure.clamp_content_contribution(10.0, None), Ok(30.0));
        assert_eq!(measure.clamp_content_contribution(60.0, None), Ok(50.0));
        assert_eq!(IntrinsicSizes::new(60.0, 10.0), None);
    }

    #[test]
    fn logical_offsets_are_identical_for_ltr_and_rtl() {
        let offsets = CellInlineOffsets {
            padding_start: AffineLengthPercentage::px(1.0),
            padding_end: AffineLengthPercentage::px(2.0),
            border_start: 3.0,
            border_end: 4.0,
        };
        assert_eq!(
            FlowAxes::HORIZONTAL_LTR.inline_start(),
            crate::PhysicalSide::Left
        );
        assert_eq!(
            FlowAxes::new(crate::WritingMode::HorizontalTb, crate::Direction::Rtl).inline_start(),
            crate::PhysicalSide::Right
        );
        assert_eq!(offsets.total(0.0), Some(10.0));
    }

    #[test]
    fn an_intrinsic_contribution_defers_a_padding_percentage_instead_of_sampling_zero() {
        let measure = TableCellInlineMeasure {
            box_id: sample_grid().1,
            content: IntrinsicSizes::new(10.0, 20.0).expect("valid intrinsic pair"),
            preferred: InlineSizeConstraint::Auto,
            minimum: InlineSizeConstraint::Auto,
            maximum: InlineSizeConstraint::None,
            box_sizing: TableBoxSizing::ContentBox,
            offsets: CellInlineOffsets {
                padding_start: AffineLengthPercentage::new(0.0, 0.1).expect("finite percentage"),
                ..CellInlineOffsets::ZERO
            },
        };
        assert_eq!(
            measure.outer_content_sizes(),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::PercentagePaddingPendingBasis
            ))
        );
        // The same offsets resolve once a basis exists.
        assert_eq!(measure.offsets.total(200.0), Some(20.0));
    }

    #[test]
    fn separated_spacing_uses_k4b_column_count_and_collapsed_metrics_defer() {
        let (grid, _) = sample_grid();
        let mut input = input(&grid);
        input.border_metrics = TableInlineBorderMetrics::Separated(TableSeparatedBorderMetrics {
            table_offsets: CellInlineOffsets {
                padding_start: AffineLengthPercentage::px(1.0),
                padding_end: AffineLengthPercentage::px(2.0),
                border_start: 3.0,
                border_end: 4.0,
            },
            inline_spacing: 5.0,
        });
        assert_eq!(input.undistributable_inline_size(), Ok(20.0));

        input.border_metrics = TableInlineBorderMetrics::CollapsedPendingK4g;
        assert_eq!(
            input.undistributable_inline_size(),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::CollapsedBorderMetricsPendingK4g
            ))
        );
    }

    #[derive(Default)]
    struct RecordingProvider {
        queries: Vec<IntrinsicSizeQuery>,
        result: Option<IntrinsicSizes>,
        failure: Option<IntrinsicQueryError>,
    }

    impl TableIntrinsicMeasureProvider for RecordingProvider {
        fn measure_intrinsic_inline(
            &mut self,
            query: IntrinsicSizeQuery,
        ) -> Result<IntrinsicSizes, IntrinsicQueryError> {
            self.queries.push(query);
            match (self.result, self.failure) {
                (_, Some(failure)) => Err(failure),
                (Some(result), None) => Ok(result),
                (None, None) => panic!("fixture must supply a result"),
            }
        }
    }

    #[test]
    fn one_box_identity_query_caches_its_complete_min_max_pair() {
        let box_id = sample_grid().1;
        let mut provider = RecordingProvider {
            result: IntrinsicSizes::new(13.0, 29.0),
            ..Default::default()
        };
        let mut cache = IntrinsicSizeCache::default();

        assert_eq!(
            query_table_cell_inline_sizes(&mut cache, box_id, &mut provider),
            Ok(IntrinsicSizes::new(13.0, 29.0).expect("valid measured pair"))
        );
        assert_eq!(provider.queries.len(), 1);
        assert_eq!(provider.queries[0].box_id, box_id);
        assert_eq!(provider.queries[0].axis, LogicalAxis::Inline);
        assert_eq!(provider.queries[0].kind, IntrinsicSizeKind::MinContent);
        assert_eq!(
            query_table_cell_inline_sizes(&mut cache, box_id, &mut provider),
            Ok(IntrinsicSizes::new(13.0, 29.0).expect("valid cached pair"))
        );
        assert_eq!(provider.queries.len(), 1);

        cache.invalidate(box_id);
        assert_eq!(
            query_table_cell_inline_sizes(&mut cache, box_id, &mut provider),
            Ok(IntrinsicSizes::new(13.0, 29.0).expect("valid remeasured pair"))
        );
        assert_eq!(provider.queries.len(), 2);
    }

    #[test]
    fn intrinsic_failures_and_cycles_never_enter_the_cache() {
        let box_id = sample_grid().1;
        let query =
            IntrinsicSizeQuery::new(box_id, LogicalAxis::Inline, IntrinsicSizeKind::MinContent);
        let mut cache = IntrinsicSizeCache::default();
        let failure = IntrinsicQueryError::IndefiniteContainingSize(query);
        let mut provider = RecordingProvider {
            failure: Some(failure),
            ..Default::default()
        };

        assert_eq!(
            query_table_cell_inline_sizes(&mut cache, box_id, &mut provider),
            Err(TableInlineSizingError::Intrinsic(failure))
        );
        assert!(cache.is_empty());
        assert_eq!(cache.begin_query(query), Ok(IntrinsicQueryState::Pending));
        assert_eq!(
            query_table_cell_inline_sizes(&mut cache, box_id, &mut provider),
            Err(TableInlineSizingError::Intrinsic(
                IntrinsicQueryError::Cycle(query)
            ))
        );
        assert!(cache.is_empty());
        assert_eq!(cache.finish_query(query, Err(failure)), Err(failure));
    }

    #[test]
    fn adapter_measurement_uses_box_identity_without_backend_layout_state() {
        let (grid, cell) = sample_grid();
        let input = input(&grid);
        let mut cache = IntrinsicSizeCache::default();
        let mut provider = RecordingProvider {
            result: IntrinsicSizes::new(8.0, 24.0),
            ..Default::default()
        };
        let measures =
            collect_table_cell_inline_measures(&input, &mut cache, &mut provider, |box_id| {
                assert_eq!(box_id, cell);
                Ok(style())
            })
            .expect("box-keyed measurement");

        assert_eq!(measures.len(), 1);
        assert_eq!(measures[0].box_id, cell);
        assert_eq!(measures[0].content, IntrinsicSizes::new(8.0, 24.0).unwrap());
    }

    #[test]
    fn pending_caption_and_invalid_sizes_are_explicit() {
        let (grid, cell) = sample_grid();
        let mut sizing_input = input(&grid);
        sizing_input.caption_min = CaptionMinContribution::PendingK4e;
        let mut cache = IntrinsicSizeCache::default();
        let mut provider = RecordingProvider {
            result: IntrinsicSizes::new(1.0, 1.0),
            ..Default::default()
        };
        assert_eq!(
            collect_table_cell_inline_measures(&sizing_input, &mut cache, &mut provider, |_| Ok(
                style()
            ),),
            Err(TableInlineSizingError::Deferral(
                TableDeferral::CaptionMinPendingK4e
            ))
        );
        assert_eq!(AffineLengthPercentage::new(f32::NAN, 0.0), None);
        assert_eq!(
            CellInlineOffsets {
                padding_start: AffineLengthPercentage::px(-1.0),
                ..CellInlineOffsets::ZERO
            }
            .total(0.0),
            None
        );
        let valid_input = input(&grid);
        assert_eq!(
            TableInlineSizingResult::new(
                &valid_input,
                IntrinsicSizes {
                    min_content: 20.0,
                    max_content: 10.0,
                },
                10.0,
                10.0,
                vec![10.0],
            ),
            Err(TableInlineSizingError::InvalidResultSize)
        );
        assert_eq!(cell, grid.cells[0].source);
    }
}
