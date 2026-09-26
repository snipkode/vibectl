# Tasks — Spec-Driven Development

- [x] 1. Add `src/agent/spec.rs` with `Spec`, `TaskItem`, `slugify`
- [x] 2. Implement `parse_tasks`: checkbox lines only, nesting, 1-based numbers
- [x] 3. Implement `Spec::load` / `Spec::create` with phase gating
- [x] 4. Error on a `tasks.md` that exists but yields zero checkboxes
- [x] 5. Deduplicate slugs with a numeric suffix instead of clobbering
- [x] 6. Add `.gitignore` negation so `.vibectl/specs/` is committable
- [x] 7. Add the spec context builder: pending-only, capped, counts stated
- [x] 8. Replace the `/spec` stub in `src/tui/mod.rs` with the real command
- [x] 9. Wire `/spec add|design|tasks|done` and the status report
- [x] 10. Inject spec context into the agent turn in `src/agent/mod.rs`
- [x] 11. Tests: parser, slug, gating, cap, dedup, empty-plan error
- [x] 12. Render per-task status in the TUI plan panel
- [x] 13. Update README to describe `/spec` and the specs layout
- [x] 14. Audit: recognise `requirements.md` / `design.md` / `tasks.md`
