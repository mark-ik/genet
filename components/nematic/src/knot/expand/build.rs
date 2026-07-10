use inker::{Block, BlockProvenanceMap, DocumentProvenance, DocumentTrustState, EngineDocument};

/// Build a knot file (frontmatter + body) from a sequence of blocks plus
/// the source's [`DocumentProvenance`] and trust state.
///
/// The host calls this when a user clips an element to a knot: the host
/// hands over the selected blocks (taken from the source tile's
/// [`inker::EngineDocument`]) plus the source document's provenance, and
/// gets back a string ready to be saved as `.knot`. Round-tripping that
/// string through the knot engine reproduces the document.
pub fn build_clip_knot(
    blocks: &[Block],
    source: &DocumentProvenance,
    trust: DocumentTrustState,
    note_kind: Option<&str>,
) -> String {
    build_clip_knot_inner(blocks, source, trust, note_kind, None)
}

/// Like [`build_clip_knot`], but additionally records per-block
/// provenance overrides into a `block_sources:` frontmatter list.
///
/// Use when the clipped blocks came from heterogeneous sources — e.g.
/// a clip composer that glues paragraphs from several documents into
/// one knot. The `block_provenance` sidecar (see
/// [`BlockProvenanceMap`]) maps block-index → override; only indices
/// whose `canonical_uri` differs from `source.canonical_uri` are
/// emitted (single-source documents emit no list at all).
///
/// The frontmatter shape:
///
/// ```text
/// block_sources: ["<index>|<uri>[|<anchor>]", ...]
/// ```
///
/// Round-trip: the knot engine's frontmatter parser will surface this
/// list as a `block_sources` MetadataRow on re-render — full
/// per-block-provenance restoration through the engine is gated on a
/// concrete consumer (embed cross-source matching,
/// citation overlays). The producer side documents the shape so
/// downstream consumers can read it directly.
pub fn build_clip_knot_with_block_provenance(
    blocks: &[Block],
    source: &DocumentProvenance,
    trust: DocumentTrustState,
    note_kind: Option<&str>,
    block_provenance: &BlockProvenanceMap,
) -> String {
    build_clip_knot_inner(blocks, source, trust, note_kind, Some(block_provenance))
}

fn build_clip_knot_inner(
    blocks: &[Block],
    source: &DocumentProvenance,
    trust: DocumentTrustState,
    note_kind: Option<&str>,
    block_provenance: Option<&BlockProvenanceMap>,
) -> String {
    use inker::EngineDocument;

    let mut out = String::new();
    out.push_str("---\n");
    if let Some(uri) = &source.canonical_uri {
        out.push_str(&format!("source: {uri}\n"));
    }
    if let Some(when) = &source.fetched_at {
        out.push_str(&format!("captured: {when}\n"));
    }
    if let Some(label) = &source.source_label {
        out.push_str(&format!("source_label: {label}\n"));
    } else if let Some(kind) = &source.source_kind {
        // No human-readable label, but the source engine kind is still useful.
        out.push_str(&format!("source_label: {kind}\n"));
    }
    out.push_str(&format!("trust: {}\n", trust_to_string(trust)));
    if let Some(kind) = note_kind {
        out.push_str(&format!("note_kind: {kind}\n"));
    }
    if let Some(map) = block_provenance {
        let entries = collect_block_source_entries(map, source);
        if !entries.is_empty() {
            out.push_str("block_sources: [");
            for (i, entry) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push('"');
                out.push_str(entry);
                out.push('"');
            }
            out.push_str("]\n");
        }
    }
    out.push_str("---\n\n");

    // Use write_knot_body for the body so semantic blocks render as
    // fences. Calling to_knot() here would double the frontmatter
    // because the document-level frontmatter overlaps the clip-aware
    // frontmatter we just wrote.
    let document = EngineDocument {
        address: source
            .canonical_uri
            .clone()
            .unwrap_or_else(|| "knot:clip".to_string()),
        title: None,
        content_type: "text/x-knot".to_string(),
        lang: None,
        provenance: source.clone(),
        trust,
        diagnostics: Vec::new(),
        blocks: blocks.to_vec(),
    };
    document.write_knot_body(&mut out);
    out
}

/// Collect the per-block source overrides that differ from the
/// document-level source, encoded as `"<index>|<uri>[|<anchor>]"`
/// strings ready to drop into the `block_sources:` frontmatter list.
///
/// Entries are emitted in ascending index order so a downstream
/// consumer parsing the list gets stable, deterministic output.
fn collect_block_source_entries(
    map: &BlockProvenanceMap,
    document_source: &DocumentProvenance,
) -> Vec<String> {
    let mut entries: Vec<(usize, String)> = map
        .iter()
        .filter_map(|(index, block_prov)| {
            // Skip entries that match the document-level source with no
            // anchor — no real override, no list line.
            let same_uri = block_prov.provenance.canonical_uri == document_source.canonical_uri;
            if same_uri && block_prov.anchor.is_none() {
                return None;
            }
            let uri = block_prov.provenance.canonical_uri.as_deref()?;
            let encoded = match block_prov.anchor.as_deref() {
                Some(anchor) => format!("{index}|{uri}|{anchor}"),
                None => format!("{index}|{uri}"),
            };
            Some((index, encoded))
        })
        .collect();
    entries.sort_by_key(|(i, _)| *i);
    entries.into_iter().map(|(_, s)| s).collect()
}

fn trust_to_string(trust: DocumentTrustState) -> &'static str {
    match trust {
        DocumentTrustState::Trusted => "trusted",
        DocumentTrustState::Tofu => "tofu",
        DocumentTrustState::Insecure => "insecure",
        DocumentTrustState::Broken => "broken",
        DocumentTrustState::Unknown => "unknown",
    }
}
