# Frontend UX and Performance Handoff

## Current repository state

The connected `un1c0` repository contains the Rust agent kernel and the Vault/admin-service operational stack, but no `mockup.html`, frontend source directory, `package.json`, `index.html`, Vite configuration, or Playwright configuration. The UX work described in the user request therefore cannot be applied safely to the absent artifact without inventing or replacing an interface that is not part of this checkout.

## Ready-to-apply acceptance budget

When the mockup or frontend package is added, preserve the existing 25+ panels, theme system, tier-gating, SSE lifecycle, and Markdown behavior while enforcing the following budgets:

| Surface | Budget or invariant |
|---|---|
| Initial HTML and critical CSS | Revalidate on every deploy; no long-lived cache for runtime configuration |
| Hashed static assets | `Cache-Control: public, max-age=31536000, immutable` |
| Optional panels | Lazy-load charts, inspectors, settings, and editor modules on first open |
| SSE | One cancellable connection per view; bounded reconnect backoff and event buffer |
| Markdown | Allowlist rendering; escape raw fallback text and unsafe URLs |
| Lists and logs | Bound retained events and virtualize unbounded rows |
| Interaction | Keyboard focus, reduced-motion, narrow viewport, offline, empty, and recovery states |
| Release gate | Record bundle size and startup smoke metrics; fail on unexplained regression |

## Integration sequence

1. Attach or restore the real frontend artifact and identify its build command.
2. Add a port-isolated local preview command and a Playwright or equivalent browser test project.
3. Split optional modules at panel boundaries, add immutable asset headers, and cap SSE/DOM retention.
4. Run desktop, narrow viewport, keyboard, reduced-motion, offline-reconnect, theme, tier-gate, malformed-Markdown, and stream-cancellation tests.
5. Record baseline and post-change bundle/startup metrics in CI before enabling a hard budget.
