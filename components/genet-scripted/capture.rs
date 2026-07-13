/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Env-gated DOM mutation capture for the scripted tier.

use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use layout_dom_api::{CapturedQualName, DomMutation, LayoutDom, LayoutDomMut};
use serde::{Deserialize, Serialize};
#[cfg(feature = "render")]
use genet_layout::{Applied, IncrementalLayout};
use genet_scripted_dom::{NodeId, ScriptedDom};

fn capture_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("GENET_DOM_CAPTURE_DIR").map(PathBuf::from))
        .as_ref()
}

fn capture_viewport_seed() -> io::Result<(u32, u32)> {
    Ok((
        capture_dimension("GENET_DOM_CAPTURE_WIDTH", 1280)?,
        capture_dimension("GENET_DOM_CAPTURE_HEIGHT", 720)?,
    ))
}

fn capture_dimension(name: &str, default: u32) -> io::Result<u32> {
    let Some(raw) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = raw.into_string().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be valid UTF-8"),
        )
    })?;
    let parsed = value.parse::<u32>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be a positive integer"),
        )
    })?;
    if parsed == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than zero"),
        ));
    }
    Ok(parsed)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum DomCaptureRecord {
    SessionStart {
        snapshot_html: String,
        stylesheets: Vec<String>,
        layout_width: u32,
        layout_height: u32,
    },
    MutationBatch {
        mutations: Vec<RecordedMutation>,
        layout: Option<RecordedLayoutBatch>,
    },
}

pub(crate) struct DomCaptureRecorder {
    writer: BufWriter<File>,
    stylesheets: Vec<String>,
    layout_width: u32,
    layout_height: u32,
    #[cfg(feature = "render")]
    shadow_layout: IncrementalLayout<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReplayReport {
    pub batch_count: usize,
    pub final_snapshot_html: String,
    pub live_node_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum RecordedMutation {
    Inserted {
        node: u64,
        parent: u64,
        next_sibling: Option<u64>,
        outer_html: String,
    },
    Removed {
        node: u64,
        former_parent: u64,
        still_live: bool,
    },
    AttributeChanged {
        node: u64,
        name: CapturedQualName,
        old_value: Option<String>,
        new_value: Option<String>,
    },
    CharacterDataChanged {
        node: u64,
        new_data: String,
    },
    SubtreeReplaced {
        node: u64,
        new_inner_html: String,
    },
    /// An atomic in-tree move (`move_before`): the subtree survives, so no
    /// serialized HTML rides along — replay re-parents the live node.
    Moved {
        node: u64,
        from_parent: u64,
        to_parent: u64,
        next_sibling: Option<u64>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum RecordedApplied {
    Unchanged,
    RepaintOnly,
    Restyled,
    Spliced,
    FullRecompute,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecordedViewport {
    width: i32,
    height: i32,
    scroll_x_bits: u32,
    scroll_y_bits: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecordedLayoutBatch {
    applied: RecordedApplied,
    fragment_digest: u64,
    viewport: RecordedViewport,
}

impl RecordedMutation {
    fn capture(dom: &ScriptedDom, mutation: &DomMutation<NodeId>) -> Self {
        match mutation {
            DomMutation::Inserted { node, parent } => Self::Inserted {
                node: dom.capture_node_id(*node),
                parent: dom.capture_node_id(*parent),
                next_sibling: dom.next_sibling(*node).map(|id| dom.capture_node_id(id)),
                outer_html: dom.outer_html(*node),
            },
            DomMutation::Removed {
                node,
                former_parent,
            } => Self::Removed {
                node: dom.capture_node_id(*node),
                former_parent: dom.capture_node_id(*former_parent),
                still_live: dom.is_live(*node),
            },
            DomMutation::AttributeChanged {
                node,
                name,
                old_value,
            } => Self::AttributeChanged {
                node: dom.capture_node_id(*node),
                name: name.into(),
                old_value: old_value.clone(),
                new_value: dom
                    .attribute(*node, &name.ns, &name.local)
                    .map(ToString::to_string),
            },
            DomMutation::CharacterDataChanged { node } => Self::CharacterDataChanged {
                node: dom.capture_node_id(*node),
                new_data: dom.text(*node).unwrap_or_default().to_string(),
            },
            DomMutation::SubtreeReplaced { node } => Self::SubtreeReplaced {
                node: dom.capture_node_id(*node),
                new_inner_html: dom.inner_html(*node),
            },
            DomMutation::Moved {
                node,
                from_parent,
                to_parent,
            } => Self::Moved {
                node: dom.capture_node_id(*node),
                from_parent: dom.capture_node_id(*from_parent),
                to_parent: dom.capture_node_id(*to_parent),
                next_sibling: dom.next_sibling(*node).map(|id| dom.capture_node_id(id)),
            },
        }
    }
}

#[cfg(feature = "render")]
impl From<Applied> for RecordedApplied {
    fn from(value: Applied) -> Self {
        match value {
            Applied::Unchanged => Self::Unchanged,
            Applied::RepaintOnly => Self::RepaintOnly,
            Applied::Restyled => Self::Restyled,
            Applied::Spliced => Self::Spliced,
            Applied::FullRecompute => Self::FullRecompute,
        }
    }
}

impl RecordedLayoutBatch {
    #[cfg(feature = "render")]
    fn capture(dom: &ScriptedDom, layout: &IncrementalLayout<NodeId>, applied: Applied) -> Self {
        let viewport = layout.viewport();
        Self {
            applied: applied.into(),
            fragment_digest: fragment_digest(dom, layout.fragments()),
            viewport: RecordedViewport {
                width: viewport.size.width,
                height: viewport.size.height,
                scroll_x_bits: viewport.scroll.0.to_bits(),
                scroll_y_bits: viewport.scroll.1.to_bits(),
            },
        }
    }
}

impl DomCaptureRecorder {
    pub(crate) fn from_env(
        dom: &mut ScriptedDom,
        stylesheets: &[String],
    ) -> io::Result<Option<Self>> {
        let Some(dir) = capture_dir() else {
            return Ok(None);
        };
        Self::open_in_dir(dir, dom, stylesheets).map(Some)
    }

    fn open_in_dir(dir: &Path, dom: &mut ScriptedDom, stylesheets: &[String]) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(session_file_name());
        Self::open_at_path(&path, dom, stylesheets)
    }

    fn open_at_path(
        path: &Path,
        dom: &mut ScriptedDom,
        stylesheets: &[String],
    ) -> io::Result<Self> {
        let (layout_width, layout_height) = capture_viewport_seed()?;
        let mut recorder = Self {
            writer: BufWriter::new(File::create(path)?),
            stylesheets: stylesheets.to_vec(),
            layout_width,
            layout_height,
            #[cfg(feature = "render")]
            shadow_layout: new_shadow_layout(dom, stylesheets, layout_width, layout_height),
        };
        recorder.write_record(&DomCaptureRecord::SessionStart {
            snapshot_html: dom.inner_html(dom.document()),
            stylesheets: recorder.stylesheets.clone(),
            layout_width: recorder.layout_width,
            layout_height: recorder.layout_height,
        })?;
        // The initial snapshot is post-parse DOM state, so the bootstrap clone
        // mutations are baseline, not replayable deltas.
        let mut bootstrap = Vec::new();
        dom.drain_mutations(&mut bootstrap);
        Ok(recorder)
    }

    pub(crate) fn record_pending(&mut self, dom: &mut ScriptedDom) -> io::Result<usize> {
        let mut pending = Vec::new();
        dom.drain_mutations(&mut pending);
        if pending.is_empty() {
            return Ok(0);
        }
        let mutations = pending
            .iter()
            .map(|m| RecordedMutation::capture(dom, m))
            .collect();
        #[cfg(feature = "render")]
        let layout = Some(self.capture_layout(dom, &pending));
        #[cfg(not(feature = "render"))]
        let layout = None;
        self.write_record(&DomCaptureRecord::MutationBatch { mutations, layout })?;
        Ok(pending.len())
    }

    #[cfg(feature = "render")]
    fn capture_layout(
        &mut self,
        dom: &ScriptedDom,
        pending: &[DomMutation<NodeId>],
    ) -> RecordedLayoutBatch {
        let sheets = stylesheet_refs(&self.stylesheets);
        let applied = self.shadow_layout.apply(dom, &sheets, pending);
        RecordedLayoutBatch::capture(dom, &self.shadow_layout, applied)
    }

    fn write_record(&mut self, record: &DomCaptureRecord) -> io::Result<()> {
        let bytes = postcard::to_stdvec(record)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        let len = u32::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "dom capture record too large")
        })?;
        self.writer.write_all(&len.to_le_bytes())?;
        self.writer.write_all(&bytes)?;
        self.writer.flush()
    }
}

pub(crate) fn read_capture_records(path: &Path) -> io::Result<Vec<DomCaptureRecord>> {
    let mut file = File::open(path)?;
    let mut out = Vec::new();
    loop {
        let mut len_bytes = [0u8; 4];
        match file.read_exact(&mut len_bytes) {
            Ok(()) => {},
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err),
        }
        let len = u32::from_le_bytes(len_bytes) as usize;
        let mut bytes = vec![0u8; len];
        file.read_exact(&mut bytes)?;
        let record = postcard::from_bytes(&bytes)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
        out.push(record);
    }
    Ok(out)
}

pub(crate) fn replay_capture(path: &Path) -> io::Result<ReplayReport> {
    let records = read_capture_records(path)?;
    let Some(DomCaptureRecord::SessionStart {
        snapshot_html,
        stylesheets,
        layout_width,
        layout_height,
    }) = records.first()
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "capture file must start with SessionStart",
        ));
    };
    let mut dom = ScriptedDom::from_serialized_document(snapshot_html);
    #[cfg(feature = "render")]
    let mut replay_layout = new_shadow_layout(&dom, stylesheets, *layout_width, *layout_height);
    #[cfg(feature = "render")]
    let replay_sheets = stylesheet_refs(stylesheets);
    let mut batch_count = 0usize;
    for record in records.iter().skip(1) {
        let DomCaptureRecord::MutationBatch { mutations, layout } = record else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unexpected SessionStart after capture start",
            ));
        };
        let drained = replay_batch(&mut dom, mutations).map_err(|msg| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("replay batch {batch_count}: {msg}"),
            )
        })?;
        #[cfg(feature = "render")]
        if let Some(expected_layout) = layout {
            let applied = replay_layout.apply(&dom, &replay_sheets, &drained);
            let actual_layout = RecordedLayoutBatch::capture(&dom, &replay_layout, applied);
            if &actual_layout != expected_layout {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "replay batch {batch_count}: layout mismatch: expected {:?}, got {:?}",
                        expected_layout, actual_layout
                    ),
                ));
            }
        }
        #[cfg(not(feature = "render"))]
        if layout.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "capture file includes layout parity data but genet-scripted was built without render",
            ));
        }
        batch_count += 1;
    }
    Ok(ReplayReport {
        batch_count,
        final_snapshot_html: dom.inner_html(dom.document()),
        live_node_count: dom.live_node_count(),
    })
}

fn replay_batch(
    dom: &mut ScriptedDom,
    mutations: &[RecordedMutation],
) -> Result<Vec<DomMutation<NodeId>>, String> {
    for mutation in mutations {
        replay_mutation(dom, mutation)?;
    }
    let mut drained = Vec::new();
    dom.drain_mutations(&mut drained);
    if drained.len() != mutations.len() {
        return Err(format!(
            "replayed batch produced {} DomMutations, expected {}",
            drained.len(),
            mutations.len()
        ));
    }
    Ok(drained)
}

fn replay_mutation(dom: &mut ScriptedDom, mutation: &RecordedMutation) -> Result<(), String> {
    match mutation {
        RecordedMutation::Inserted {
            node,
            parent,
            next_sibling,
            outer_html,
        } => {
            let imported = dom.import_serialized_subtree(outer_html)?;
            if dom.capture_node_id(imported) != *node {
                return Err(format!(
                    "inserted subtree root reminted to {}, expected {}",
                    dom.capture_node_id(imported),
                    node
                ));
            }
            let parent = dom.remint_node_id(*parent);
            let next_sibling = next_sibling.map(|raw| dom.remint_node_id(raw));
            dom.insert_before(parent, imported, next_sibling);
        },
        RecordedMutation::Removed {
            node,
            former_parent,
            still_live,
        } => {
            let node = dom.remint_node_id(*node);
            let former_parent = dom.remint_node_id(*former_parent);
            if dom.parent(node) != Some(former_parent) {
                return Err("removed node parent mismatch before replay".to_string());
            }
            if *still_live {
                dom.remove_child(node);
            } else {
                dom.remove(node);
            }
        },
        RecordedMutation::AttributeChanged {
            node,
            name,
            new_value,
            ..
        } => {
            let node = dom.remint_node_id(*node);
            let name = name.clone().into_qual_name();
            match new_value {
                Some(value) => dom.set_attribute(node, name, value),
                None => dom.remove_attribute(node, name),
            }
        },
        RecordedMutation::CharacterDataChanged { node, new_data } => {
            dom.set_text(dom.remint_node_id(*node), new_data);
        },
        RecordedMutation::SubtreeReplaced {
            node,
            new_inner_html,
        } => {
            dom.set_inner_html(dom.remint_node_id(*node), new_inner_html);
        },
        RecordedMutation::Moved {
            node,
            from_parent,
            to_parent,
            next_sibling,
        } => {
            let node = dom.remint_node_id(*node);
            let from_parent = dom.remint_node_id(*from_parent);
            if dom.parent(node) != Some(from_parent) {
                return Err("moved node parent mismatch before replay".to_string());
            }
            let to_parent = dom.remint_node_id(*to_parent);
            let next_sibling = next_sibling.map(|raw| dom.remint_node_id(raw));
            dom.move_before(to_parent, node, next_sibling);
        },
    }
    Ok(())
}

#[cfg(feature = "render")]
fn new_shadow_layout(
    dom: &ScriptedDom,
    stylesheets: &[String],
    layout_width: u32,
    layout_height: u32,
) -> IncrementalLayout<NodeId> {
    let sheets = stylesheet_refs(stylesheets);
    IncrementalLayout::new(dom, &sheets, layout_width as f32, layout_height as f32)
}

#[cfg(feature = "render")]
fn stylesheet_refs(stylesheets: &[String]) -> Vec<&str> {
    stylesheets.iter().map(String::as_str).collect()
}

#[cfg(feature = "render")]
fn fragment_digest(dom: &ScriptedDom, fragments: &genet_layout::FragmentPlane<NodeId>) -> u64 {
    let mut entries: Vec<_> = fragments
        .iter()
        .map(|(node, layout)| {
            (
                dom.capture_node_id(*node),
                layout.location.x.to_bits(),
                layout.location.y.to_bits(),
                layout.size.width.to_bits(),
                layout.size.height.to_bits(),
            )
        })
        .collect();
    entries.sort_unstable_by_key(|entry| entry.0);

    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    for (node, x, y, width, height) in entries {
        hash_u64(&mut digest, node);
        hash_u32(&mut digest, x);
        hash_u32(&mut digest, y);
        hash_u32(&mut digest, width);
        hash_u32(&mut digest, height);
    }
    digest
}

#[cfg(feature = "render")]
fn hash_u64(state: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *state ^= byte as u64;
        *state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

#[cfg(feature = "render")]
fn hash_u32(state: &mut u64, value: u32) {
    hash_u64(state, value as u64);
}

fn session_file_name() -> String {
    format!("dom-capture-{}.postcard", now_millis())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::{LayoutDomMut, LocalName, Namespace, QualName};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    fn temp_capture_path() -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("genet-dom-capture-{}-{unique}.bin", now_millis()))
    }

    #[cfg(feature = "render")]
    fn structural_sheets() -> Vec<String> {
        crate::STRUCTURAL_SHEET
            .iter()
            .map(|sheet| sheet.to_string())
            .collect()
    }

    #[test]
    fn recorder_writes_snapshot_then_replayable_batches() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let body = dom.create_element(qual("body"));
        dom.append_child(root, body);

        let sheets = Vec::new();
        let path = temp_capture_path();
        let mut recorder = DomCaptureRecorder::open_at_path(&path, &mut dom, &sheets).unwrap();

        dom.set_attribute(body, qual("id"), "main");
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        let records = read_capture_records(&path).unwrap();
        let (layout_width, layout_height) = capture_viewport_seed().unwrap();
        assert_eq!(
            records[0],
            DomCaptureRecord::SessionStart {
                snapshot_html: "<body></body>".to_string(),
                stylesheets: sheets,
                layout_width,
                layout_height,
            }
        );
        match &records[1] {
            DomCaptureRecord::MutationBatch { mutations, layout } => {
                assert_eq!(
                    mutations,
                    &vec![RecordedMutation::AttributeChanged {
                        node: dom.capture_node_id(body),
                        name: (&qual("id")).into(),
                        old_value: None,
                        new_value: Some("main".to_string()),
                    }]
                );
                #[cfg(feature = "render")]
                assert!(layout.is_some(), "render build should record layout parity");
                #[cfg(not(feature = "render"))]
                assert!(
                    layout.is_none(),
                    "non-render build should skip layout parity"
                );
            },
            other => panic!("unexpected record: {other:?}"),
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn recorder_captures_insert_position_and_removed_liveness() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let a = dom.create_element(qual("a"));
        let c = dom.create_element(qual("c"));
        dom.append_child(root, a);
        dom.append_child(root, c);

        let sheets = Vec::new();
        let path = temp_capture_path();
        let mut recorder = DomCaptureRecorder::open_at_path(&path, &mut dom, &sheets).unwrap();

        let b = dom.create_element(qual("b"));
        dom.insert_before(root, b, Some(c));
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        dom.remove_child(b);
        dom.remove(c);
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 2);

        let records = read_capture_records(&path).unwrap();
        match &records[1] {
            DomCaptureRecord::MutationBatch { mutations, .. } => {
                assert_eq!(
                    mutations,
                    &vec![RecordedMutation::Inserted {
                        node: dom.capture_node_id(b),
                        parent: dom.capture_node_id(root),
                        next_sibling: Some(dom.capture_node_id(c)),
                        outer_html: "<b></b>".to_string(),
                    }]
                );
            },
            other => panic!("unexpected record: {other:?}"),
        }
        match &records[2] {
            DomCaptureRecord::MutationBatch { mutations, .. } => {
                assert_eq!(
                    mutations,
                    &vec![
                        RecordedMutation::Removed {
                            node: dom.capture_node_id(b),
                            former_parent: dom.capture_node_id(root),
                            still_live: true,
                        },
                        RecordedMutation::Removed {
                            node: dom.capture_node_id(c),
                            former_parent: dom.capture_node_id(root),
                            still_live: false,
                        },
                    ]
                );
            },
            other => panic!("unexpected record: {other:?}"),
        }

        let _ = fs::remove_file(path);
    }

    #[test]
    fn replay_capture_rebuilds_document_and_orphan_liveness() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let html = dom.create_element(qual("html"));
        dom.append_child(root, html);
        let head = dom.create_element(qual("head"));
        dom.append_child(html, head);
        let body = dom.create_element(qual("body"));
        dom.append_child(html, body);

        #[cfg(feature = "render")]
        let sheets = structural_sheets();
        #[cfg(not(feature = "render"))]
        let sheets = Vec::new();
        let path = temp_capture_path();
        let mut recorder = DomCaptureRecorder::open_at_path(&path, &mut dom, &sheets).unwrap();

        dom.set_inner_html(body, "<section><p>one</p></section>");
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        let section = dom.dom_children(body).next().unwrap();
        let note = dom.create_comment("note");
        dom.insert_before(body, note, Some(section));
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        dom.remove_child(note);
        let p = dom
            .first_tag(body, "p")
            .expect("subtree replacement created paragraph");
        let text = dom.dom_children(p).next().unwrap();
        dom.set_text(text, "two");
        dom.set_attribute(section, qual("data-x"), "1");
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 3);

        let expected_snapshot = dom.inner_html(dom.document());
        let expected_live_nodes = dom.live_node_count();

        let report = replay_capture(&path).unwrap();
        assert_eq!(
            report,
            ReplayReport {
                batch_count: 3,
                final_snapshot_html: expected_snapshot,
                live_node_count: expected_live_nodes,
            }
        );

        let _ = fs::remove_file(path);
    }

    #[cfg(feature = "render")]
    #[test]
    fn replay_capture_verifies_layout_parity_batches() {
        let mut dom =
            ScriptedDom::from_serialized_document("<html><body><div>hello</div></body></html>");
        let root = dom.document();
        let body = dom.first_tag(root, "body").expect("body");
        let div = dom.first_tag(root, "div").expect("div");

        let sheets = structural_sheets();
        let path = temp_capture_path();
        let mut recorder = DomCaptureRecorder::open_at_path(&path, &mut dom, &sheets).unwrap();

        dom.set_attribute(div, qual("style"), "display:block; width: 100px;");
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        dom.set_attribute(
            div,
            qual("style"),
            "display:block; width: 100px; color: red;",
        );
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        let note = dom.create_comment("note");
        dom.insert_before(body, note, Some(div));
        assert_eq!(recorder.record_pending(&mut dom).unwrap(), 1);

        let records = read_capture_records(&path).unwrap();
        let applied: Vec<_> = records
            .iter()
            .skip(1)
            .map(|record| match record {
                DomCaptureRecord::MutationBatch {
                    layout: Some(layout),
                    ..
                } => layout.applied.clone(),
                other => panic!("expected layout batch, got {other:?}"),
            })
            .collect();
        assert_eq!(
            applied,
            vec![
                RecordedApplied::Restyled,
                RecordedApplied::RepaintOnly,
                RecordedApplied::Spliced,
            ]
        );

        let report = replay_capture(&path).unwrap();
        assert_eq!(report.batch_count, 3);

        let _ = fs::remove_file(path);
    }
}
