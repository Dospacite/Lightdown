# Lightdown interface

Lightdown is for people who live in an editor and want to get somewhere in
a few keys. The interface follows [T3 Code](https://github.com/pingdotgg/t3code):
dense, quiet and flat. Panels lie edge to edge, parted by hairlines rather
than gaps. Almost everything is grey; one blue marks what acts and what has
focus, and green, amber and red are kept for state.

Three rules decide most questions:

- **Everything is a keystroke away, and a click away.** Every action is in
  the command palette with its keys. Every control that shows only an icon
  names itself and its keys in a tooltip.
- **Controls stand where they act.** Layout toggles are on the title bar;
  what can be done with the file is at the right of its tabs; what a panel
  can do is in its head; a terminal's tools float over the terminal.
- **Colour is information.** Blue is the main action, the focus ring, an on
  switch, the caret and selection. Green, amber and red are done, pending or
  unsaved, and failed or destructive. Nothing else is coloured.

## Tokens

`src/theme.rs` owns every colour and metric. Use roles, never hex values.

| Role | Dark | Light | Use |
| --- | --- | --- | --- |
| `bg` | `#000000` | `#fcfcfc` | The canvas: editor, sidebars, bars |
| `raised` | `#111111` | `#ffffff` | Menus, dialogs, the composer, tooltips |
| `inset` | `#0d0d0d` | `#f4f4f5` | Code blocks, pages of a PDF |
| `tint` | `#1e1e1e` | `#ebebed` | Inline code, the user's messages |
| `fg` / `bright` | `#f5f5f5` / `#ffffff` | `#18181b` / `#09090b` | Text, headings |
| `dim` | `#a1a1a1` | `#5c5c66` | Labels, metadata, inactive tabs |
| `faint` / `icon` | `#8a8a8a` / `#8b8b8b` | `#6b6b75` / `#6e6e78` | Hints, line numbers, icons at rest |
| `line` | `#1c1c1c` | `#e4e4e7` | Hairlines between panels |
| `edge` / `field` | `#262626` / `#2c2c2c` | `#e4e4e7` / `#d4d4d8` | Edges of raised surfaces and of fields |
| `hover` / `active` | `#141414` / `#1f1f1f` | `#f2f2f3` / `#e8e8ea` | Row under the pointer, chosen row, pressed toggle |
| `primary` | `#346bf1` | `#1b4ed8` | Main action, focus ring, on switch |
| `selection` / `found` | `#1d3a75` / `#3a3214` | `#cddcfc` / `#fbe9a8` | Selected text, other search matches |
| `caret`, `link` | `#b4cbff`, `#51a2ff` | `#1b4ed8`, `#1447e6` | Caret; links and matched characters in lists |
| `success`, `warning`, `danger` | | | Done, pending or unsaved, failed |
| `keyword` … `special` | | | Syntax |

The palette test checks all text roles at 4.5:1 on every surface they lie
on, hints and icons at 3.8:1, the focus ring and caret at 3:1, and text on
a selection, a match and a message bubble at 4.5:1.

These two are the default themes. Other themes are files that set the same
roles (`src/themes.rs`, `assets/themes`), so code still reads colours only
through `theme::*()`. A new role is one line in `theme::ROLES`, naming the
existing role it falls back to, so themes written before it keep working;
`every_colour_has_a_role` fails until the line is there. The terminal's
sixteen ANSI colours are roles too (`theme::ansi`). Bundled themes are
checked more loosely, as `bundled_themes_keep_text_and_focus_readable`
documents: their syntax colours stay as their authors made them. Caches of
drawn text key on `theme::stamp()` and `font::generation()`, not on
`theme::light()`.

Metrics, in logical pixels, scaled by the display:

| Token | Value | |
| --- | --- | --- |
| `TITLE` | 40 | Title bar |
| `STATUS` | 24 | Status bar |
| `HEAD` | 36 | Heads of the editor, the explorer and the right panel, which line up |
| `ROW` | 24 | Explorer and outline rows |
| `MENU_ROW` | 30 | Rows of lists that drop from controls; the palette's are 32 |
| `ITEM` | 28 | Context menu rows |
| `CONTROL` / `SMALL_CONTROL` | 28 / 24 | Buttons and toggles |
| `ICON` / `SMALL_ICON` | 16 / 14 | Icons |
| `RADIUS_SM` … `RADIUS_2XL` | 4, 6, 8, 12, 16 | Key caps; rows and tabs; buttons, fields and context menus; lists that drop from controls and the approval card; dialogs, the palette and the composer |
| `UI_TEXT` / `SMALL` / `LARGE` | 13 / 12 / 14 | Interface text, metadata, titles and the agent's thread |
| `BODY` | 16 | Reading text, line height 1.6 |

## Type

- **Inter** (embedded, `font::UI`, `UI_MEDIUM`, `UI_BOLD`, `UI_ITALIC`) sets
  the interface and, by default, prose: markdown, plain text, the agent's
  thread. Medium marks controls and the open thing; semibold marks titles
  and headings.
- **Libron** (embedded) sets prose instead when *Reading font* is serif.
- **The system monospace face** sets code, the terminal and key paths in
  the agent's approval card, unless `buffer_font_family` names another;
  `ui_font_family` replaces Inter in the interface faces only (`font::UI`
  and its weights), not in sans-serif prose. Code's size and line height
  come from `theme::mono_scale()` and `theme::code_line_height()`.
- File icons in lists go through `icon_theme::draw`, never `icons::draw`
  with a fixed file icon, so the icon theme decides.

Labels are sentence case. Commands are named `Group: Action`, so the
palette reads as an index.

## Shapes

- **Panel:** the canvas, edge to edge, parted by a `line` hairline. A panel
  head is `HEAD` tall with a hairline under it.
- **Raised surface:** `raised` with a 1-pixel `edge`, a soft shadow and a
  radius by size (`ui::surface`). Menus, popovers, tooltips, the find bar.
- **Modal:** a raised surface with `RADIUS_2XL` over a scrim: the palette,
  dialogs, the settings, the file chooser. Lists that drop from a control
  are not modal and do not dim the window.
- **Row:** `hover` under the pointer, `active` when chosen, radius
  `RADIUS_MD`, inset from the panel's sides. A chosen row with the keyboard
  gets a 1-pixel `primary` ring.
- **Ghost button:** an icon or label on nothing, `hover` under the pointer,
  `active` when it is a toggle that is on (`ui::icon_button`).
- **Button:** primary (blue), outline, ghost or destructive (red), `CONTROL`
  tall (`ui::button`).
- **Field:** `bg` with a `field` edge; with the keys, a `primary` edge and a
  soft ring (`ui::input`).
- **Key caps:** each key on its own cap, `RADIUS_SM` (`ui::kbd`).
- **Switch, badge, tooltip:** `ui::switch`, `ui::badge`, `ui::tooltip`.

## Layout

```
 [◧] proj / src / main.py ●       [ ⌕ Search proj        Ctrl P ]       [▭][◨]  [⚙]
 ┌──────────────┬──────────────────────────────────────┬────────────────────────────┐
 │ proj   + + ⇕ ⟳│ (main.py ●) (notes.md)       ▶ ⋯     │ (Agent) Outline Preview ⟲ ✎ ×│
 ├──────────────┼──────────────────────────────────────┼────────────────────────────┤
 │ ▾ src        │                                      │ Thread title         12k/1M │
 │   main.py  M │ editor                               │        ┌─────────────────┐ │
 │ notes.md   A │                                      │        │ user's message  │ │
 │              ├──────────────────────────────────────┤        └─────────────────┘ │
 │              │ $ terminal          [+ ◫ ⟳ 🗑 | ×] │ 1 │ │ │ answer                    │
 │              │                                 │ 2 │ │ ╭───────────────────────╮ │
 │              │                                      │ │ Ask anything          │ │
 │              │                                      │ │ ⚇ model  ◔ effort @ (↑)│ │
 └──────────────┴──────────────────────────────────────┴────────────────────────────┘
  ⎇ master  Unsaved changes                         Ln 3, Col 5   ◉ Live   Markdown
```

- **Title bar:** the explorer's toggle, the project (opens a file or
  folder) and the file's place in it (its actions, Alt+F); the command
  centre in the middle (Ctrl+P); the terminal's and the right panel's
  toggles and the settings at the right. Where the compositor draws no
  frame, it is the window's title bar, with its buttons; a tiling
  compositor gets only Close.
- **Editor head:** a tab per file as a pill; the active one on `active`.
  An unsaved file has an amber dot that becomes × under the pointer; a tab
  not yet edited is italic. At the right, the file's actions: edit mode,
  PDF preview, run, run all, and the rest under ⋯.
- **Explorer:** the folder's name and new file, new folder, collapse and
  refresh in its head; rows with chevrons and indent guides; git's state
  tints the name and stands as a letter at the right.
- **Right panel:** Agent, Outline and Preview as tabs in its head, and what
  the view shown can do beside close.
- **Terminal:** no head. A toolbar floats at its top right; with more than
  two terminals, or two not split, a list at its right names them.
- **Status bar:** the branch, the agent at work when its panel is hidden,
  and what just happened at the left; where the cursor is (go to line),
  the markdown mode and the kind the file is shown as at the right, each a
  control.

Below 640 logical pixels the title bar keeps the toggles and a search
button, and an open sidebar takes the whole width.

## The palette

One box goes everywhere. Typing names a file of the project (fuzzy, the
file's name counting most); `>` lists every command with its keys; `:`
goes to a line, `12:5` to a column; `@` goes to a heading or definition.
Ctrl+P, Ctrl+Shift+P, Ctrl+G and Ctrl+R open it in each mode, and the same
keys switch mode while it is open. Matched characters are `link` blue.

## Motion

None runs on its own. Hover, focus and pressed states appear on the frame
the input arrives. Tooltips wait 550 ms for the pointer to rest.

## Verifying

`cargo test -- --test-threads=1` runs the palette test, the icon test and
the geometry tests. Build with `cargo build --features dump` to render a
frame without a Wayland session, and check both themes and a narrow window:

```sh
lightdown notes.md --dump /tmp/a.ppm 1400x900 'ctrl+shift+e,ctrl+l'
lightdown notes.md --dump /tmp/b.ppm 1400x900 'ctrl+shift+p,type:term'
lightdown notes.md --dump /tmp/c.ppm 520x760 'ctrl+comma'
```

Lightdown draws on a software canvas and does not publish accessibility
semantics to the desktop; screen-reader support needs a separate
implementation.
