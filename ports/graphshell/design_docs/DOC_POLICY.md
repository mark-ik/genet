# Documentation Policy

There are two halves of this project, and an archive. Those are the three directories:

graphshell_docs\
    Graphshell: force directed graph web browser and desktop application.
verse_docs\
    Verse: a decentralized, peer network of tokenized browsing data, search indices, and storage.
archive_docs\

## Core Principles

### 1. Control Documentation Growth

We prioritize keeping the number of docs manageable. Add information to existing docs unless the material is substantial (>500 words, trying to keep words to a minimum), covers multiple sub-topics such as to constitute a new category, and unrelated to any current documentation. When considering whether to create a new file, prioritize organization and keep things consolidated. Do not create files for one-time analyses or specific details; add to existing files when appropriate. If a more sensible organizational structure emerges, suggest it in discussion.

### 2. Eliminate Redundancy

Documentation should be organized and edited to prevent duplicate information across files. Conduct periodic (before any commit, or upon a substantial change with confirmation) audits to ensure all documentation is properly organized and non-redundant. Archival docs do not need to be kept up-to-date with the project unless directly relevant developments have occurred. Only add to archival docs when necessary (e.g., when a related feature is deprecated). If a more realistic redundancy elimination plan emerges, suggest it in discussion. Newer documents should be considered more authoritative, generally.

### 2.1 Architecture-First Evolution (No Legacy Friction)

When a new framework or architecture path is chosen, optimize for a clean fit with that path rather than preserving legacy implementations by default.

- Keep unique behavior/semantics that are hard to recreate and still align with the active architecture.
- Reimplement features instead of preserving legacy subsystems when preservation adds complexity or friction.
- Preserve fallback mechanisms only when they provide clear architectural/technical safety (for example, preventing crashes on empty runtime state), not to keep obsolete parallel systems alive.
- Do not add migration branches for historical formats unless explicitly requested.
- Keep tests focused on current semantics and current persistence schema.
- This default remains in force until an explicit product release introduces real-user migration requirements.

### 3. Documentation Location and Archival Strategy

- **Active project docs**: Store in `graphshell_docs/` or `verse_docs/`, or in subdirectories of either. Only create, delete, merge, or alter subdirectories to eliminate redundancy and make it easier to track large-scale project shifts, upon discussion and confirmation.
- **Archive structure**: `archive_docs/` contains notes and information no longer actively relevant to the project, organized in checkpoint folders (each dated to last edit of any file in the checkpoinit folder).
- **Lifecycle**: When a feature or angle is deprecated as we develop, create a doc for it in `archive_docs/`. If a feature is adopted again, move the relevant documentation back into the appropriate active doc files/folders. Delete files only upon providing a rationale and asking for confirmation.
- **Cross-referencing conventions**: Reference other docs using relative links; for cross-folder references (graphshell_docs ↔ verse_docs), include the full path in link text for clarity.

### 4. Category Organization

Subdirectories are categories. Limit categories to a maximum of ten. Start with these five core categories, and only add, delete, merge, or alter categories upon confirmation. In all docs, refer to other docs including across category when relevant for context:

1. **Technical Architecture**: Core app architecture for desktop targets, including an index, descriptions of core components (including boundaries and interfaces), dependencies, cross-component interactions, Servo integration and leveraging, IPC patterns, architectural decisions and their rationale.
2. **Design**: UI and UX documentation, interaction design, user workflows, design principles and rationale, accessibility targets and implementations, visual hierarchy and information design
3. **Features**: Briefs on features, how they're implemented, implementation guidance if not implemented (code patterns, integration points), feature-specific dependencies if any, and future possibilities.
4. **Tests**: validation tests for each feature target, performance targets/profiling/benchmarks, visual regression testing, property-based physics tests, organized by test specification and success criteria.
5. **Implementation Strategy**: Development approach, feature-gated roadmap (in order of necessity), technical details of implementation for reference, implementation status assessments.

### 5. README Requirements

Each active docs directory (`graphshell_docs/` and `verse_docs/`) must contain a unique `README.md` file at the root of that directory (not in subdirectories). Each README should include:

- Project description
- Current goals
- Index and current description of all documents in the directory
- Current build & setup guidelines for supported platforms
- References
- Additional sections only upon confirmation

### 6. PROJECT_DESCRIPTION.md Ownership

`PROJECT_DESCRIPTION.md` is reserved for the maintainer. Do not edit this file unless explicitly instructed. Consider this document authoritative, but if there are contradictions with the rest of the docs, surface them for discussion.

### 7. Upstream Dependency Maintenance: BUILD.md

The `BUILD.md` document provides cross-platform build instructions for Graphshell, which depends on Servo as its upstream browser engine. As Servo evolves, `BUILD.md` must be kept current:

- **Trigger**: When syncing upstream Servo changes:
  - Check if Rust version requirements changed (see `rust-toolchain.toml`)
  - Verify `mach bootstrap` and `mach build` commands still work
  - Test build instructions on at least one platform (preferably the maintainer's native platform, Windows (or using WSL))
  - Document any new platform requirements or tool versions

- **Update process**:
  1. Run `./mach bootstrap` and note any new dependencies or prompts
  2. Run successful build and record timing (first build and incremental)
  3. Add notes in BUILD.md like: "Updated Feb 2026: Rust 1.91.0 → 1.95.0"
  4. Test platform-specific instructions if you modified them
  5. Commit BUILD.md updates together with Servo version bumps

- **Deprecation**: If a platform becomes unsupported by Servo, mark instructions as "Deprecated" with date and move detailed instructions to `archive_docs/` while keeping a reference in BUILD.md

### 8. Implementation Planning Documents: Feature-Driven Organization

Checklists, task lists, and implementation planning documents should be organized by **feature targets and validation tests**, not by calendar time (days, weeks, or dates). This approach provides flexibility in actual development while maintaining clear acceptance criteria.

**Structure for task documents:**

- **Feature Target**: Name the capability (e.g., "Feature Target 1: Understand Architecture Foundation")
- **Context**: Brief explanation of why this target matters
- **Tasks**: Step-by-step work items (without time estimates or "Day 1/Monday" labels)
- **Validation Tests**: Specific, measurable criteria that prove the feature target is complete
- **Outputs**: Deliverables (documents, code, diagrams) that result from completing this target
- **Success Criteria**: Knowledge validation questions that confirm deep understanding

### Workflow Documentation Rule

For AI assistants: store documentation (including memories, instructions, plans, etc.) in doc_policy per this file's rules. Refer to doc_policy for context when needed. This also means that the content of agent-specific instruction folders should exist in doc_policy (such as .claude folders).

When asked to do a project (like implementing a feature, or trying to accomplish something documented as a planned task in design_docs) that requires changing the codebase (not the docs), create a markdown file in the relevant design_docs directory with the current date, a keyword related to the task (such as the feature being implemented), and the suffix _plan. Include in this file three sections:

- (keyword) Plan: track phases and progress
- Findings: store research and findings
- Progress: session log and test results

Update the file every two prompts related to the project, or every two tasks you complete related to the project. Update the file upon completing the project, and move it to archive docs (creating a timestamped folder if none has been made for the day). Reread the relevant file before working on the same project, if the _plan file exists.
