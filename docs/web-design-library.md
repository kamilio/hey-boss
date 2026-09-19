# Web design library

The Issues page is the visual reference for Inbox, Issues, Workers and Mindmaps.
Every route renders `src/issues/web/app-shell.html` and loads `components.css`
and `components.js`. The server sets the current navigation item. Inbox lives
within the Issues client and uses the same shell and controls.

`components.css` owns the light/dark theme tokens, typography, page width and
responsive spacing, header/navigation, project picker, focus states and reduced
motion/transparency preferences. Reuse these classes when adding UI:

| Element | Classes |
| --- | --- |
| Page title and supporting text | `page-heading`, `eyebrow`, `page-description` |
| Action | `button`, optionally `primary`, `small` or `danger` |
| Icon action | `icon-button` with an accessible label |
| Toolbar and search | `toolbar`, `search-field` wrapping an input |
| Filters | `filter-controls`, `select-control` |
| Standalone input | `text-input` |
| Content surface | `glass` |
| Page footer | `page-footer`, `footer-mark`, `footer-product` |

`HeyBossUI` supplies icons and the accessible `ProjectPicker`. Page clients own
loading and mutations, passing projects and the selected project to the picker.
Workers display the entire fleet; its picker sets the project context for links
back to Issues and Mindmaps, rather than filtering machine availability.

Keep page-specific content geometry in `app.css`, `fleet.css` or `mindmap.css`.
Do not redefine the theme, shell or shared controls in those files. There is no
additional framework, font download or CSS runtime. The library is embedded in
the CLI and distributed through normal source upgrades.

Verify the HTTP shell contract with `cargo test --locked --test issues_web`.
Browser verification should compare heading and main bounds across routes at
320, 390, 768 and 1440 pixels, in light and dark modes, and exercise navigation,
the picker, search and worker connection status.
