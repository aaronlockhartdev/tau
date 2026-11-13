# Delta: surface/components

## ADDED

### R-surface-01  The component library stays in the app crate

WHERE the desktop app builds THE SYSTEM SHALL keep the component library out of `crates/tau-core`.

Acceptance: gate core has no UI logic

### R-surface-02  The pin is a library version, not a Zed revision

THE SYSTEM SHALL take GPUI through a published component-library release, and SHALL NOT depend on a Zed source revision.

Acceptance: gate workspace layout
Why: the library pins weekly `gpui-pre` snapshots of Zed main, so upstream breaks arrive as a library release.

### R-surface-03  Library widgets, not hand-written controls

WHEN the desktop app renders a transcript, a diff, or a prompt field THE SYSTEM SHALL use the library's `TextView`, `Diff`, and `Input` widgets.

Acceptance: @R-surface-03

### R-surface-04  Heavy library features stay opt-in

WHEN the app crate declares the library THE SYSTEM SHALL keep tree-sitter grammars and `syntect` out of the default feature set.

Acceptance: gate workspace layout

### R-surface-05  Core parses markdown

WHEN core receives assistant text THE SYSTEM SHALL parse it with `pulldown-cmark`.

Acceptance: @R-surface-05
Why: a headless session renders the transcript too, so the parse cannot live in the library.

### R-surface-06  Interactive controls expose accessibility semantics

WHEN a control accepts focus THE SYSTEM SHALL expose its semantics through AccessKit.

Acceptance: gate desktop

### R-surface-07  Fallback when a closed block cannot hold layout

IF the surface spike shows `TextView` unable to retain a closed block's layout while the tail re-parses THEN THE SYSTEM SHALL style the unstyled layer's `TextView` in tau.

Acceptance: gate desktop
