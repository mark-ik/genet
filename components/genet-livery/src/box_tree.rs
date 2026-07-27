//! Livery computed-value and DOM adapter for Buckram's CSS box generator.
//!
//! Livery resolves computed values into box-generation input. Buckram owns
//! suppression, flattening, anonymous fixup, inline splitting, roles,
//! provenance, and tree relationships.

use std::{hash::Hash, ops::Deref};

use buckram::{
    BoxGeneration, BoxOrigin, BoxTreeInput, CssBoxTree, Direction, DisplayInside, DisplayOutside,
    DisplayRole, FloatSide, FlowAxes, InternalTableRole, PositioningScheme, WritingMode,
    generate_box_tree,
};
use layout_dom_api::{LayoutDom, NodeKind};
use livery::{
    ComputedValues,
    values::{
        Direction as ComputedDirection, Display as ComputedDisplay, Float as ComputedFloat,
        Position as ComputedPosition, WhiteSpaceCollapse, WritingMode as ComputedWritingMode,
    },
};

use crate::StylePlane;

/// Livery-generated, Buckram-normalized CSS boxes.
#[derive(Clone, Debug)]
pub(crate) struct GeneratedBoxTree<Id> {
    tree: CssBoxTree<Id>,
}

impl<Id> Deref for GeneratedBoxTree<Id> {
    type Target = CssBoxTree<Id>;

    fn deref(&self) -> &Self::Target {
        &self.tree
    }
}

impl<Id> GeneratedBoxTree<Id>
where
    Id: Copy + Eq + Hash,
{
    /// Resolve the host DOM into Buckram box-generation input.
    pub(crate) fn from_dom<D>(dom: &D, styles: &StylePlane<Id>) -> Self
    where
        D: LayoutDom<NodeId = Id>,
    {
        fn collect<D>(
            dom: &D,
            styles: &StylePlane<D::NodeId>,
            node: D::NodeId,
            inherited: Option<&ComputedValues>,
            output: &mut Vec<BoxTreeInput<D::NodeId>>,
        ) where
            D: LayoutDom,
            D::NodeId: Copy + Eq + Hash,
        {
            match dom.kind(node) {
                NodeKind::Document | NodeKind::DocumentFragment => {
                    for child in dom.dom_children(node) {
                        collect(dom, styles, child, inherited, output);
                    }
                },
                NodeKind::Element => {
                    let computed = styles.get(node).cloned().unwrap_or_default();
                    let mut children = Vec::new();
                    for child in dom.dom_children(node) {
                        collect(dom, styles, child, Some(&computed), &mut children);
                    }
                    output.push(
                        BoxTreeInput::new(
                            BoxOrigin::Element(node),
                            display_role(computed.display),
                            flow_axes(&computed),
                            positioning_scheme(computed.position),
                            is_replaced_element(dom, node),
                            children,
                        )
                        .with_float(float_side(computed.float)),
                    );
                },
                NodeKind::Text => {
                    let preserves_whitespace = inherited.is_some_and(|style| {
                        matches!(
                            style.white_space_collapse,
                            WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::BreakSpaces
                        )
                    });
                    let collapsible_whitespace = !preserves_whitespace
                        && dom.text(node).is_none_or(|text| {
                            text.chars()
                                .all(|ch| matches!(ch, ' ' | '\t' | '\n' | '\r' | '\u{c}'))
                        });
                    output.push(BoxTreeInput::text(
                        BoxOrigin::Text(node),
                        inherited.map(flow_axes).unwrap_or_default(),
                        collapsible_whitespace,
                    ));
                },
                _ => {},
            }
        }

        let mut roots = Vec::new();
        collect(dom, styles, dom.document(), None, &mut roots);
        Self {
            tree: generate_box_tree(roots),
        }
    }

    pub(crate) fn into_tree(self) -> CssBoxTree<Id> {
        self.tree
    }
}

fn display_role(display: ComputedDisplay) -> DisplayRole {
    let normal = |outside, inside| DisplayRole {
        generation: BoxGeneration::Normal,
        outside,
        inside,
        list_item: false,
        internal_table: None,
    };
    let internal = |role| DisplayRole {
        generation: BoxGeneration::Normal,
        outside: None,
        inside: None,
        list_item: false,
        internal_table: Some(role),
    };
    match display {
        ComputedDisplay::None => DisplayRole::NONE,
        ComputedDisplay::Contents => DisplayRole::CONTENTS,
        ComputedDisplay::Inline => DisplayRole::INLINE_FLOW,
        ComputedDisplay::Block => DisplayRole::BLOCK_FLOW,
        ComputedDisplay::ListItem => DisplayRole {
            list_item: true,
            ..DisplayRole::BLOCK_FLOW
        },
        ComputedDisplay::InlineBlock => {
            normal(Some(DisplayOutside::Inline), Some(DisplayInside::FlowRoot))
        },
        ComputedDisplay::Flex => normal(Some(DisplayOutside::Block), Some(DisplayInside::Flex)),
        ComputedDisplay::Grid => normal(Some(DisplayOutside::Block), Some(DisplayInside::Grid)),
        ComputedDisplay::Table => normal(Some(DisplayOutside::Block), Some(DisplayInside::Table)),
        ComputedDisplay::TableRowGroup => internal(InternalTableRole::RowGroup),
        ComputedDisplay::TableRow => internal(InternalTableRole::Row),
        ComputedDisplay::TableCell => internal(InternalTableRole::Cell),
        ComputedDisplay::TableCaption => internal(InternalTableRole::Caption),
    }
}

fn positioning_scheme(position: ComputedPosition) -> PositioningScheme {
    match position {
        ComputedPosition::Static => PositioningScheme::Static,
        ComputedPosition::Relative => PositioningScheme::Relative,
        ComputedPosition::Absolute => PositioningScheme::Absolute,
        ComputedPosition::Fixed => PositioningScheme::Fixed,
        ComputedPosition::Sticky => PositioningScheme::Sticky,
    }
}

fn float_side(float: ComputedFloat) -> FloatSide {
    match float {
        ComputedFloat::None => FloatSide::None,
        ComputedFloat::Left => FloatSide::Left,
        ComputedFloat::Right => FloatSide::Right,
    }
}

fn flow_axes(computed: &ComputedValues) -> FlowAxes {
    let writing_mode = match computed.writing_mode {
        ComputedWritingMode::HorizontalTb => WritingMode::HorizontalTb,
        ComputedWritingMode::VerticalRl => WritingMode::VerticalRl,
        ComputedWritingMode::VerticalLr => WritingMode::VerticalLr,
        ComputedWritingMode::SidewaysRl => WritingMode::SidewaysRl,
        ComputedWritingMode::SidewaysLr => WritingMode::SidewaysLr,
    };
    let direction = match computed.direction {
        ComputedDirection::Ltr => Direction::Ltr,
        ComputedDirection::Rtl => Direction::Rtl,
    };
    FlowAxes::new(writing_mode, direction)
}

fn is_replaced_element<D>(dom: &D, node: D::NodeId) -> bool
where
    D: LayoutDom,
{
    dom.element_name(node).is_some_and(|name| {
        name.local.as_ref().eq_ignore_ascii_case("img")
            || name.local.as_ref().eq_ignore_ascii_case("canvas")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, InteractionStates, StyleSet, resolve_styles};
    use genet_static_dom::StaticDocument;
    use layout_dom_api::{LocalName, Namespace};

    #[test]
    fn generated_tree_applies_suppression_contents_and_comment_rules() {
        fn find(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            needle: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.kind(node) == NodeKind::Element
                && dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(needle)
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| find(dom, child, needle))
        }

        let dom = StaticDocument::parse(
            "<html><body id=\"body\">before<!-- boundary --><span id=\"shown\">after</span>\
             <span id=\"hidden\">hidden</span><span id=\"contents\">\
             <b id=\"inside\">inside</b></span></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["#hidden { display: none; } #contents { display: contents; }"]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let generated = GeneratedBoxTree::from_dom(&dom, &styles);
        let body = find(&dom, dom.document(), "body").expect("body");
        let shown = find(&dom, dom.document(), "shown").expect("shown");
        let hidden = find(&dom, dom.document(), "hidden").expect("hidden");
        let contents = find(&dom, dom.document(), "contents").expect("contents");
        let inside = find(&dom, dom.document(), "inside").expect("inside");
        let body_box = generated.principal_box(body).expect("body principal box");
        let inside_box = generated
            .principal_box(inside)
            .expect("inside principal box");

        assert!(generated.principal_box(shown).is_some());
        assert_eq!(generated.principal_box(hidden), None);
        assert!(generated.boxes_for_node(hidden).is_empty());
        assert_eq!(generated.principal_box(contents), None);
        assert!(generated.boxes_for_node(contents).is_empty());
        assert_eq!(generated[inside_box].parent(), Some(body_box));
    }

    #[test]
    fn generated_boxes_carry_inherited_writing_mode_and_direction() {
        fn find(
            dom: &StaticDocument,
            node: <StaticDocument as LayoutDom>::NodeId,
            needle: &str,
        ) -> Option<<StaticDocument as LayoutDom>::NodeId> {
            if dom.kind(node) == NodeKind::Element
                && dom.attribute(node, &Namespace::from(""), &LocalName::from("id")) == Some(needle)
            {
                return Some(node);
            }
            dom.dom_children(node)
                .find_map(|child| find(dom, child, needle))
        }

        let dom = StaticDocument::parse(
            "<html><body id=\"body\"><span id=\"child\">word</span></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["#body { writing-mode: sideways-lr; direction: rtl; }"]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let generated = GeneratedBoxTree::from_dom(&dom, &styles);
        let body = find(&dom, dom.document(), "body").expect("body");
        let child = find(&dom, dom.document(), "child").expect("child");
        let text = dom
            .dom_children(child)
            .find(|node| dom.kind(*node) == NodeKind::Text)
            .expect("text");
        let expected = FlowAxes::new(WritingMode::SidewaysLr, Direction::Rtl);

        assert_eq!(
            generated[generated.principal_box(body).expect("body box")].flow,
            expected
        );
        assert_eq!(
            generated[generated.principal_box(child).expect("child box")].flow,
            expected
        );
        assert!(
            generated
                .boxes_for_node(text)
                .iter()
                .all(|box_id| generated[*box_id].flow == expected)
        );
    }
}
