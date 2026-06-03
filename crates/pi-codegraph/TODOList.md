# pi-codegraph TODOList

## Implementation Order

- [x] 1. Persist project indexes in `<project>/.pi-coding/db.sqlite`.
  - Create SQLite schema for files, symbols, calls, and metadata.
  - Store extraction fingerprints so unchanged files are skipped.
  - Keep database creation local to the project root.
- [x] 2. Add project sync APIs.
  - Full init/sync for supported files.
  - Incremental sync for changed paths.
  - Removal handling for indexed files that no longer exist.
- [x] 3. Add query APIs matching codegraph-style behavior.
  - `codegraph_search`
  - `codegraph_callers`
  - `codegraph_callees`
  - `codegraph_impact`
  - `codegraph_node`
  - `codegraph_trace`
- [ ] 4. Expose Pi built-in tools for the query APIs.
  - Tool schemas and handlers.
  - Automatic index sync before read-only queries when enabled.
- [ ] 5. Add project-open auto init/sync controls.
  - Default enabled.
  - Public config can disable auto init/sync.
  - Disabled projects can still trigger sync manually.
- [ ] 6. Add manual trigger surfaces.
  - CLI command for sync/query smoke checks.
  - TUI command to trigger indexing when auto init is disabled.
- [ ] 7. Add project file change monitoring.
  - Detect file changes when no external watcher exists.
  - Incrementally sync changed files.
  - Keep watcher scoped to supported source files.
- [ ] 8. Add validation coverage.
  - Unit tests for SQLite persistence and query APIs.
  - Integration coverage for built-in tool behavior.
  - Cargo format, check, clippy, and targeted tests.
