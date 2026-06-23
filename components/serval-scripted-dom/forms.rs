//! Form-control value extraction for the verso flip's form layer.
//!
//! serval has no separate dirty-value store (`set_attribute` is the only value
//! path), so a control's value *is* its `value` attribute — `<textarea>` is the
//! one exception, whose value is its text content. Best-effort, per the flip's
//! form layer: it degrades, never blocks.

use layout_dom_api::NodeKind;

use crate::{NodeId, ScriptedDom};

impl ScriptedDom {
    /// Form-control values under `root` (inclusive), keyed by the control's `name`,
    /// falling back to `id`. Covers `<input>`/`<select>` (the `value` attribute) and
    /// `<textarea>` (text content). Controls with neither a name nor an id are
    /// skipped — nothing identifies them in a freshly loaded page.
    pub fn form_values(&self, root: NodeId) -> Vec<(String, String)> {
        let mut out = Vec::new();
        self.collect_form_values(root, &mut out);
        out
    }

    fn collect_form_values(&self, id: NodeId, out: &mut Vec<(String, String)>) {
        let node = self.node(id);
        if node.kind == NodeKind::Element {
            if let Some(name) = &node.name {
                let tag = name.local.as_ref();
                if matches!(tag, "input" | "select" | "textarea") {
                    let attr = |local: &str| {
                        node.attrs
                            .iter()
                            .find(|(n, _)| n.local.as_ref() == local)
                            .map(|(_, v)| v.as_str())
                    };
                    if let Some(key) = attr("name").or_else(|| attr("id")) {
                        let value = if tag == "textarea" {
                            self.text_content(id)
                        } else {
                            attr("value").unwrap_or("").to_owned()
                        };
                        out.push((key.to_owned(), value));
                    }
                }
            }
        }
        for &child in &node.children {
            self.collect_form_values(child, out);
        }
    }

    /// Concatenated descendant text — a `<textarea>`'s value.
    fn text_content(&self, id: NodeId) -> String {
        let mut out = String::new();
        self.append_text_content(id, &mut out);
        out
    }

    fn append_text_content(&self, id: NodeId, out: &mut String) {
        let node = self.node(id);
        if node.kind == NodeKind::Text {
            if let Some(text) = &node.text {
                out.push_str(text);
            }
        }
        for &child in &node.children {
            self.append_text_content(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use html5ever::{local_name, ns};
    use layout_dom_api::{LayoutDomMut, QualName};

    fn tag(local: markup5ever::LocalName) -> QualName {
        QualName::new(None, ns!(html), local)
    }
    fn attr(local: markup5ever::LocalName) -> QualName {
        QualName::new(None, ns!(), local)
    }

    #[test]
    fn collects_input_value_keyed_by_name() {
        let mut dom = ScriptedDom::new();
        let form = dom.create_element(tag(local_name!("form")));
        let input = dom.create_element(tag(local_name!("input")));
        dom.set_attribute(input, attr(local_name!("name")), "email");
        dom.set_attribute(input, attr(local_name!("value")), "a@b.com");
        dom.append_child(form, input);
        assert_eq!(
            dom.form_values(form),
            vec![("email".to_owned(), "a@b.com".to_owned())]
        );
    }

    #[test]
    fn textarea_value_is_its_text_content() {
        let mut dom = ScriptedDom::new();
        let ta = dom.create_element(tag(local_name!("textarea")));
        dom.set_attribute(ta, attr(local_name!("id")), "note");
        let text = dom.create_text("hello");
        dom.append_child(ta, text);
        assert_eq!(
            dom.form_values(ta),
            vec![("note".to_owned(), "hello".to_owned())]
        );
    }

    #[test]
    fn unnamed_controls_are_skipped() {
        let mut dom = ScriptedDom::new();
        let input = dom.create_element(tag(local_name!("input")));
        dom.set_attribute(input, attr(local_name!("value")), "x");
        assert!(dom.form_values(input).is_empty());
    }
}
