//! A toy host DOM store. Stands in for `serval-scripted-dom`: a `NodeId`-keyed
//! Rust arena that lives entirely outside the JS engine. Reflectors carry the
//! `NodeId`; the engine never owns the node data.

use std::cell::RefCell;
use std::rc::Rc;

pub type NodeId = u32;

#[derive(Debug)]
struct Node {
    tag: String,
    text: String,
}

#[derive(Debug, Default)]
pub struct DomStore {
    nodes: Vec<Node>,
}

impl DomStore {
    pub fn shared() -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::default()))
    }

    /// Append a node, returning its stable id (the arena does not compact).
    pub fn push(&mut self, tag: &str) -> NodeId {
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node {
            tag: tag.to_owned(),
            text: String::new(),
        });
        id
    }

    pub fn tag(&self, id: NodeId) -> String {
        self.nodes[id as usize].tag.clone()
    }

    pub fn text(&self, id: NodeId) -> String {
        self.nodes[id as usize].text.clone()
    }

    pub fn set_text(&mut self, id: NodeId, text: &str) {
        self.nodes[id as usize].text = text.to_owned();
    }
}
