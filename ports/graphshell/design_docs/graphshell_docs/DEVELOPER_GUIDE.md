# Graphshell Developer Guide

**Last Updated:** February 11, 2026  
**For:** New contributors and AI assistants  
**See Also:** [QUICKSTART.md](QUICKSTART.md), [ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md)

---

## Quick Orientation

**Graphshell** is a spatial browser built on Servo where webpages are nodes in a force-directed graph.

- **Location:** `ports/graphshell/` (~7,000 LOC, 137 tests)
- **Status:** Phase 1 refinement complete (11/11 steps)
- **Active Work:** Feature Target 2 (Thumbnails & Favicons)

---

## Essential Commands

### Build & Run
```bash
# Build (release mode recommended)
./mach build -r graphshell

# Run
./mach run -r graphshell -- https://example.com

# Run with logging
RUST_LOG=debug ./mach run -r graphshell

# Clean build (if stuck)
./mach clean
```

### Testing
```bash
# Run all graphshell tests
./mach test-unit graphshell

# Run specific test
cd ports/graphshell && cargo test test_name --lib

# Count passing tests
cargo test --lib 2>&1 | grep "test result"
```

### Code Quality
```bash
./mach fmt           # Format code
./mach clippy graphshell  # Lint
cargo check          # Check compilation
```

---

## Code Conventions

### Required Practices

1. **UTF-8 Safety:** Always use `util::truncate_with_ellipsis()` for string truncation
2. **Persistence Discipline:** Every mutation must call `log_mutation()` before applying
3. **Test Coverage:** Every bug fix needs a regression test
4. **URL Identity:** URLs are stable keys; NodeIndex is a session-local handle
5. **Tests in Same File:** Use `#[cfg(test)]` modules in implementation files

### Architecture Constraints

- **No breaking Servo core:** Graphshell changes isolated to `ports/graphshell/`
- **NodeKey stability:** petgraph StableGraph ensures NodeKey survives deletions
- **Webview mapping:** `webview_to_node` and `node_to_webview` are inverses
- **Physics sync:** Worker graph must match app graph for positions to be valid

---

## Module Map (Quick Reference)

### Core Data (~600 LOC)
- **`graph/mod.rs`** (461 lines) — StableGraph wrapper, Node/Edge types
- **`graph/egui_adapter.rs`** (163 lines) — Graph → egui_graphs conversion

### Physics (~606 LOC)
- **`physics/mod.rs`** (385 lines) — Force-directed engine
- **`physics/worker.rs`** (221 lines) — Background thread

### UI (~1,477 LOC)
- **`desktop/gui.rs`** (794 lines) — Servo integration, webview lifecycle
- **`desktop/webview_controller.rs`** (278 lines) — Webview lifecycle/sync helpers
- **`render/mod.rs`** (467 lines) — egui_graphs rendering, events
- **`input/mod.rs`** (216 lines) — Keyboard shortcuts

### State & Persistence (~1,760 LOC)
- **`app.rs`** (1010 lines) — Application state, view model
- **`persistence/mod.rs`** (518 lines) — fjall log + redb snapshots
- **`persistence/types.rs`** (232 lines) — LogEntry variants, serialization

### Utilities
- **`util.rs`** (66 lines) — String truncation, utilities

**See [CODEBASE_MAP.md](CODEBASE_MAP.md) for detailed module breakdown.**

---

## Common Development Tasks

### Add a Graph Mutation (Full Cycle)

**1. Define LogEntry variant** (`persistence/types.rs`)
```rust
#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub enum LogEntry {
    // ... existing variants
    YourMutation { url: String, data: YourData },
}
```

**2. Add replay logic** (`persistence/mod.rs`)
```rust
ArchivedLogEntry::YourMutation { url, data } => {
    if let Some((key, _)) = graph.get_node_by_url(url.as_str()) {
        graph.apply_mutation(key, data);
    }
},
```

**3. Wire in app.rs**
```rust
pub fn your_mutation(&mut self, key: NodeKey, data: YourData) {
    if let Some(store) = &mut self.persistence {
        store.log_mutation(&LogEntry::YourMutation {
            url: self.graph.get_node(key)?.url.clone(),
            data: data.clone(),
        });
    }
    self.graph.apply_mutation(key, data);
    self.egui_state_dirty = true;
}
```

**4. Add tests**
```rust
#[test]
fn test_your_mutation_persists() {
    let (mut store, _dir) = create_test_store();
    store.log_mutation(&LogEntry::YourMutation { ... });
    let graph = store.recover().unwrap();
    // Verify mutation was applied
}
```

### Add a Keyboard Shortcut

**1. Add to `input/mod.rs`**
```rust
if !ui_has_focus(ctx) && ctx.input(|i| i.key_pressed(egui::Key::Y)) {
    app.your_action();
    return true;
}
```

**2. Document in [QUICKSTART.md](QUICKSTART.md)**

**3. Add test**

---

## Debugging Patterns

### Physics Issues

**Nodes not moving:**
```rust
// Check if paused
if !app.physics.is_running() { warn!("Physics paused"); }

// Check for NaN
if !node.position.is_finite() { error!("NaN position"); }
```

**Enable physics panel:** Press `P` key for live config

### Persistence Issues

**Changes not persisting:**
```rust
// Verify logging
if let Some(store) = &mut self.persistence {
    store.log_mutation(&LogEntry::YourMutation { ... });
}
```

**Add debug logging in `persistence/mod.rs::replay_log()`**

### Webview Issues

**Webview not appearing:**
```rust
// Check mapping
if let Some(webview_id) = app.node_to_webview.get(&node_key) {
    log::info!("Mapped: {:?} -> {:?}", webview_id, node_key);
}

// Check view state
match app.view {
    View::Graph => log::info!("Graph view - webviews destroyed"),
    View::Detail(key) => log::info!("Detail view: {:?}", key),
}
```

### Rendering Issues

**Graph not updating:**
```rust
app.egui_state_dirty = true;  // Force rebuild
```

**Low FPS:**
- Check node count (target: 500 @ 45 FPS)
- Profile with `RUST_LOG=debug`
- Consider viewport culling (not yet implemented)

---

## Current Work Status

**Phase:** Phase 1 Refinement complete (11/11 steps)  
**Next:** Feature Target 2 (Thumbnails & Favicons)

### Recent Changes (Session Feb 11, 2026)
- ✅ Phase 1 refinement complete (Steps 1-11)
- ✅ Webview controller extracted, persistence UX added
- Test count: 80 → 137 (all passing)

### Known Issues (Post-Refinement)
1. **gui.rs still large** (~794 lines) — consider further decomposition
2. **No unit tests for gui.rs/webview_controller.rs** — integration only
3. **Thumbnails/favicons not implemented** — Feature Target 2

---

## Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Nodes @ 45 FPS | 500 | Not measured (benchmarks pending) |
| Nodes @ 30 FPS | 1000 | Not measured |
| Test coverage | 90%+ | 137 tests (gui.rs lacks unit tests) |
| Startup time | <2s | Not measured |

---

## Git Workflow

### Before Committing
```bash
./mach fmt                  # Format
./mach clippy graphshell    # Lint
./mach test-unit graphshell # Test
git add -A ports/graphshell
git commit -m "Step X: Summary..."
```

### Commit Message Format (Follow Recent Pattern)
```
Step X: Short summary (50 chars max)

## Changes

### Category 1
- Bullet detail
- Another detail

### Test Status
- Test count: X -> Y (all passing)
```

---

## Troubleshooting Checklist

### Build Fails
- [ ] Run `./mach clean`
- [ ] Check Rust version: `rustc --version` (need 1.91.0+)
- [ ] Check disk space (~25GB needed)
- [ ] Try debug build: `./mach build -d graphshell`

### Tests Fail
- [ ] Use `TempDir` for test isolation
- [ ] Use `new_for_testing()` instead of `new()`
- [ ] Run single test: `cargo test test_name -- --nocapture`

### Runtime Crash
- [ ] Check for NaN positions
- [ ] Verify char-aware string truncation
- [ ] Check persistence replay cases
- [ ] Enable `RUST_LOG=debug`

---

## Resources

### Documentation
- **[README.md](README.md)** — Project overview
- **[ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md)** — Implementation details
- **[IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md)** — Feature targets
- **[QUICKSTART.md](QUICKSTART.md)** — Command reference
- **[CODEBASE_MAP.md](CODEBASE_MAP.md)** — Detailed module map

### Crates
- [petgraph 0.8](https://docs.rs/petgraph/0.8/)
- [egui 0.33.3](https://docs.rs/egui/0.33.3/)
- [egui_graphs 0.29](https://docs.rs/egui_graphs/0.29/)
- [fjall 3](https://docs.rs/fjall/3/)
- [redb 3](https://docs.rs/redb/3/)
- [rkyv 0.8](https://docs.rs/rkyv/0.8/)

### Servo
- [API Documentation](https://doc.servo.org/servo/)
- [GitHub](https://github.com/servo/servo)
- [Style Guide](../../../../STYLE_GUIDE.md)
