//! Livery computed-value and DOM adapter for Buckram's CSS box tree.
//!
//! Buckram owns box identity, roles, provenance, and tree relationships. This
//! module translates Livery values and preserves the two B0-only lowering
//! boundaries needed for an exact geometry migration.

use std::{hash::Hash, ops::Deref};

use buckram::{
    BoxGeneration, BoxId, BoxOrigin, ContainingBlock, ContainingBlockRule, CssBox, CssBoxTree,
    DisplayInside, DisplayOutside, DisplayRole, FormattingContextKind, InternalTableRole,
    PositioningScheme,
};
use layout_dom_api::{LayoutDom, NodeKind};
use livery::values::{Display as ComputedDisplay, Position as ComputedPosition};

use crate::StylePlane;

/// Livery-generated Buckram boxes plus private behavior-preserving lowering
/// metadata.
#[derive(Clone, Debug)]
pub(crate) struct GeneratedBoxTree<Id> {
    tree: CssBoxTree<Id>,
    suppressed: Vec<SuppressedNode<Id>>,
    lowering_roots: Vec<LoweringSource>,
    lowering_children: std::collections::HashMap<BoxId, Vec<LoweringSource>>,
}

impl<Id> Deref for GeneratedBoxTree<Id> {
    type Target = CssBoxTree<Id>;

    fn deref(&self) -> &Self::Target {
        &self.tree
    }
}

/// B0-only traversal metadata for behavior-preserving Taffy lowering.
///
/// A `display: none` subtree generates no CSS boxes, but the old builder
/// inserted a suppressed node and treated it as an inline-run boundary. K2
/// removes this compatibility source under the actual box-generation rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LoweringSource {
    Box(BoxId),
    Suppressed(SuppressedId),
    /// A DOM node that generated no backend node but split an inline run in
    /// the pre-B0 builder, notably a comment.
    Boundary,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SuppressedId(u32);

#[derive(Clone, Debug)]
pub(crate) struct SuppressedNode<Id> {
    pub node: Id,
    pub children: Vec<LoweringSource>,
}

impl<Id> GeneratedBoxTree<Id>
where
    Id: Copy + Eq + Hash,
{
    /// Generate Buckram boxes from Livery computed values.
    ///
    /// K0 preserves DOM order and the current element/text structure. K2
    /// replaces this adapter's direct traversal with CSS Display fixup.
    pub(crate) fn from_dom<D>(dom: &D, styles: &StylePlane<Id>) -> Self
    where
        D: LayoutDom<NodeId = Id>,
    {
        fn generate<D>(
            generated: &mut GeneratedBoxTree<D::NodeId>,
            dom: &D,
            styles: &StylePlane<D::NodeId>,
            node: D::NodeId,
            parent: Option<BoxId>,
        ) where
            D: LayoutDom,
            D::NodeId: Copy + Eq + Hash,
        {
            match dom.kind(node) {
                NodeKind::Document | NodeKind::DocumentFragment => {
                    for child in dom.dom_children(node) {
                        generate(generated, dom, styles, child, parent);
                    }
                },
                NodeKind::Element => {
                    let computed = styles.get(node).cloned().unwrap_or_default();
                    let display = display_role(computed.display);
                    if display.generation == BoxGeneration::None {
                        let suppressed = generated.capture_suppressed(dom, node);
                        generated
                            .push_lowering_source(parent, LoweringSource::Suppressed(suppressed));
                        return;
                    }
                    let positioning = positioning_scheme(computed.position);
                    let containing_block = match parent {
                        None => ContainingBlock::Initial,
                        Some(_) => match positioning {
                            PositioningScheme::Absolute => {
                                ContainingBlock::Pending(ContainingBlockRule::AbsolutePositioned)
                            },
                            PositioningScheme::Fixed => {
                                ContainingBlock::Pending(ContainingBlockRule::FixedPositioned)
                            },
                            _ => ContainingBlock::Pending(ContainingBlockRule::NormalFlow),
                        },
                    };
                    let formatting_context = match display.inside {
                        Some(DisplayInside::Flex) => Some(FormattingContextKind::Flex),
                        Some(DisplayInside::Grid) => Some(FormattingContextKind::Grid),
                        Some(DisplayInside::Table) => Some(FormattingContextKind::Table),
                        _ => None,
                    };
                    let box_id = generated.tree.push(
                        CssBox::new(
                            BoxOrigin::Element(node),
                            display,
                            positioning,
                            is_replaced_element(dom, node),
                            formatting_context,
                            containing_block,
                        ),
                        parent,
                        true,
                    );
                    generated.push_lowering_source(parent, LoweringSource::Box(box_id));
                    for child in dom.dom_children(node) {
                        generate(generated, dom, styles, child, Some(box_id));
                    }
                },
                NodeKind::Text => {
                    let box_id = generated.tree.push(
                        CssBox::new(
                            BoxOrigin::Text(node),
                            DisplayRole::INLINE_FLOW,
                            PositioningScheme::Static,
                            false,
                            None,
                            parent.map_or(ContainingBlock::Initial, |_| {
                                ContainingBlock::Pending(ContainingBlockRule::NormalFlow)
                            }),
                        ),
                        parent,
                        false,
                    );
                    generated.push_lowering_source(parent, LoweringSource::Box(box_id));
                },
                _ => generated.push_lowering_source(parent, LoweringSource::Boundary),
            }
        }

        let mut generated = Self {
            tree: CssBoxTree::default(),
            suppressed: Vec::new(),
            lowering_roots: Vec::new(),
            lowering_children: std::collections::HashMap::new(),
        };
        generate(&mut generated, dom, styles, dom.document(), None);
        generated
    }

    pub(crate) fn into_tree(self) -> CssBoxTree<Id> {
        self.tree
    }

    pub(crate) fn lowering_roots(&self) -> &[LoweringSource] {
        &self.lowering_roots
    }

    pub(crate) fn lowering_children(&self, parent: BoxId) -> &[LoweringSource] {
        self.lowering_children
            .get(&parent)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn suppressed(&self, id: SuppressedId) -> &SuppressedNode<Id> {
        &self.suppressed[id.0 as usize]
    }

    fn capture_suppressed<D>(&mut self, dom: &D, node: Id) -> SuppressedId
    where
        D: LayoutDom<NodeId = Id>,
    {
        let id = SuppressedId(
            self.suppressed
                .len()
                .try_into()
                .expect("a suppressed source tree exceeded u32::MAX nodes"),
        );
        self.suppressed.push(SuppressedNode {
            node,
            children: Vec::new(),
        });
        let children = dom
            .dom_children(node)
            .map(|child| match dom.kind(child) {
                NodeKind::Document
                | NodeKind::DocumentFragment
                | NodeKind::Element
                | NodeKind::Text => LoweringSource::Suppressed(self.capture_suppressed(dom, child)),
                _ => LoweringSource::Boundary,
            })
            .collect();
        self.suppressed[id.0 as usize].children = children;
        id
    }

    fn push_lowering_source(&mut self, parent: Option<BoxId>, source: LoweringSource) {
        if let Some(parent) = parent {
            self.lowering_children
                .entry(parent)
                .or_default()
                .push(source);
        } else {
            self.lowering_roots.push(source);
        }
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
        ComputedDisplay::Inline => DisplayRole::INLINE_FLOW,
        ComputedDisplay::Block => DisplayRole::BLOCK_FLOW,
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

fn is_replaced_element<D>(dom: &D, node: D::NodeId) -> bool
where
    D: LayoutDom,
{
    dom.element_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Device, InteractionStates, StyleSet, resolve_styles};
    use genet_static_dom::StaticDocument;
    use layout_dom_api::{LocalName, Namespace};

    #[test]
    fn generated_tree_keeps_b0_boundaries_outside_box_identity() {
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
             <span id=\"hidden\">hidden</span></body></html>",
        );
        let styles = resolve_styles(
            &dom,
            &StyleSet::cambium(&["#hidden { display: none; }"]),
            &Device::screen(800.0, 600.0),
            &InteractionStates::default(),
        );
        let generated = GeneratedBoxTree::from_dom(&dom, &styles);
        let body = find(&dom, dom.document(), "body").expect("body");
        let shown = find(&dom, dom.document(), "shown").expect("shown");
        let hidden = find(&dom, dom.document(), "hidden").expect("hidden");
        let body_box = generated.principal_box(body).expect("body principal box");

        assert!(generated.principal_box(shown).is_some());
        assert_eq!(generated.principal_box(hidden), None);
        assert!(generated.boxes_for_node(hidden).is_empty());
        assert!(
            generated
                .lowering_children(body_box)
                .contains(&LoweringSource::Boundary),
            "an ignored comment remains a B0 inline-run boundary"
        );
        assert!(
            generated
                .lowering_children(body_box)
                .iter()
                .any(|source| matches!(source, LoweringSource::Suppressed(_))),
            "display:none remains private lowering metadata, not a CSS box"
        );
    }
}
