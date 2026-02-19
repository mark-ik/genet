# graphshell

    An open source, prototype, spatial browser that represents webpages as nodes in a force-directed graph

- Force-directed graph canvas with Servo-powered web rendering
- Tiled multi-pane workspace: graph overview and webview panes, side by side
- Local-first persistent browsing graph with crash-safe recovery
- Event-driven navigation semantics from Servo delegate callbacks

## Currently Implemented

### Graph UI

- Force-directed graph canvas: webpages are nodes, navigation and associations are edges
- Zoom, pan, and fit-to-screen camera controls
- Thumbnail and favicon rendering on nodes with tiered fallback (thumbnail > favicon > lifecycle color)
- Fuzzy search and filtering across node titles and URLs (nucleo, fzf-like scoring)
- Node selection, creation, deletion, and edge creation from graph interactions
- View-specific keyboard controls (guarded when text fields are focused)

### Tiled Workspace

- egui_tiles multi-pane layout: graph pane and webview panes coexist in a tiling tree
- Per-pane tab bars with close buttons and focus management
- Active/cold node lifecycle: webviews created on demand, destroyed when not visible
- Omnibar with graph search (@query) and URL navigation, routed to the active tile's webview

### Servo Integration

- Full webview lifecycle: create, navigate, destroy, track URL and title changes
- Navigation tracking creates graph edges (Hyperlink for clicks, History for back/forward)
- Favicon ingestion from Servo's page metadata
- Thumbnail capture from webview rendering output

### Persistence

- Crash-safe local storage: fjall append-only mutation log + redb periodic snapshots + rkyv serialization
- Startup recovery: load latest snapshot, replay log entries since snapshot
- Explicit graph reset (clear data and start fresh)

## In Development

### Navigation Control Plane - COMPLETE

- Wire Servo delegate callbacks (notify_url_changed, notify_history_changed) as primary mutation source
- Remove polling-based node creation from sync_to_graph
- Unify intent-based mutation boundary across graph, tile, and webview layers

### Physics Migration - COMPLETE

- Replace custom physics engine with egui_graphs built-in force-directed layout
- Remove kiddo spatial index and background physics worker thread

### Selection Consolidation

- Single-source selection state in app, projected to graph rendering
- Remove duplicated selection tracking from node model

## Planned

### Near-term

- Bookmarks and browsing history import to seed graph from existing browser data
- Performance optimization targeting 500 nodes at 45fps, 1000 nodes at 30+fps
- Node identity migration from URL-based to UUID-based (enabling duplicate URL tabs)
- History edge creation from navigation history metadata
- Stop-loading control wired to Servo API
- Accessibility bridge for screen readers

### Graph UI (future)

- Rule-based node motility: physics system organizes nodes according to rules and graph structure
- Lasso zoning: prescribe exclusionary or inclusionary sections for specific access or domains
- Active, warm, and cold node states with memory pressure demotion
- Level-of-detail rendering: zooming out groups nodes by time, domain, origin, or relatedness
- Minimap for large graphs
- 2D/3D canvas modes

### Detail View (future)

- Clipping: DOM inspection and element extraction from webpages into graph as independent nodes
- Collapsible groups from hub-connected node clusters
- Drag-and-reorganize reflected in graph structure

### Sessions (future)

- Graph export as JSON, interactive HTML, or portable format
- Individual nodes shareable as standard URLs with metadata
- Ghost nodes to represent deleted nodes while preserving graph shape

### Ergonomics (future)

- Arrow key focus traversal across all interactable elements
- Edge and node types differentiated by line style, shape, color, and icon
- Graph-to-list conversion for screen reader accessibility
- Mods: shareable physics parameters, custom node/edge/filter types, canvas region definitions

## Verse

    Optional, decentralized network component (design phase)

The second half of the project: pooling browsing data into a decentralized, permissions-based peer network.

### P2P Co-op Browsing

- Collaborative browsing where changes to a shared graph synchronize across participants
- Async mode: check in/check out with diffs
- Live mode: version-controlled realtime edits with time-synchronized web processes

## SPECULATION

### Tokenized Browsing Data

- Selective (you pick what) tokenization enabling portability, management, encryption, and distribution
- Portability:
- - store in crypto wallet, accessible everywhere the wallet is;
- - synced from your local device via syncthing, ipfs, other protocols;
- - store with and acceess from a storage provider, p2p, or vendor
- Management: access, share, trade, transfer, process your data with cryptographic privacy guarantees
- Encryption: reliable, file-level, default key-based encryption.

### Network Infrastructure

- IPFS integration for persistent, decentralized hosting of public graphs and indices
- Storage-backed fungible token: issuance rates tied to storage provided and host reputation
- Channels (verses): organized, persistent graphs and indices addressed by tag in IPFS

### Peer Roles

- Create and optionally publish reports (anonymous or signed for reputation)
- Permissions-based, cryptographically-enforced access rules
- Host storage and selectively rebroadcast data
- Index data for efficient parsing and queries
- Provide attestations and integrity checks leveraging reputation
- Stake tokens to create and govern channels
- Semantic browsing suggestions: communally sourced desire paths for the web

## AI Disclaimer

First, a disclaimer: I use and have used AI to support this project.

The idea itself is not the product of AI. I have years of notes in which I drafted the graph browser idea and the decentralized network component. I iterated my way into the insight that users should own their data, not be tracked, and we ourselves can capture much richer browsing insights than trackers. That's the second, prospective half of this project, the Verse bit.

I'm not an experienced developer in the least but I've got opinions, a smidgen of coding experience, and honestly, I want to learn how to use these discursive tools and see how far I can get with them. I've also followed the Servo community for years, despite not being a real developer: please contribute if you are able!

This is an open source, non-commercial effort. These ideas work much better open source forever as far as I'm concerned.

## History

My first inkling of this idea actually came from a mod for the game Rimworld, which added a relationship manager that arranged your colonists or factions spatially with links defining their relationships. It occurred to me that this UI, reminiscent of a mind map, would be a good fit for representing tabs spatially, and that there were a lot of rule-based options for how to arrange not just the browsing data, but tons of data patterns in computing.

I learned there was a name for this sort of UI: a force-directed node graph. A repeating, branching pattern of nodes connected to nodes by lines (edges). The nodes are browser tabs (or any file, document, applet, application, etc.), edges represent the relationship between the two nodes (clicked hyperlink, historical previous-next association, user-associated), and all nodes have both attractive and repellant forces which orient the graph's elements.

Depending on the behavior you want from the graph or the data you're trying to represent, you alter the canvas's physics and node/edge rules/types. You could filter, search, create new rules and implement graph topologies conducive to representing particular datasets: trees, buses, self-closing rings, etc.

This leads to rich, opinionated web browsing datasets, and the opportunity to pool our resources to visualize the accessible web with collective browsing history that is anonymous, permissions- and reputation-based, peer-to-peer, and open source. The best implementation of both halves would be somewhere between federated googles combined with subreddits with an Obsidian-esque personal data management layer.

Other inspirations:

- The Internet Map <https://internet-map.net/>
- YaCy (decentralized search index)
- Syncthing (open source device sync)
- Obsidian (canvas, plugins)
- Anytype (IPFS, shared vaults)
