//! The complete block-axis table pipeline.
//!
//! K4d1 through K4d6a each accepted one phase of table row layout as its own
//! function, so each could be gated on its own evidence. Running them in the
//! right order is part of the algorithm, not adapter policy: an adapter that
//! sized rows before applying baseline minima, or aligned against the
//! first-pass sizing rather than the percentage pass's, would silently
//! produce a different table. This module owns that order once, so every
//! adapter gets the same one.

use crate::{
    BoxId,
    table::{
        TableAlignment, TableBlockConstraint, TableBlockSizingInput, TableCellBlockStyle,
        TableCellFormatter, TableCellLayoutOutput, TableFragments, TableRowLayoutError,
        TableRowSizing, align_table_cells, apply_baseline_row_minima, emit_table_fragments,
        format_table_cells, measure_single_span_rows, resolve_percentage_block_sizes,
        size_table_rows,
    },
};

/// One table's complete block-axis result.
#[derive(Clone, Debug, PartialEq)]
pub struct TableBlockLayout {
    pub sizing: TableRowSizing,
    pub alignment: TableAlignment,
    /// Every cell's final formatting output, in K4b cell order. A cell the
    /// percentage pass relaid out carries its second-pass output here, never
    /// its first.
    pub cell_outputs: Vec<(BoxId, TableCellLayoutOutput)>,
    /// Cells the percentage pass relaid out, in K4b cell order. Every other
    /// cell was formatted exactly once.
    pub relaid_out: Vec<BoxId>,
    pub fragments: TableFragments,
}

/// Lay out one table's block axis, from cell formatting through the emitted
/// fragment subtree.
///
/// The order is load-bearing at two points. Baseline minima are a genuine row
/// minimum that content measurement cannot see, so they are applied before
/// rows are sized. The percentage pass may then grow rows again, so alignment
/// and fragment emission both read its sizing rather than the first pass's.
///
/// `resolved_offsets_of` supplies each cell's resolved inline offsets by K4b
/// cell index; the basis is real once the accepted inline result exists.
pub fn layout_table_block(
    input: &TableBlockSizingInput<'_>,
    cell_styles: &[TableCellBlockStyle],
    row_constraints: &[TableBlockConstraint],
    inline_spacing: f32,
    resolved_offsets_of: impl FnMut(usize, BoxId) -> f32,
    formatter: &mut impl TableCellFormatter,
) -> Result<TableBlockLayout, TableRowLayoutError> {
    let mut cell_outputs =
        format_table_cells(input, inline_spacing, resolved_offsets_of, formatter)?;
    let mut measures =
        measure_single_span_rows(input, cell_styles, &cell_outputs, row_constraints)?;
    apply_baseline_row_minima(input, cell_styles, &cell_outputs, &mut measures)?;
    let first_pass = size_table_rows(input, &measures, cell_styles, &cell_outputs)?;
    let percentage = resolve_percentage_block_sizes(
        input,
        &first_pass,
        &measures,
        cell_styles,
        &mut cell_outputs,
        row_constraints,
        formatter,
    )?;
    let alignment = align_table_cells(
        input,
        &percentage.sizing,
        cell_styles,
        &cell_outputs,
        inline_spacing,
    )?;
    let fragments = emit_table_fragments(
        input,
        &percentage.sizing,
        &alignment,
        &cell_outputs,
        inline_spacing,
    )?;
    Ok(TableBlockLayout {
        sizing: percentage.sizing,
        alignment,
        cell_outputs,
        relaid_out: percentage.relaid_out,
        fragments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Baselines, BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, DisplayInside,
        DisplayOutside, DisplayRole, FlowAxes, InternalTableRole, IntrinsicSizes, LogicalRect,
        PositioningScheme, generate_box_tree,
        table::{
            AffineLengthPercentage, CaptionMinContribution, FragmentDraftTree,
            TableBlockBorderMetrics, TableCellAlignment, TableCellLayoutInput, TableCellLayoutPass,
            TableFragmentRole, TableGrid, TableGridInputs, TableInlineBorderMetrics,
            TableInlineConstraints, TableInlineSizingInput, TableInlineSizingResult,
            TableSeparatedBlockMetrics, TableSeparatedBorderMetrics, TableTrackVisibility,
        },
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

    fn leaf(id: u8, role: InternalTableRole) -> BoxTreeInput<u8> {
        BoxTreeInput::new(
            BoxOrigin::Element(id),
            table_role(role),
            FlowAxes::HORIZONTAL_LTR,
            PositioningScheme::Static,
            false,
            vec![],
        )
    }

    /// One explicit row group, as `<tbody>` supplies in real markup: rows in
    /// separate anonymous groups would clamp every span to one row.
    fn grid(rows: &[&[u8]]) -> TableGrid {
        let row_inputs = rows
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                BoxTreeInput::new(
                    BoxOrigin::Element(100 + index as u8),
                    table_role(InternalTableRole::Row),
                    FlowAxes::HORIZONTAL_LTR,
                    PositioningScheme::Static,
                    false,
                    cells
                        .iter()
                        .map(|id| leaf(*id, InternalTableRole::Cell))
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
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
            vec![BoxTreeInput::new(
                BoxOrigin::Element(90),
                table_role(InternalTableRole::RowGroup),
                FlowAxes::HORIZONTAL_LTR,
                PositioningScheme::Static,
                false,
                row_inputs,
            )],
        )]);
        TableGrid::from_box_tree(
            &tree,
            tree.principal_box(1).expect("table grid"),
            &TableGridInputs::default(),
        )
    }

    fn inline_result(grid: &TableGrid, columns: Vec<f32>) -> TableInlineSizingResult {
        let total: f32 = columns.iter().sum();
        let sizing = TableInlineSizingInput {
            grid,
            available_inline_size: Some(total),
            table_constraints: TableInlineConstraints::default(),
            border_metrics: TableInlineBorderMetrics::Separated(
                TableSeparatedBorderMetrics::default(),
            ),
            caption_min: CaptionMinContribution::NoCaption,
            track_visibility: TableTrackVisibility::all_visible(grid),
        };
        TableInlineSizingResult::new(
            &sizing,
            IntrinsicSizes::new(total, total).expect("intrinsic pair"),
            total,
            total,
            columns,
        )
        .expect("reconciled inline result")
    }

    fn px(value: f32) -> TableBlockConstraint {
        TableBlockConstraint::Value(AffineLengthPercentage::px(value))
    }

    /// A percentage constraint, as a fraction: `0.6` is `60%`.
    fn percent(fraction: f32) -> TableBlockConstraint {
        TableBlockConstraint::Value(
            AffineLengthPercentage::new(0.0, fraction).expect("percentage constraint"),
        )
    }

    /// A formatter whose per-cell content height and baseline are scripted by
    /// K4b cell index, and which reports every request it received.
    struct ScriptedFormatter {
        /// Content block size and optional baseline per cell.
        cells: Vec<(f32, Option<f32>)>,
        /// Content block size to report on a percentage-pass reformat.
        second_pass: f32,
        requests: Vec<TableCellLayoutInput>,
    }

    impl TableCellFormatter for ScriptedFormatter {
        fn format_cell(
            &mut self,
            input: TableCellLayoutInput,
        ) -> Result<TableCellLayoutOutput, TableRowLayoutError> {
            self.requests.push(input);
            let index = self
                .requests
                .iter()
                .filter(|request| request.pass == TableCellLayoutPass::Measure)
                .position(|request| request.box_id == input.box_id)
                .unwrap_or(0);
            let (content, baseline) = self.cells.get(index).copied().unwrap_or((10.0, None));
            let content = match input.pass {
                TableCellLayoutPass::Measure => content,
                TableCellLayoutPass::ResolvePercentages { .. } => self.second_pass,
            };
            Ok(TableCellLayoutOutput {
                content_block_size: content,
                border_box_min_block_size: 0.0,
                baselines: Baselines::new(baseline, baseline)
                    .unwrap_or(Baselines::synthesized_from_block_end(content)),
                overflow: LogicalRect::default(),
                fragments: FragmentDraftTree::default(),
            })
        }
    }

    struct Case {
        grid: TableGrid,
        inline: TableInlineSizingResult,
    }

    impl Case {
        fn new(rows: &[&[u8]], columns: Vec<f32>) -> Self {
            let grid = grid(rows);
            let inline = inline_result(&grid, columns);
            Self { grid, inline }
        }

        fn input(&self, table_constraint: TableBlockConstraint) -> TableBlockSizingInput<'_> {
            TableBlockSizingInput {
                grid: &self.grid,
                inline: &self.inline,
                table_constraint,
                border_metrics: TableBlockBorderMetrics::Separated(
                    TableSeparatedBlockMetrics::default(),
                ),
                available_block_size: None,
                track_visibility: TableTrackVisibility::all_visible(&self.grid),
            }
        }
    }

    /// CSS 2.1 section 17.5.3: aligning baselines can make a row taller than
    /// its tallest cell. Content measurement cannot see that growth, so
    /// sizing rows before applying it would leave the row at 50 rather than
    /// 70. The driver owns that order.
    #[test]
    fn baseline_minima_reach_row_sizing() {
        // Cell A: 50 tall, baseline at 10, so 40 below it.
        // Cell B: 40 tall, baseline at 30, so 10 below it.
        // The shared baseline is 30, needing 30 above and 40 below: 70.
        let case = Case::new(&[&[3, 4]], vec![100.0, 100.0]);
        let input = case.input(TableBlockConstraint::Auto);
        let styles = vec![TableCellBlockStyle::default(); 2];
        let mut formatter = ScriptedFormatter {
            cells: vec![(50.0, Some(10.0)), (40.0, Some(30.0))],
            second_pass: 0.0,
            requests: Vec::new(),
        };
        let layout = layout_table_block(
            &input,
            &styles,
            &[TableBlockConstraint::Auto],
            0.0,
            |_, _| 0.0,
            &mut formatter,
        )
        .expect("table block layout");

        assert!(
            (layout.sizing.row_sizes[0] - 70.0).abs() < 0.05,
            "{layout:?}"
        );
        assert!((layout.alignment.rows[0].baseline - 30.0).abs() < 0.05);
        // The emitted row fragment carries the grown size, not the tallest
        // cell's 50.
        let row = layout
            .fragments
            .with_role(TableFragmentRole::Row)
            .next()
            .expect("row fragment");
        assert!((row.rect.block_size - 70.0).abs() < 0.05, "{row:?}");
    }

    /// The percentage pass may grow rows after the first sizing. Alignment
    /// and fragment emission must both read that sizing: aligning against the
    /// first pass would place every cell in a row shorter than the one
    /// painted.
    #[test]
    fn alignment_and_fragments_read_the_percentage_pass() {
        // A 300px table over two rows; the first is 60%, so 180px. Content is
        // only 10px tall, so nothing but the percentage can produce that.
        let case = Case::new(&[&[3], &[4]], vec![100.0]);
        let input = case.input(px(300.0));
        let styles = vec![
            TableCellBlockStyle {
                alignment: TableCellAlignment::Bottom,
                ..TableCellBlockStyle::default()
            };
            2
        ];
        let mut formatter = ScriptedFormatter {
            cells: vec![(10.0, None), (10.0, None)],
            second_pass: 10.0,
            requests: Vec::new(),
        };
        let layout = layout_table_block(
            &input,
            &styles,
            &[percent(0.6), TableBlockConstraint::Auto],
            0.0,
            |_, _| 0.0,
            &mut formatter,
        )
        .expect("table block layout");

        assert!(
            (layout.sizing.row_sizes[0] - 180.0).abs() < 0.05,
            "{:?}",
            layout.sizing
        );
        // A bottom-aligned 10px cell in a 180px row sits 170px down. Against
        // the first pass's content-height row it would sit at 0.
        assert!(
            (layout.alignment.cells[0].content_block_offset - 170.0).abs() < 0.05,
            "{:?}",
            layout.alignment.cells[0]
        );
        let row = layout
            .fragments
            .with_role(TableFragmentRole::Row)
            .next()
            .expect("row fragment");
        assert!((row.rect.block_size - 180.0).abs() < 0.05, "{row:?}");
    }

    /// A cell whose contents depend on its own block size is relaid out
    /// exactly once, and the driver returns that second-pass output rather
    /// than the measurement it replaced.
    #[test]
    fn a_relaid_out_cell_returns_its_second_pass_output() {
        let case = Case::new(&[&[3]], vec![100.0]);
        let input = case.input(px(200.0));
        let styles = vec![TableCellBlockStyle {
            percentage_dependent_contents: true,
            ..TableCellBlockStyle::default()
        }];
        let mut formatter = ScriptedFormatter {
            cells: vec![(10.0, None)],
            second_pass: 40.0,
            requests: Vec::new(),
        };
        let layout = layout_table_block(
            &input,
            &styles,
            &[TableBlockConstraint::Auto],
            0.0,
            |_, _| 0.0,
            &mut formatter,
        )
        .expect("table block layout");

        assert_eq!(layout.relaid_out.len(), 1);
        assert!(
            (layout.cell_outputs[0].1.content_block_size - 40.0).abs() < 0.05,
            "the first-pass output must be replaced: {:?}",
            layout.cell_outputs[0].1
        );
        // The second pass never re-drives row sizing: the table keeps the
        // 200px its own constraint fixed.
        assert!((layout.sizing.used_table_block_size - 200.0).abs() < 0.05);
    }

    /// Run one K4d4b interop case: `rows` gives each row's constraint and its
    /// single cell's `(constraint, content height)`. Returns the used row
    /// sizes and the table's used block size, which is what the browsers
    /// report.
    fn interop_case(
        table: TableBlockConstraint,
        block_spacing: f32,
        rows: &[(TableBlockConstraint, TableBlockConstraint, f32)],
    ) -> (Vec<f32>, f32) {
        let ids = (0..rows.len())
            .map(|i| vec![3u8 + i as u8])
            .collect::<Vec<_>>();
        let refs = ids.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let case = Case::new(&refs, vec![100.0]);
        let mut input = case.input(table);
        input.border_metrics = TableBlockBorderMetrics::Separated(TableSeparatedBlockMetrics {
            table_offset_start: 0.0,
            table_offset_end: 0.0,
            block_spacing,
        });
        let styles = rows
            .iter()
            .map(|(_, cell, _)| TableCellBlockStyle {
                specified: *cell,
                ..TableCellBlockStyle::default()
            })
            .collect::<Vec<_>>();
        let constraints = rows.iter().map(|(row, _, _)| *row).collect::<Vec<_>>();
        let mut formatter = ScriptedFormatter {
            cells: rows
                .iter()
                .map(|(_, _, content)| (*content, None))
                .collect(),
            second_pass: 0.0,
            requests: Vec::new(),
        };
        let layout = layout_table_block(
            &input,
            &styles,
            &constraints,
            0.0,
            |_, _| 0.0,
            &mut formatter,
        )
        .expect("table block layout");
        (layout.sizing.row_sizes, layout.sizing.used_table_block_size)
    }

    /// The K4d4b interop matrix, where Chrome 150 and Firefox 153 agree on
    /// all seven cases. K4d4 accepted the table's specified block size as the
    /// basis for a percentage row or cell height, which is right, but never
    /// covered a percentage cell in a row with a *definite* height. There the
    /// resolved growth has to be fitted back into the table's own height, or
    /// the table doubles.
    #[test]
    fn percentage_growth_fits_back_into_a_definite_table_height() {
        let auto = TableBlockConstraint::Auto;

        // Q1 is `table-as-item-cell-percentage-002`: two 50px rows whose
        // cells are 100% tall must stay 50, not grow to 100 each.
        let (rows, table) = interop_case(
            px(100.0),
            0.0,
            &[(px(50.0), percent(1.0), 0.0), (px(50.0), percent(1.0), 0.0)],
        );
        assert_eq!(rows, vec![50.0, 50.0], "Q1");
        assert_eq!(table, 100.0, "Q1 table");

        // Q2: one row of 50 in a 300px table still takes the whole table.
        let (rows, _) = interop_case(px(300.0), 0.0, &[(px(50.0), percent(1.0), 0.0)]);
        assert_eq!(rows, vec![300.0], "Q2");

        // Q3: no definite table height is no basis at all, so the row keeps
        // its own 80.
        let (rows, _) = interop_case(auto, 0.0, &[(px(80.0), percent(0.5), 0.0)]);
        assert_eq!(rows, vec![80.0], "Q3");

        // Q4 is K4d4's accepted control and must not move: an automatic row
        // whose cell is 50% of a 300px table.
        let (rows, _) = interop_case(
            px(300.0),
            0.0,
            &[(auto, percent(0.5), 20.0), (auto, auto, 40.0)],
        );
        assert_eq!(rows, vec![150.0, 150.0], "Q4");

        // Q5: the resolved 200 exceeds the row's own 20, and the table is
        // tall enough to hold it, so nothing shrinks.
        let (rows, _) = interop_case(
            px(400.0),
            0.0,
            &[(px(20.0), percent(0.5), 0.0), (auto, auto, 30.0)],
        );
        assert_eq!(rows, vec![200.0, 200.0], "Q5");

        // Q6 pins the proportion to the pre-distribution minima: floors of 50
        // and 10 against a demand of 200 and 10 give 190 and 10. Measuring
        // growth from the already-distributed first pass would give 95/105.
        let (rows, _) = interop_case(
            px(200.0),
            0.0,
            &[(percent(0.25), percent(1.0), 0.0), (auto, auto, 10.0)],
        );
        assert_eq!(rows, vec![190.0, 10.0], "Q6");

        // Q7: separated spacing leaves 180 distributable, and the row fills
        // exactly that.
        let (rows, table) = interop_case(px(200.0), 10.0, &[(px(60.0), percent(1.0), 0.0)]);
        assert_eq!(rows, vec![180.0], "Q7");
        assert_eq!(table, 200.0, "Q7 table");
    }

    /// Collapsed borders defer before any phase runs, so a deferral can never
    /// be read as a laid-out table.
    #[test]
    fn collapsed_metrics_defer_the_whole_pipeline() {
        let case = Case::new(&[&[3]], vec![100.0]);
        let mut input = case.input(TableBlockConstraint::Auto);
        input.border_metrics = TableBlockBorderMetrics::CollapsedPendingK4g;
        let mut formatter = ScriptedFormatter {
            cells: vec![(10.0, None)],
            second_pass: 0.0,
            requests: Vec::new(),
        };
        let error = layout_table_block(
            &input,
            &[TableCellBlockStyle::default()],
            &[TableBlockConstraint::Auto],
            0.0,
            |_, _| 0.0,
            &mut formatter,
        )
        .expect_err("collapsed metrics must defer");
        assert!(
            matches!(error, TableRowLayoutError::Deferral(_)),
            "{error:?}"
        );
        assert!(
            formatter.requests.is_empty(),
            "no cell may be formatted behind a deferral"
        );
    }
}
