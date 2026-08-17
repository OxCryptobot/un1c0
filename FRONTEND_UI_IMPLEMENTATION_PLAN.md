# Frontend UI Implementation Plan

## Scope and prerequisite

The current repository does not contain the referenced `mockup.html`, a frontend source directory, `package.json`, Vite configuration, `index.html`, or Playwright configuration. This plan is therefore an implementation-ready handoff rather than a speculative UI rewrite. The first frontend commit must attach or restore the real mockup artifact and record its build and preview commands.

## Target architecture

Preserve the existing 25+ panels, theme support, tier-gating, real-time SSE behavior, and Markdown rendering. Introduce explicit boundaries for the application shell, navigation, panel registry, server-state cache, SSE transport, Markdown safety adapter, tier policy, theme state, and optional panel modules. Keep secrets and runtime configuration server-side; the browser receives only the minimum public configuration.

| Layer | Responsibility | Required invariant |
|---|---|---|
| Shell | Layout, navigation, responsive breakpoints, theme | Keyboard reachable and usable at narrow widths |
| Panel registry | Panel metadata, tier requirement, lazy loader | Unknown panels fail closed |
| Data client | Fetch/cache/retry/cancel | No duplicate requests or unbounded cache |
| SSE manager | One stream per view, reconnect, dedupe, cancel | No orphaned connections or unbounded event history |
| Markdown adapter | Safe parsing and rendering | No arbitrary HTML or unsafe URLs |
| Tier policy | Entitlements and gated states | Client gating is presentational; server remains authoritative |
| Performance layer | Code splitting, cache headers, list virtualization | Optional features do not block first paint |

## Implementation phases

### Phase A — Restore and baseline the UI

Attach the real `mockup.html` or frontend package. Identify the current build, preview, and test commands. Capture screenshots and a baseline for bundle size, first render, time to interactive, and stream-to-first-event. Record panel count, theme variants, tier-gated interactions, and current SSE/Markdown behavior before modifications.

### Phase B — Establish state and safety boundaries

Create a typed panel registry with `id`, `title`, `tier`, `loader`, and `availability` fields. Move theme state into a single store with system preference detection, persisted user choice, no-flash initialization, and reduced-motion support. Wrap SSE in a cancellable manager using `AbortController`, a capped reconnect schedule, event IDs for deduplication, a bounded ring buffer, and explicit `connecting`, `live`, `reconnecting`, `offline`, and `closed` states.

Treat all Markdown as untrusted. Allow only the required formatting nodes, sanitize links to approved protocols, escape raw fallback text, and never pass model or server content to `innerHTML` without a sanitizer. Keep tier gating visible and actionable while enforcing authorization on the server.

### Phase C — Polish the interaction model

Add consistent loading skeletons, empty states, actionable errors, offline recovery, permission denial, and tier-upgrade states. Ensure visible focus rings, logical tab order, semantic landmarks, labelled controls, accessible live updates, minimum touch targets, and full keyboard operation. Verify theme transitions, narrow viewport layout, high-contrast behavior, and reduced-motion behavior.

### Phase D — Split and optimize optional panels

Lazy-load charts, inspectors, settings, editors, and other optional panels on first open. Add suspense or skeleton boundaries so the shell remains interactive. Use immutable hashed asset caching for static resources while revalidating HTML and runtime configuration. Bound log/event retention and virtualize long lists. Debounce search and avoid registering duplicate event handlers on panel reopen.

### Phase E — Browser E2E and release gates

Add a port-isolated preview command and Playwright or equivalent browser tests. Cover desktop and narrow viewports, theme persistence, keyboard navigation, reduced motion, tier gates, empty/error/offline states, SSE reconnect and cancellation, event ordering, malformed Markdown, unsafe URL rejection, and lazy-panel loading. Store traces and screenshots only on failure and redact credentials and source contents.

## Acceptance criteria

The frontend is ready for release only when the original panel inventory and critical workflows remain intact, the UI is usable from keyboard and narrow screens, theme and tier state survive reloads, SSE connections close cleanly, malformed Markdown cannot inject markup, optional modules do not block first paint, and bundle/startup metrics meet the baseline budget. Any regression must be explained in the pull request and recorded in the release evidence.

## Immediate next action

Provide the actual `mockup.html` or frontend package. Once present, implement Phase A first and do not enable hard performance budgets until the baseline has been recorded.
