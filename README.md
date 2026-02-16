# graphshell

    An open source, prototype, spatial browser that represents webpages as nodes in a force-directed graph

- Force-directed node graph canvas with adjustable physics and node/edge criteria enabling many graph topologies
- Servo-powered web rendering in viewports powered by egui_tiles panes/tabs structured by graph layout
- Clipping: DOM inspection & element extraction from nodes into graph UI as nodes
- Local-first, permissions-based P2P co-op browsing

## AI Disclaimer

First, a disclaimer: I use and have used AI to support this project.

The idea itself is not the product of AI. I have years of notes in which I drafted the graph browser idea and the decentralized network component. I iterated my way into the insight that users should own their data, not be tracked, and we ourselves can capture much richer browsing insights than trackers. That's the second, prospective half of this project, the Verse bit.

I'm not an experienced developer in the least but I've got opinions, a smidgen of coding experience, and honestly, I want to learn how to use these discursive tools and see how far I can get with them. I've also followed the Servo community for years, despite not being a real developer: please contribute if you are able!

This is an open source, non-commercial effort. These ideas work much better open source forever as far as I'm concerned.

## History

My first inkling of this idea actually came from a mod for the game Rimworld, which added a relationship manager that arranged your colonists or factions spatially with links defining their relationships. It occurred to me that this UI, reminiscent of a mind map, would be a good fit for representing tabs spatially, and that there were a lot of rule-based options for how to arrange not just the browsing data, but tons of data patterns in computing.

I learned there was a name for this sort of UI: a force-directed node graph.  a repeating, branching pattern of nodes connected to nodes by lines (edges). The nodes are browser tabs (or any file, document, applet, application, etc.), edges represent the relationship between the two nodes (clicked hyperlink, historical previous-next association, user-associated), and all nodes have both attractive and repellant forces which orient the graph’s elements.

Depending on the behavior you want from the graph or the data you’re trying to represent, you alter the canvas's physics and node/edge rules/types. You could filter, search, create new rules and implement graph topologies conducive to representing particular datasets: trees, buses, self-closing rings, etc.

This leads to rich, opinionated web browsing datasets, and the opportunity to pool our resources to visualize the accessible web with collective browsing history that is anonymous, permissions- and reputation-based, peer-to-peer, and open source. The best implementation of both halves would be somewhere between federated googles combined with subreddits with an Obsidian-esque personal data management layer.

Other inspirations:

- The Internet Map <https://internet-map.net/>
- YaCy (decentralized search index)
- Syncthing (open source device sync)
- Obsidian (canvas, plugins)
- Anytype (IPFS, shared vaults)

## Planned Features

### Graph UI: visual representation of a browsing session. No more history list: you could just look and see where you’re at and where you’ve been

- Organize web resources beyond webpages: documents, notes, files, and applets coexist in graph as nodes!
- Lasso Zoning: prescribe exclusionary or inclusionary sections of the graph for specific access/domains
- Rule-based node motility: physics system allows nodes to organize themselves according to your rules/the graph structure
- Auto grouping:
- Active, warm, and cold node states with memory pressure demotion for resource efficiency
- Origin-grouped processes
- Minimap for large graphs/reference in detail view (useful for persistent, public graphs)
- Hotswitch between 2D and 3D versions of the canvas, preserving connections and relative positions of nodes.
- - 2D version: ideal for dense maps or devices with limited capabilities
- - 3D version: full 3D (reorientable camera), stacked 3D (layers of depth, not arbitrary deep), or soft 3D (nonreorientable camera)
- - Level-of-detail rendering prevents information overload; zooming out groups by selectable categories, such as time, domain, origin, relatedness, or other sorting rules.
- Mods: use the rules someone else set up for their graph, like physics parameters, custom node/edge/filter types, canvas region definitions, or extend the capabilities of the app.

### Detail View

- When a node is opened, you get the familiar tab ui, with tabs organized according to your graph, recency, chronology, relatedness, domain, or origin, or other sorting rules.
- Clipping: DOM inspection & element extraction from detail view into graph UI as nodes
- Split view: viewport split between graph and detail view(s)
- Groups of nodes connected to a hub node would be recognized as a collapsible group
- Drag and reorganize support reflected in graph structure (drag a tab onto a tab to link them in the graph).
- Tiling view manager, allowing multiple webviews to exist within the graphshell context.

### Sessions

- Ean be deleted, saved, shared, and manipulated as needed
- Editing: change the relations between nodes; use ghost nodes to represent deleted nodes while maintaining the graph’s shape. Multiple edge and node types.
- You can retain a previous session's graph or start anew at will
- Graphs can be exported as JSON (preserving structure and metadata), interactive HTML, or into other browsers, enabling data portability.
- Individual nodes can be shared as standard URLs with embedded metadata cards
- Bookmarks and browsing history can be imported to seed a new graph

### Ergonomics

- - View-specific keybinds/hotkeys
- - Graph search, filtering,
- - WASD panning in graph view
- - Arrow keys to move focus between every interactable UI element, within category (nodes, edges, menu items, buttons)
- - Edge and node types differentiated by line style, shape, color, and icon
- - Convert graphs into lists for screen reader software to parse

### P2P Co-op Browsing

- - Make browsing a collaborative activity, where the changes one person makes to a shared graph are synchronized with the rest of the participants.
- - Async: check in/check out model with diffs
- - Live: Version-controlled, realtime edits with time synchronized web processes (to watch netflix with the homies, research, cooperatively use web-hosted applications...)

### Verse

    Optional, decentralized network component

- Selective tokenization of browsing data enabling portability, management, encryption, and distribution.
- - Portability: stored in crypto wallet, accessible everywhere the wallet is.
- - Management: access, share, trade, transfer, process your data with cryptographic privacy guarantees.
- - Encryption: even if you have no interest in sharing your data, you should be able to encrypt it for guaranteed privacy. No trackers
- Integration with IPFS allows persistent, decentralized hosting of public graphs, indices (searchable, mutually composable layers for collections of graphs)
- Storage-backed fungible token: token issuance rates tied to amount of storage provided (calculated and issued at time thresholds) and host reputation (uptime + recent activity + peers/seeders).

### Peer roles

- Create and optionally, conditionally (free, cost, time-limited, etc.) publish reports (default anonymous or opt-in signing to build reputation).
- - Check report token performance on network (seeds/peers) using an identifier asssociated with a receipt token (created when publishing a tokenized report) and the report.
- - Permissions-based, cryptographically-enforced access rules for published reports/graphs/indices.
- - Retain (or purge) a ledger of receipts.
- - If compensated for report, essentially revoke your report's access/use on demand for the amount paid, otherwise do so free.
- - Sign reports with a (regenerable for after the fact disambiguation) user identifier to build reputation, or don't.
- Host storage and selectively rebroadcast data, leveraging black/whitelists.
- Index data for efficient parsing and queries.
- Provide attestations, black/whitelists, and integrity checks of reports, graphs, and indices, leveraging reputation.
- Stake fungible tokens to make a channel (verse) of organized, persistent graphs, indices (a custom search portal!), and applets, addressed by tag in IPFS.
- Participate in channel governance by staking your tokens to the verse’s stake, growing the pot the verse can offer to user contributers and incentivizing communal governance.
- - Semantic browsing suggestions, the equivalent of desire paths for browsing, communally sourced.
- - Voluntary or compensated sharing/forking of user graphs, reminiscent of git.
- - Scrape-resistant network topology requiring permission and potentially compensation for access.
