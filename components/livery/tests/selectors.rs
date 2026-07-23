use std::{collections::HashSet, sync::Arc};

use livery::selector::{
    Atom, AttrSelectorOperation, BloomFilter, CaseSensitivity, Element, ElementSelectorFlags,
    LiverySelectorImpl, NamespaceConstraint, NoPseudoElement, OpaqueElement, SelectorList,
    StatePseudoClass,
};
use livery::{
    cascade::{CascadeLayer, Origin},
    media::Device,
    stylesheet::{StyleRule, cascade_rules},
    values::Color,
};
use selectors::matching::MatchingContext;

#[derive(Debug)]
struct Node {
    name: &'static str,
    attributes: Vec<(&'static str, &'static str)>,
    parent: Option<usize>,
    children: Vec<usize>,
    states: HashSet<StatePseudoClass>,
}

#[derive(Debug)]
struct Dom {
    nodes: Vec<Node>,
}

#[derive(Clone, Debug)]
struct ElementRef {
    dom: Arc<Dom>,
    id: usize,
}

impl ElementRef {
    fn node(&self) -> &Node {
        &self.dom.nodes[self.id]
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.node()
            .attributes
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| *value)
    }

    fn sibling(&self, offset: isize) -> Option<Self> {
        let parent = self.node().parent?;
        let siblings = &self.dom.nodes[parent].children;
        let index = siblings.iter().position(|id| *id == self.id)? as isize + offset;
        let id = *siblings.get(usize::try_from(index).ok()?)?;
        Some(Self {
            dom: self.dom.clone(),
            id,
        })
    }
}

impl Element for ElementRef {
    type Impl = LiverySelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.node())
    }

    fn parent_element(&self) -> Option<Self> {
        self.node().parent.map(|id| Self {
            dom: self.dom.clone(),
            id,
        })
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling(-1)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling(1)
    }

    fn first_element_child(&self) -> Option<Self> {
        self.node().children.first().copied().map(|id| Self {
            dom: self.dom.clone(),
            id,
        })
    }

    fn is_html_element_in_html_document(&self) -> bool {
        true
    }

    fn has_local_name(&self, local_name: &Atom) -> bool {
        self.node().name.eq_ignore_ascii_case(local_name.as_str())
    }

    fn has_namespace(&self, namespace: &Atom) -> bool {
        namespace.as_str().is_empty()
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.node().name.eq_ignore_ascii_case(other.node().name)
    }

    fn attr_matches(
        &self,
        namespace: &NamespaceConstraint<&Atom>,
        local_name: &Atom,
        operation: &AttrSelectorOperation<&livery::selector::AttributeValue>,
    ) -> bool {
        if matches!(namespace, NamespaceConstraint::Specific(ns) if !ns.as_str().is_empty()) {
            return false;
        }
        self.attribute(local_name.as_str())
            .is_some_and(|value| operation.eval_str(value))
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo: &StatePseudoClass,
        _context: &mut MatchingContext<LiverySelectorImpl>,
    ) -> bool {
        self.node().states.contains(pseudo)
    }

    fn match_pseudo_element(
        &self,
        pseudo: &NoPseudoElement,
        _context: &mut MatchingContext<LiverySelectorImpl>,
    ) -> bool {
        match *pseudo {}
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        self.node().name == "a" && self.attribute("href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &Atom, case_sensitivity: CaseSensitivity) -> bool {
        self.attribute("id")
            .is_some_and(|value| case_sensitivity.eq(value.as_bytes(), id.as_str().as_bytes()))
    }

    fn has_class(&self, class: &Atom, case_sensitivity: CaseSensitivity) -> bool {
        self.attribute("class").is_some_and(|classes| {
            classes
                .split_ascii_whitespace()
                .any(|value| case_sensitivity.eq(value.as_bytes(), class.as_str().as_bytes()))
        })
    }

    fn has_custom_state(&self, _name: &Atom) -> bool {
        false
    }

    fn imported_part(&self, _name: &Atom) -> Option<Atom> {
        None
    }

    fn is_part(&self, _name: &Atom) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.node().children.is_empty()
    }

    fn is_root(&self) -> bool {
        self.node().parent.is_none()
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        false
    }
}

fn fixture() -> (ElementRef, ElementRef) {
    let dom = Arc::new(Dom {
        nodes: vec![
            Node {
                name: "main",
                attributes: vec![("id", "app")],
                parent: None,
                children: vec![1],
                states: HashSet::new(),
            },
            Node {
                name: "section",
                attributes: vec![("class", "catalog")],
                parent: Some(0),
                children: vec![2, 3],
                states: HashSet::new(),
            },
            Node {
                name: "button",
                attributes: vec![
                    ("id", "save"),
                    ("class", "primary control"),
                    ("data-role", "action"),
                ],
                parent: Some(1),
                children: vec![],
                states: HashSet::from([StatePseudoClass::Hover]),
            },
            Node {
                name: "button",
                attributes: vec![("class", "control")],
                parent: Some(1),
                children: vec![],
                states: HashSet::new(),
            },
        ],
    });
    (
        ElementRef {
            dom: dom.clone(),
            id: 2,
        },
        ElementRef { dom, id: 3 },
    )
}

#[test]
fn substrate_matches_structural_attribute_and_state_selectors() {
    let (primary, plain) = fixture();

    for selector in [
        "main button",
        "section > button.primary:hover",
        "button[data-role=action]",
        "button:first-child",
        "#save",
    ] {
        assert!(
            SelectorList::parse(selector)
                .unwrap()
                .matching_specificity(&primary)
                .is_some(),
            "{selector}"
        );
    }
    assert!(
        SelectorList::parse("button.primary:hover")
            .unwrap()
            .matching_specificity(&plain)
            .is_none()
    );
    assert!(
        SelectorList::parse("button + button")
            .unwrap()
            .matching_specificity(&plain)
            .is_some()
    );
}

#[test]
fn selector_lists_return_the_strongest_matching_specificity() {
    let (primary, _) = fixture();
    let specificity = SelectorList::parse("button, .primary, #save")
        .unwrap()
        .matching_specificity(&primary)
        .unwrap();

    assert!(
        specificity.0
            > SelectorList::parse(".primary")
                .unwrap()
                .matching_specificity(&primary)
                .unwrap()
                .0
    );
}

#[test]
fn selector_dependencies_only_widen_structural_restyles() {
    let plain = SelectorList::parse(".card .label").unwrap();
    assert!(!plain.has_sibling_dependency());
    assert!(!plain.has_structural_dependency());

    let sibling = SelectorList::parse(".card + .card").unwrap();
    assert!(sibling.has_sibling_dependency());
    assert!(sibling.has_structural_dependency());

    let positional = SelectorList::parse("li:nth-child(2)").unwrap();
    assert!(!positional.has_sibling_dependency());
    assert!(positional.has_structural_dependency());

    let attribute_value = SelectorList::parse("[data-key='a+b'] .label").unwrap();
    assert!(!attribute_value.has_sibling_dependency());
    assert!(!attribute_value.has_structural_dependency());
}

#[test]
fn rules_join_selector_media_and_cascade_ordering() {
    let (primary, plain) = fixture();
    let rules = vec![
        StyleRule::parse(
            "button",
            "color: #111111",
            None,
            Origin::Author,
            CascadeLayer::Unlayered,
            0,
        )
        .unwrap(),
        StyleRule::parse(
            ".primary",
            "color: #3568b8",
            None,
            Origin::Author,
            CascadeLayer::Unlayered,
            1,
        )
        .unwrap(),
        StyleRule::parse(
            "#save",
            "color: #aa0000",
            Some("(min-width: 700px)"),
            Origin::Author,
            CascadeLayer::Unlayered,
            2,
        )
        .unwrap(),
        StyleRule::parse(
            "button:hover",
            "background-color: #ffffff",
            None,
            Origin::Author,
            CascadeLayer::Unlayered,
            3,
        )
        .unwrap(),
    ];

    let wide = cascade_rules(None, &primary, &Device::screen(800.0, 600.0), &rules);
    assert_eq!(wide.color, "#aa0000".parse::<Color>().unwrap());
    assert_eq!(wide.background_color, "#ffffff".parse::<Color>().unwrap());

    let narrow = cascade_rules(None, &primary, &Device::screen(600.0, 600.0), &rules);
    assert_eq!(narrow.color, "#3568b8".parse::<Color>().unwrap());

    let plain = cascade_rules(None, &plain, &Device::screen(800.0, 600.0), &rules);
    assert_eq!(plain.color, "#111111".parse::<Color>().unwrap());
    assert_eq!(plain.background_color, Color::Transparent);
}
