# RemoteX Apple-Style UI Refinement Plan

Scope: frontend presentation and interaction polish only. The remote-control backend, transport, encryption, agent lifecycle, permission semantics, and updater behavior remain unchanged.

1. Replace the Home card stack with two compact, flat work areas: **This Device** and **Connect to a Device**. Show only useful state, remove the nested device-ID card and persistent onboarding panel, and keep one dominant connection action.
2. Simplify the shell: separate primary and secondary navigation with whitespace, remove the decorative eyebrow and redundant Home title bar, use a subtle selected row, and retain native Windows window behavior.
3. Consolidate the visual system around graphite semantic tokens, Windows system fonts, one blue accent, separators instead of containers, neutral off toggles, and short reduced-motion-safe transitions.
4. Flatten Settings into scan-friendly rows with trailing controls, preserve existing bindings, and make advanced network options quiet disclosures. Keep the active-session video dominant with a compact toolbar.
5. Verify keyboard focus, switch semantics, English/Simplified Chinese layout, common desktop sizes (1280x720, 1366x768, 1920x1080), frontend build, Rust tests, and a local installer build. Do not publish, tag, or release.
