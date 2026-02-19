# Universal Node Content Model and Protocol Resolution (2026-02-18)

## Status

Aspirational design note. No implementation planned this cycle. Intended to guide long-term architectural decisions and prevent short-term choices that would foreclose this direction.

---

## 1. Core Vision

A GraphShell node is a **persistent, addressable content container** — not a browser tab. Closing the renderer (webview, PDF viewer, image viewer) leaves the node intact, with its address, history, graph relationships, and metadata preserved. The renderer is one way to look at the node's content, not the node itself.

This generalizes the current `Cold`/`Active` distinction:

| State | Current meaning | Generalized meaning |
| --- | --- | --- |
| `Cold` | Webview closed; URL stored | Renderer detached; address + history stored |
| `Active` | Webview open and loaded | Renderer attached and rendering |
| `Loading` | (implicit in webview) | Address resolving or renderer initializing |
| `Error` | (partial, via tile crash) | Address unresolvable or renderer failed |

A Cold node is not an empty shell — it is a fully realized graph citizen with address, title, history, edges, and pins. It just has no active renderer attached.

---

## 2. What Nodes Would Contain

```
Node {
    // Identity (stable across sessions)
    id: NodeId,              // stable UUID, independent of address
    address: Address,        // the "what" — see §3

    // Content metadata
    title: Option<String>,
    mime_hint: Option<MimeType>,  // declared or sniffed content type
    favicon: Option<Texture>,

    // Graph data
    is_pinned: bool,
    edges: (managed by graph layer),

    // History (append-only log, already aligned with fjall model)
    address_history: Vec<HistoryEntry>,  // prior addresses for this node
    // note: navigation within a node's lifetime is an address_history entry,
    //       NOT a new node (same container, new content state)

    // Renderer hint (user preference or auto-detected)
    preferred_renderer: Option<RendererKind>,
}

Address {
    Http(Url),          // served over HTTP/HTTPS via Servo
    File(PathBuf),      // local filesystem path
    Onion(OnionAddr),   // Tor hidden service
    Ipfs(Cid),          // IPFS content address
    Gemini(Url),        // Gemini protocol URI (gemini://)
    Dat(Url),           // Hypercore/Dat protocol
    Custom(String),     // extensibility escape hatch
}

HistoryEntry {
    address: Address,
    timestamp: Instant,
    title: Option<String>,
    scroll_position: Option<(f32, f32)>,
}
```

**Key consequence:** Node identity (`NodeId`) is decoupled from address. Navigation within a node (following a link) changes the address but not the node. This is the "tab stays alive" mental model generalized: the container persists, the content inside it changes.

This is a breaking change from the current URL-based node identity used for persistence. It's the most significant migration in this vision — worth tracking as a prerequisite.

---

## 3. Renderer Module Architecture

A renderer is anything that can display a node's content. The interface is a Rust trait:

```rust
trait ContentRenderer: Send + Sync {
    /// Can this renderer handle the given address and/or MIME type?
    fn can_render(&self, address: &Address, mime: Option<&MimeType>) -> bool;

    /// Priority (higher wins when multiple renderers match).
    fn priority(&self) -> u8;

    /// Create a live view for a node. Returns an opaque handle.
    fn open(&self, node: &Node, ctx: &RendererCtx) -> Box<dyn RendererView>;
}

trait RendererView: Send {
    fn navigate(&mut self, address: &Address);
    fn as_egui_widget(&mut self, ui: &mut egui::Ui);
    fn close(self: Box<Self>);
    fn title(&self) -> Option<String>;
    fn can_go_back(&self) -> bool;
    fn go_back(&mut self);
}
```

### 3.1 Built-in Renderers

| Renderer | Address types | Notes |
| --- | --- | --- |
| `ServoRenderer` | `Http`, `File` (html/css/js) | Current default; full web engine |
| `NativePdfRenderer` | `File(.pdf)`, `Http` (application/pdf) | PDFium or similar via FFI |
| `NativeImageRenderer` | `File(.png/.jpg/...)`, `Http` (image/*) | egui texture rendering |
| `PlainTextRenderer` | `File(.txt/.md/...)`, any text/* | Syntax-highlighted text view |
| `DirectoryRenderer` | `File` (directory) | File-manager-style listing; each entry can become a new node |

### 3.2 Optional Platform Renderer Modules

Platform webviews can render anything Servo can't (yet) — useful for sites with heavy JS incompatibilities or proprietary DRM.

| Module | Platform | Backend |
| --- | --- | --- |
| `WebView2Renderer` | Windows | EdgeHTML/Chromium via `webview2-rs` |
| `WkWebViewRenderer` | macOS/iOS | WKWebView via objc bindings |
| `WebKitGtkRenderer` | Linux | WebKitGTK |

These are cargo feature-gated (`--features platform-webview`) and never the default. Servo remains the primary renderer. Platform webviews are fallback modules the user can activate per-node from the command palette.

### 3.3 Renderer Selection

Selection priority at node open time:

1. User's `preferred_renderer` on the node (explicit user choice, persisted).
2. Registered renderer with highest `priority()` where `can_render()` returns true.
3. Fallback: `PlainTextRenderer` (always succeeds, shows raw content).
4. Error state if address is unresolvable.

The renderer registry is a `Vec<Box<dyn ContentRenderer>>` sorted by priority, built at startup from compiled-in and optionally loaded modules.

---

## 4. Protocol Resolver Architecture

Before a renderer can display content, the address must be resolved into a byte stream (or local resource). The protocol resolver layer sits between the address and the renderer.

```rust
trait ProtocolResolver: Send + Sync {
    fn handles_scheme(&self) -> &[&str];
    fn resolve(&self, address: &Address, ctx: &ResolverCtx) -> ResolverResult;
}

enum ResolverResult {
    ByteStream(Box<dyn Read + Send>),
    LocalPath(PathBuf),    // renderer opens the file directly
    ProxyUrl(Url),         // redirect to this URL (for Tor: SOCKS5 proxy URL)
    Error(ResolverError),
}
```

### 4.1 Built-in Resolvers

| Resolver | Scheme | Notes |
| --- | --- | --- |
| `HttpResolver` | `http`, `https` | Delegates to Servo's net layer; already implemented |
| `FileResolver` | `file` | OS filesystem access; respects sandbox permissions |
| `GeminiResolver` | `gemini` | Gemini protocol is simple text-based; pure Rust implementation feasible (~500 LOC) |

### 4.2 Tor Support (Arti integration)

[Arti](https://gitlab.torproject.org/tpo/core/arti) is the Tor Project's official pure-Rust Tor client. It exposes a SOCKS5 proxy interface that Servo can route through, or a higher-level async API for direct use.

Two integration paths, in order of complexity:

**Path A (simpler): System Tor + SOCKS5 proxy**

- User runs `tor` daemon locally (or graphshell bundles a minimal tor binary).
- Graphshell's `TorResolver` intercepts `.onion` addresses and routes Servo's network stack through `socks5://127.0.0.1:9050`.
- Pros: No Rust Tor implementation needed; works with existing Servo net layer.
- Cons: Requires external tor process; less integrated.

**Path B (deeper): Arti as library dependency**

- Add `arti-client` crate as an optional dependency (`--features tor`).
- `TorResolver` opens a `TorClient` connection per-node and streams bytes directly.
- Pros: Self-contained; no external process; better control over circuit lifecycle.
- Cons: Arti is still maturing; adds ~3MB to binary.

Either path keeps onion routing entirely behind the resolver interface — renderers don't know or care whether content came from clearnet or Tor.

### 4.3 IPFS Support

IPFS content addresses (`ipfs://Qm...` or CIDv1) can be resolved via:

- **HTTP gateway** (simplest): route to `https://ipfs.io/ipfs/{CID}` — no IPFS daemon needed, but relies on public gateway.
- **Local Kubo daemon**: HTTP API at `http://localhost:5001/api/v0/cat?arg={CID}` — full IPFS node.
- **Rust IPFS** (`rust-ipfs` crate): embedded IPFS node — most integrated but heaviest.

The resolver interface abstracts all three; the user (or graphshell config) chooses which backend is active.

### 4.4 Unusual / Custom Protocols

The resolver registry is extensible. Any scheme not handled by a registered resolver falls through to an error state (addressable from the palette: "Open with…" allows user to pick a resolver or external application).

Future candidates:
- `dat://` / `hyper://` — Hypercore protocol
- `magnet:` — BitTorrent magnet links (resolve to torrent content stream)
- `mxc://` — Matrix content URIs
- `nostr:` — Nostr event references

---

## 5. Version-Controlled Node History

### 5.1 History Model

The existing fjall append-only log already provides the structural basis. Node history generalizes this:

```
LogEntry::AddNode { node_id, initial_address }
LogEntry::NavigateNode { node_id, from_address, to_address, timestamp }
LogEntry::UpdateNodeTitle { node_id, title }
LogEntry::PinNode { node_id, is_pinned }
LogEntry::RemoveNode { node_id }
```

`NavigateNode` replaces the current `UpdateNodeUrl` entry. The key difference: the history of every address a node has ever held is now intrinsic to the log, not just the current state.

### 5.2 Temporal Navigation

With a complete navigation log per node, the UI can offer:

- **Node history panel**: Chronological list of addresses this node has visited (like browser back/forward, but persistent across cold states).
- **Graph time scrubbing** (from the P2P collaboration vision): dragging a time slider replays the log to any point, showing the graph as it was.
- **Snapshot restore**: "What was this node showing last Tuesday?" — answered by replaying log entries up to a timestamp.

This naturally integrates with the persistence model already in place; it's an extension of the log schema, not a new storage system.

---

## 6. The Node as File Manager

When a `File` address points to a directory, the `DirectoryRenderer` shows its contents. Each entry (file, subdirectory) is interactive:

- **Click a file**: Opens it in a new node (or the current node, like navigation) using the appropriate renderer.
- **Click a directory**: Navigates the current node into that directory.
- **Drag a file to the graph**: Creates a new node with a `File` address for that file.

This turns graphshell into a file manager where the graph is the navigation context — bookmarks and browsing history live alongside local filesystem exploration in the same graph structure.

Analogies:
- Emacs `dired` mode, but with graph relationships.
- macOS Finder, but navigation history forms graph edges.
- A browser where `file://` is first-class.

---

## 7. Permission and Sandbox Model

Different address types need different permissions:

| Address type | Permission required | Default |
| --- | --- | --- |
| `Http`/`Https` | Network access (already assumed) | Granted |
| `File` | Filesystem read (sandboxed to user home?) | Prompt on first access |
| `Onion` | Tor routing enabled | Off; user activates per-node |
| `Ipfs` | IPFS resolver configured | Off; requires setup |
| Platform webview | Platform webview module installed | Off; feature flag |

Permissions are stored per-node in the graph metadata, not globally. This is important for privacy: a node browsing a Tor hidden service should not share circuit state with a node browsing clearnet content.

---

## 8. Relationship to Current Architecture

### What carries forward

- `Cold`/`Active` node lifecycle — maps directly to renderer-detached/renderer-attached.
- fjall append-only log — extended with `NavigateNode` entries, not replaced.
- `NodeIndex`-based graph — identity is stable regardless of address changes.
- egui tile system — the renderer's output (`as_egui_widget`) fills a tile pane.
- Intent/reducer architecture — `OpenNode`, `NavigateNode`, `CloseRenderer` intents remain the mutation boundary.

### What changes

- **Node identity**: Currently URL-based (a URL change = new node in persistence). Must migrate to UUID-based with URL as one field. This is the highest-friction migration in the whole vision.
- **Webview controller**: Becomes `RendererController` — manages a `Box<dyn RendererView>` instead of a `WebViewId`.
- **`UpdateNodeUrl` log entry**: Replaced by `NavigateNode` (from/to addresses).
- **`GraphBrowserApp::webviews` HashMap**: Becomes `active_renderers: HashMap<NodeId, Box<dyn RendererView>>`.

### What this does NOT touch

- Servo's internals — the ServoRenderer wraps the existing webview API unchanged.
- Edge semantics — edges connect nodes, not renderer instances.
- Physics/layout — purely graph topology, renderer-agnostic.
- Persistence format — fjall log extended, not replaced.

---

## 9. Why Rust and Servo's Structure Enable This

- **Trait objects**: `Box<dyn ContentRenderer>` and `Box<dyn ProtocolResolver>` give the modularity without a plugin VM.
- **Cargo features**: `--features tor,ipfs,platform-webview` keeps the default binary small; heavy optional modules are opt-in at compile time.
- **Servo's network stack**: Already abstracted behind fetch/net traits; inserting a protocol resolver layer is architecturally natural.
- **Servo's embedder API**: Already handle-based (`WebViewId`); wrapping it in a `ServoRenderer` that implements `ContentRenderer` is straightforward.
- **Arti (Rust Tor)**: First-class Rust library, maintained by the Tor Project — no FFI required for Tor integration.
- **egui's retained-mode UI**: `as_egui_widget()` is the right integration point — renderers produce widgets, not windows.

---

## 10. Implementation Path

This is an aspiration, not a roadmap. Ordered roughly by prerequisite dependency:

| Step | Work | Prerequisite |
| --- | --- | --- |
| 1 | Define `Address` enum; generalize `Node` struct to hold it | None |
| 2 | Add UUID node identity; migrate persistence to UUID-keyed log | Step 1 |
| 3 | Define `ContentRenderer` trait; wrap current webview as `ServoRenderer` | Step 1 |
| 4 | Define `ProtocolResolver` trait; wrap current HTTP fetch as `HttpResolver` | Step 1 |
| 5 | Add `FileResolver` + `DirectoryRenderer` (local file browsing) | Steps 3, 4 |
| 6 | Add `NativePdfRenderer` (PDFium or pdfrs FFI) | Step 3 |
| 7 | Add `GeminiResolver` + render gemini pages as styled text | Steps 3, 4 |
| 8 | Add `TorResolver` via system-tor SOCKS5 proxy | Step 4 |
| 9 | Add Arti (`arti-client`) as optional embedded Tor backend | Step 8 |
| 10 | Add platform webview modules (WebView2 / WKWebView) as optional features | Step 3 |
| 11 | Add IPFS resolver (gateway or local daemon) | Step 4 |
| 12 | Node history panel UI using `NavigateNode` log replay | Step 2 |
| 13 | Graph time-scrubbing UI | Step 12 |

Steps 1–4 are the core architectural migration. Steps 5–7 are the first useful extensions (filesystem + PDF + Gemini add immediate value with modest effort). Steps 8–13 are where the vision becomes distinctive.

---

## 11. Open Questions

1. **Node identity migration**: Current persistence uses URL as the stable identity key. Migrating to UUID while maintaining replay compatibility from existing fjall logs is non-trivial. What's the migration strategy — full re-import, or a compatibility shim?

2. **Renderer isolation**: Should renderer instances (especially platform webviews) run in separate processes for security/crash isolation? Servo already has this for webviews; the architecture should preserve it for other renderers where feasible.

3. **Address resolution caching**: IPFS and Tor resolution can be slow. How much caching is appropriate? Per-node? Per-session? Persistent across sessions?

4. **Content-addressed nodes**: For IPFS content, the CID *is* the address and never changes. This is actually better than URL-based identity. Should IPFS nodes use CID as their stable identity instead of UUID?

5. **Inline content types**: Should a node ever display mixed content (e.g., a Markdown file with embedded images at `file://` paths)? This requires the renderer to be aware of the resolver for sub-resource fetching.

6. **PDF renderer choice**: `pdfium-render` (PDFium via FFI, Google-maintained, best fidelity) vs `mupdf-sys` (MuPDF, lighter, AGPL). Both require a C/C++ build step. Which is the right default?

---

## 12. Default Renderer Inventory and Crate Selection

### 12.1 File Type Detection Pipeline

Two crates compose to cover the full detection space:

| Crate | Mechanism | Coverage | Notes |
| --- | --- | --- | --- |
| `infer` | Magic bytes (file header inspection) | 150+ MIME types | Pure Rust; works on streams; no file extension needed |
| `mime_guess` | File extension lookup | All common extensions | Fast; no I/O needed; fallback when no content available |

Detection pipeline at node open time:

```rust
fn detect_mime(path: Option<&Path>, bytes: Option<&[u8]>) -> Mime {
    // 1. Magic bytes (most reliable — content-based)
    if let Some(b) = bytes {
        if let Some(kind) = infer::get(b) {
            return kind.mime_type().parse().unwrap_or(mime::APPLICATION_OCTET_STREAM);
        }
    }
    // 2. Extension fallback (fast — no I/O)
    if let Some(p) = path {
        if let Some(guess) = mime_guess::from_path(p).first() {
            return guess;
        }
    }
    // 3. HTTP Content-Type header (for remote addresses — set by resolver)
    // 4. Last resort: octet-stream (binary blob, opens as download/hex view)
    mime::APPLICATION_OCTET_STREAM
}
```

### 12.2 Tier 1 — Default Renderers (Ship With Graphshell)

All Tier 1 crates are pure Rust except `pdfium-render` (see §11 Q6).

| Renderer | MIME types / address types | Crate(s) | Notes |
| --- | --- | --- | --- |
| `ServoRenderer` | `text/html`, `application/xhtml+xml`, `Http`, `File` (html) | *(existing)* | Full web engine; default for HTTP/S |
| `ImageRenderer` | `image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/bmp`, `image/tiff` | `image` | Decodes to `egui::ColorImage`; handles EXIF rotation |
| `SvgRenderer` | `image/svg+xml` | `resvg` | Pure Rust SVG renderer; rasterizes to egui texture |
| `PdfRenderer` | `application/pdf` | `pdfium-render` | PDFium via C FFI; page-at-a-time rendering; search support |
| `TextRenderer` | `text/plain`, `text/markdown`, `text/x-rust`, `text/x-python`, … | `syntect` + `pulldown-cmark` | `syntect` for 400+ language syntax highlighting; `pulldown-cmark` for Markdown→HTML |
| `DirectoryRenderer` | `File` (directory path) | stdlib only | Lists entries; click to navigate or drag to graph |
| `AudioRenderer` | `audio/mpeg`, `audio/ogg`, `audio/flac`, `audio/wav` | `symphonia` + `rodio` | `symphonia` decodes; `rodio` handles playback; minimal UI (waveform + controls) |

### 12.3 Tier 2 — Useful Extensions (Compile-In, Off by Default)

Feature-gated with `--features extended-renderers`. Each is pure Rust.

| Renderer | MIME types | Crate(s) | Notes |
| --- | --- | --- | --- |
| `JsonTreeRenderer` | `application/json`, `application/toml`, `application/yaml` | `serde_json` + `egui_extras` | Collapsible tree view; read-only; no external deps |
| `ArchiveRenderer` | `application/zip`, `application/x-tar`, `application/gzip` | `zip` + `tar` | Lists archive contents; click entry creates new node with extracted path |
| `HexRenderer` | `application/octet-stream` (fallback) | stdlib | Raw byte viewer; always available as last resort for unknown binary |

### 12.4 Tier 3 — Deferred (Too Hard or Too Heavy for This Cycle)

| Renderer | Blocker | Notes |
| --- | --- | --- |
| `VideoRenderer` | No maintained pure-Rust video decoder with egui integration | `ffmpeg-next` (C FFI, heavy) is the only realistic option; defer |
| `DocxRenderer` | DOCX parsing is XML + zip, complex layout engine needed | `docx-rs` exists but has no rendering; defer until demand is clear |
| `XlsxRenderer` | Similar to DOCX; `calamine` parses but no grid renderer | Consider JSON tree view as stopgap for structured data |
| `EpubRenderer` | ZIP of HTML files; could route through ServoRenderer with a shim | Interesting but non-trivial address mapping |

### 12.5 Renderer Feature Flags Summary

```toml
[features]
default = []                        # Tier 1 renderers always compiled in
extended-renderers = [              # Tier 2 opt-in
    "dep:zip",
    "dep:serde_json",
]
platform-webview = [                # Optional platform webviews (§3.2)
    "dep:webview2-rs",              # Windows only
]
tor = ["dep:arti-client"]           # Tor via Arti (§4.2 Path B)
ipfs = []                           # IPFS resolver selection (§4.3)
```

### 12.6 Why This Set

- **Everything in Tier 1 is pure Rust** except the PDF renderer. This keeps the build simple on all platforms (no CMake, no pkg-config surprises) for the common case.
- **`resvg` over a WebKit SVG path**: SVGs used as node icons or local diagrams should not spin up a full web engine. `resvg` handles all static SVG; only interactive/animated SVGs (rare in the filesystem context) need Servo.
- **`symphonia` over `rodeo` alone**: `symphonia` is the pure-Rust decoding layer (FLAC, MP3, OGG, WAV, AAC). `rodio` handles OS audio output. Splitting them keeps the decoder testable without hardware.
- **PDF is the one C dep in Tier 1**: PDFium (`pdfium-render`) has the best fidelity and is actively maintained by Google. MuPDF (`mupdf-sys`) is lighter and AGPL; the license may be a problem if graphshell is ever distributed commercially. Track as open question (§11 Q6).
- **No video in Tier 1**: The only realistic pure-Rust video path would route through `ffmpeg` FFI. That's a 10MB+ C dependency that affects every build for a feature most graph browsing sessions won't use. Tier 3 until proven necessary.
