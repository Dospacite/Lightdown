# Lightdown

A lightweight editor for markdown, LaTeX, Python and Jupyter notebooks on Linux,
written in Rust, with an AI agent in a sidebar. It shows its first frame
about 10 ms after launch, ships as one 15 MB binary, renders markdown in
place while you edit it, and draws PDFs itself.

Lightdown is a native Wayland application, tested on Hyprland (Arch Linux)
and Sway (Ubuntu 24.04). On desktops that draw no title bars for
applications, such as GNOME, its title bar is the window's, with its
buttons.

## Interface

The interface follows [T3 Code](https://github.com/pingdotgg/t3code): flat
panels edge to edge, parted by hairlines, true black in the dark theme and
paper white in the light one. One blue marks the main action of a panel,
the focus ring, an on switch, the caret and selected text; green, amber
and red mark what is done, pending or unsaved, and failed. In the explorer,
git's new, changed and conflicted files are tinted green, amber and red,
each with a letter beside it.

- **Title bar.** The explorer's toggle sits at the left, then the project
  and where the file is in it (click it, or press Alt+F, to see what can
  be done with the file). The command centre is in the middle, and the
  terminal's toggle, the right panel's toggle and the settings are at the
  right.
- **Editor.** Each file has a tab along the head, with the file's actions
  at the right: edit mode, PDF preview, run, and the rest under ⋯.
- **Right panel.** Agent, Search, Problems, Tests, Hierarchy, Outline,
  Backlinks, Debug and Preview are tabs in its head. Tabs that do not fit
  are listed under ⋯ at its right, and the open tab always stays in view.
  **View: Choose right panel tab** lists them all.
- **Status bar.** The branch and what just happened are at the left. At
  the right are the file's indentation, line endings and encoding, the
  cursor position, the markdown mode and the kind the file is shown as.
  Each of these is a control. The format items show for text files only,
  not for PDFs, pictures or notebooks, and drop out one by one, encoding
  first, when the window is too narrow for them.

Every control that shows only an icon names itself and its keys when the
pointer rests on it. Drag the hairline between two panels to resize them.

Documents and the interface are set in Inter, code in the system's
monospace face. *Reading font* in the settings sets prose in Libron, a
serif, instead.

### Themes, icons and fonts

The default themes are T3 Code's: **Lightdown Dark** and **Lightdown
Light**. Bundled beside them are One Dark and One Light, Solarized Dark and
Light, Gruvbox Dark and Light, Nord, Catppuccin Mocha and Latte, and Tokyo
Night and Tokyo Night Light.

| Command | What it does |
|---|---|
| **Theme: Select theme** | Lists every theme; moving through the list shows each, Enter keeps it, Esc goes back |
| **Preferences: Switch between dark and light** (the sun or moon) | Switches between `dark_theme` and `light_theme` |
| **Theme: Follow the desktop's dark or light preference** | Sets `theme_mode` to `system` |
| **Theme: Import Zed or VS Code theme…** | Converts a theme JSON into `~/.config/lightdown/themes` |
| **Theme: Select icon theme** | File icons in the explorer, tabs and file finder |
| **Font: Select buffer font** | Lists the installed monospace families for code |
| **Font: Select interface font** | Lists every installed family for the interface |

A theme is a JSON file in `~/.config/lightdown/themes` or a project's
`.lightdown/themes`. It names Lightdown's colour roles, and one file can
hold a family under `themes`:

```json
{"name": "Paper", "appearance": "light",
 "colors": {"bg": "#fbfaf6", "fg": "#222222", "primary": "#2255cc", "keyword": "#7a3e9d", "ansi_red": "#c4342d"}}
```

The roles are the surfaces `bg`, `raised`, `inset`, `tint`, `hover`,
`active`; text `fg`, `bright`, `dim`, `faint`, `icon`, `quote`; lines
`line`, `edge`, `field`; `primary`, `primary_hover`, `on_primary`,
`selection`, `found`, `caret`, `link`; states `success`, `warning`,
`danger`, `destructive`, `info`; syntax `keyword`, `string`, `number`,
`function`, `special`; and the terminal's `term_blue` and sixteen
`ansi_black` … `ansi_bright_white`. Colours are `#rgb`, `#rrggbb` or
`#rrggbbaa`, the last laid over `bg`. A role left out falls back to the
nearest one (`raised` is mixed from `bg` and `fg`, `caret` and `link`
follow `primary`, ANSI red follows `danger`, and so on), so a theme can be
as short as `bg`, `fg` and `primary`; unknown roles are ignored, which
keeps files from newer versions loading. A theme file that is written is
read again within two seconds; a broken one is named in the status bar and
skipped. A theme of the same name as a bundled one replaces it.

Importing reads a Zed theme family (`themes` with `style`: surfaces,
`text.*`, `border.*`, `element.*`, the first player's cursor and
selection, `syntax` and `terminal.ansi.*`) or a VS Code colour theme
(workbench `colors`, with comments allowed, and `tokenColors` by
TextMate scope). Syntax colours without a Lightdown role yet, such as
`comment`, `property` or `parameter`, are kept in the file.

An icon theme is a folder in `~/.config/lightdown/icon-themes` holding
`icon-theme.json` and its SVGs:

```json
{"name": "Mine", "file": "file.svg", "folder": "folder.svg", "folder_open": "folder-open.svg",
 "file_names": {"Makefile": "make.svg"}, "suffixes": {"rs": "rust.svg", "tar.gz": "archive.svg"},
 "tint": false}
```

Exact names win over suffixes, and the longest suffix over shorter ones.
SVGs keep their colours, or with `"tint": true` are drawn in the icon
colour. The built-in **Lightdown** theme draws line icons in the file
finder only, as before; **Lightdown Lines** draws them in the explorer
and tabs too.

Code can be set in any installed family (`buffer_font_family`), at its own
size (`buffer_font_size`) and line height (`buffer_line_height`), with
OpenType features such as `{"calt": true, "liga": false, "ss01": true}`
(`buffer_font_features`), so programming ligatures can be turned on or
off. The cursor can stand inside a ligature, and selections and columns
still count characters. `ui_font_family` sets the interface in another
family. Fonts are found through fontconfig (`fc-match`, `fc-list`).

Source files and labelled Markdown code fences share highlighting for Python,
LaTeX, Rust, JavaScript, TypeScript, C-family languages, Java, Go, JSON, shell,
TOML, YAML, SQL and CSS. Optional tree-sitter grammars, installed from the
palette, highlight from a parse tree instead: Bash, C, C++, CSS, Go,
Haskell, HTML, Java, JavaScript, JSON, Kotlin, Lua, Markdown, PHP, Python, Ruby,
Rust, SQL, Swift, TOML, TSX, TypeScript, YAML and Zig, in source files and
labelled code fences. Until a file's first parse is ready, the lexical
highlighter draws; while later parses run, the last tree's colours move with
the edit. Injection queries highlight embedded languages with their installed
grammars, including HTML script and style, tagged templates and Markdown
fences. Locals queries resolve parameters and local references within scopes.
Parameters and properties have their own colours. Unknown languages stay plain.

Menus, the file chooser and settings fit the window and scroll. Below 640
logical pixels, the title bar keeps its toggles and a search button, and an
open sidebar uses the whole width.

The palette, metrics and interaction rules are in [DESIGN.md](DESIGN.md).
Palette tests check text contrast in both themes.

## Command palette

One box goes anywhere:

| Keys | Typed | Lists |
|:-----|:------|:------|
| Ctrl+P | a name | the project's files, fuzzily: `mpy` finds `src/main.py` |
| Ctrl+Shift+P | `>` | every command, with its keys |
| Ctrl+G | `:` | a line to go to; `12:5` goes to line 12, column 5 |
| Ctrl+R | `@` | the document's headings, or its classes and functions |
| Ctrl+Alt+R | `#` | symbols across the project, including unsaved open tabs |

Clicking the search box in the title bar opens it too. Up and Down choose,
Enter goes, Esc closes; the same keys switch what it lists while it is
open. Files open in the project are listed first; the rest are those git
knows of, or else everything outside hidden and build folders.

**Go to: Project TODO comments** scans `TODO`, `FIXME` and `XXX` comment
tokens in the background and opens a filterable palette list. Strings and
identifier fragments such as `TODOS` are ignored. Enter opens the file with
the marker selected. Source languages use the editor's lexical comments;
Markdown and HTML include `<!-- comments -->`. Unsaved open text tabs
replace disk contents. The scan follows project-search file exclusions,
skips binary and files over 2 MB, and stops at 64 MB or 1000 markers. Run
the command again to refresh.

## Split editing and source tools

Open two files, then run **View: Toggle two files side by side** from the
palette. Click either view or use **View: Focus other editor** to edit it.
Every view keeps its tab's cursor, scrolling, unsaved text and undo history.
Views nest in rows and columns, and any view can be split to any side.
**View: Add editor view to the right** and **View: Add editor view below**
put another open tab beside the focused view. **View: Split right** (and
left, up, down) shows the current file again on that side, or another open
tab when the file already has a second view or cannot have one. **View:
Focus other editor** goes through the views in order. Opening another file
replaces the focused view; closing a tab removes its views.

| Command | Keys |
| --- | --- |
| View: Split right | Ctrl+\\ |
| View: Split left | Ctrl+Alt+\\ |
| View: Split down | Ctrl+Alt+- |
| View: Split up | Ctrl+Alt+= |
| View: Close view | Ctrl+Alt+Shift+W |
| View: Zoom focused view | Ctrl+Alt+Shift+Z |
| View: Focus view to the left, right, above, below | Ctrl+Alt+Shift+Left, Right, Up, Down |
| View: Move view left, right | Ctrl+Alt+Left, Ctrl+Alt+Right |
| View: Move view up, down | Ctrl+Alt+PgUp, Ctrl+Alt+PgDn |

**View: Close view** closes the focused view and keeps its tab open. **Move
view** trades places with the view on that side. Drag a tab onto a view:
near an edge, a blue mark shows the half the new view takes, and dropping
splits there; in the middle, the two views trade places. Dragging a tab to
an edge of its own view shows the file there a second time.

Hairlines part the views, and the focused one has a blue line along its
top. Drag a hairline to resize the views on either side, as with the side
panels; double-click it to give them equal room. Sizes are kept as shares,
so they follow window resizes. **View: Zoom focused view** shows the focused
view alone across the editor, with a *Zoomed* badge at its top right; the
side panels and terminal stay. The command again, Esc, or a click on the
badge shows all views. Focus moves while zoomed keep the zoom. Folder
sessions restore the views, their places, sizes and second views when the
files still exist.

All views share one tab strip, and the focused view shows the active tab;
tabs shown in other views have bright names. Lightdown keeps one strip
rather than one per view. The editor head lines up with the explorer's and
the right panel's heads and holds the active file's actions, which a strip
in each view would repeat, and every tab already keeps its own place.

**View: Show this file in a second view, or close it** shows the current
file twice, beside the focused view; with one tab open, the add-view
commands do the same. Both views edit the same text with one undo history,
and each keeps its own cursor, selection and scrolling. An edit in one
moves the other's place with the text. Click a view, or use **View: Focus
other editor**, to give it the keyboard; the wheel scrolls the view under
the pointer. Running the command again closes the second view and keeps the
one with the keyboard. PDFs, pictures and notebooks have one view, and a
file has at most two.

The editor highlights whole-word occurrences of the word under the cursor.
It matches Unicode letters and underscores, with case preserved. Searches
keep their own marks. Source files also have indent guides and a column 80
ruler. **View: Toggle indent guides** and **View: Cycle column ruler 80, 100
or off** save these preferences. Guides follow the file's EditorConfig indent
unit, with four columns for a tab.

Alt+Z, **View: Toggle word wrap**, turns wrapping off or on for the kind
of file in view: code (with Python and LaTeX source), markdown, or plain
text, each remembered in `settings.json` as
`"word_wrap": {"code": true, "markdown": true, "text": true}`. All three
wrap by default. Without wrapping each line stays on one row: Shift+wheel and a
touchpad's sideways scrolling move the text left and right, the caret is
kept in view as you type and move, and dragging a selection past the
column's edge slides the text that way. A horizontal scrollbar along the
bottom shows when a line is wider than the column; drag it, or click it to
jump there. The line numbers, gutter marks and sticky headers stay put.
A second view scrolls sideways on its own. Notebook cells always wrap. In
markdown, a long line in a code fence runs past the fence's panel.

**File: Copy source line reference** copies a path and the selected source
lines, such as `src/app.rs:120-134`. With no selection it uses the cursor line.
**File: Copy GitHub link to source lines** copies a permalink at HEAD using
the repository's GitHub `origin`. It requires the buffer and saved file to
match that commit so the line numbers point at the same text. Notebook and
PDF views have no source line reference.

**File: Recent projects and files** lists the last 120 existing paths across
windows. An empty editor shows Open and New file actions, with the latest 12 recent
paths below them when history exists. Typing starts a new document. Picking
a file opens a tab. Picking a project follows the usual folder-opening rules.

The **Text** palette commands sort lines, remove duplicate lines while
preserving their first occurrence, uppercase or lowercase text, convert
to snake_case, camelCase, PascalCase, kebab-case or Title Case, and join
lines. Sorting and duplicate removal use selected complete lines, or the
whole document if nothing is selected. Case changes use the selection or
cursor word. Conversion splits separators and changes of case, keeping
acronym boundaries such as `XMLHttp` as `xml_http`. Joining uses selected lines, or the current and next line.
Each change is one undo step. Notebook transformations stay within one cell.

Ctrl+Alt+K, **Bookmarks: Toggle bookmark**, marks the cursor's line with a
blue bar left of its number. Ctrl+Alt+] and Ctrl+Alt+[, **Bookmarks: Next
in file** and **Bookmarks: Previous in file**, go to the next or previous
mark, wrapping round. **Bookmarks: Next across project** and **Bookmarks:
Previous across project** carry on into the project's other files, in path
order. **Bookmarks: List all** lists every mark with its file, line and the
line's text, and **Bookmarks: Clear in file** removes the file's marks.
Marks follow their lines through edits: lines inserted above move them
down, a deleted line drops its mark, and a file reformatted or reindented
as a whole keeps them by line. They are kept per project in the private
data folder (`~/.local/share/lightdown/bookmarks`) and come back when the
file opens again; a file reloaded after an outside change keeps them by
line number where the line still exists. A document that was never saved
keeps its marks only while it is open.

Ctrl+Alt+Z, **View: Toggle focus mode**, hides the title, tabs, status bar and
panels while you write. Other paragraphs use the theme's readable dim text;
the paragraph around the caret keeps its usual styling. Blank source lines
separate paragraphs. Ctrl+Alt+Z or Esc brings the interface back with its panels
as they were. Commands and save keys still work while it is hidden.

Ctrl+Alt+W, **View: Toggle typewriter mode**, keeps the caret at 45% of the
editor's height as you type or move, including at the start and end of the
file. Scrolling still works; the next edit or caret movement returns to that
height. The modes are independent and last for this window.

## Python debugging

Save project files, open a `.py` script, and use **Debug: Toggle breakpoint
on current line** where execution should stop. **Debug: Start Python script**
starts with the project's selected interpreter and literal `.env` values.
It pauses at the first executable line. Use **Debug: Continue**, **Step over**,
**Step into**, **Step out**, and **Stop** from the palette.

The Debug tab in the right panel shows the call stack, local variable names,
types and values, breakpoints, and captured output. **Debug: Inspect stack
frame** selects a caller's variables and source location. Source jumps stay within the project. The source gutter
marks breakpoints, and the selected paused frame's line is highlighted.
Breakpoints can change while stopped or paused. Stop ends the script and its
child processes, including a script blocked in input or native code.

The bridge uses Python's standard-library [bdb debugger framework](https://docs.python.org/3/library/bdb.html). No debug
adapter package is needed. Ctrl+F5 starts a saved script, F9 toggles a
breakpoint, F10 steps over, F11 steps into and Shift+F11 steps out.
**Debug: Set breakpoint condition** takes a Python expression at the current
line. **Debug: Set logpoint expression** evaluates and prints an expression
there while continuing; an empty expression makes it a normal breakpoint.
A condition also applies to a logpoint. **Debug: Toggle watch expression**
adds an expression to the selected paused frame, or removes an existing one.
**Debug: Evaluate in paused frame** evaluates once and puts its result in
Output. These expressions can call functions and change the program.

**Debug: Set script arguments** takes a JSON string array, such as
`["--verbose", "file name"]`. **Debug: Set script standard input** takes a
JSON string, such as `"answer\n"`, supplied at the next start. Input then
reaches EOF. **Debug: Run one pytest test** takes a pytest node ID and starts
it with output capture disabled; pytest must be installed in the selected
interpreter. Breakpoints, conditions, logpoints, watches and launch options
are saved per project in the private XDG data folder. They load when a Debug
command is used. Restart after source edits to align execution with the new
text. Variable inspection shows up to 200 locals and 500 characters per
value; output retains the latest 60 KB. There are at most 100 watches.

Variables are a tree. A variable with a triangle opens and closes with a
click, or with Right and Left; Up, Down, Home and End move between rows,
Enter opens or closes the selected one and Esc gives the keys back to the
document. **Debug: Browse variables** puts the keyboard in the tree without
the mouse. Children are fetched from the paused process when a row opens,
200 at a time: **Show 200 more** under a long container loads the next page,
up to 20,000 children per variable and eight levels deep. Closing a row
forgets what was loaded beneath it, and selecting another frame closes
them all. For Python the children of lists, tuples, dicts and sets are their
items, and the children of other objects are the attributes stored in their
`__dict__` and slots. Properties, `__getattr__`, `__repr__` and other code of
the program never run while inspecting. Values of the program's own classes
show as `<Name object>`, and built-in containers as their first eight items.

**Debug: Toggle break on raised exceptions** stops where an exception is
raised in a project file, outside `site-packages` and virtual environments.
An exception raised inside a library stops in the first project frame it
reaches, and each exception stops once, not again in each caller.
`StopIteration` and `GeneratorExit` do not stop. Raised exceptions keep
tracing on while the script runs, which makes it slower. **Debug: Toggle
break on uncaught exceptions** stops after an exception ends the script,
with the traceback's frames and their variables to inspect; `SystemExit`
does not stop, and pytest's own handling means its failures are not
uncaught. **Debug: Select exception breakpoints** lists both switches. They
are saved per project with the other debug options and can change while
paused. When stopped on an exception, its type and message head the Debug
tab and are written at the end of the stopped line. The message is the
standard library's; an exception class with its own `__str__` shows its
arguments instead, so that no code of the program runs.

While paused, each line from the paused function's first line to the
stopped line shows, dimmed at its end, the locals named on it with their
short values: up to four per line, 60 characters each, over at most 500
lines. They are the values the Debug tab already has, so nothing is
evaluated, and they are the values at the stop, not when the line ran.
Names inside strings, comments and after a dot are not matched, and
functions, classes and modules are left out. A file edited since the stop
shows none. **Debug: Toggle inline values** turns them off and on, saved per
project. Native frames do not say where their function starts, so the
nearest function header above the stop is used.

Resting the pointer on a name or a dotted chain such as `self.items` in the
paused frame's file shows its value in the hover box, in the selected frame.
The Python bridge looks the name up in the frame's locals, globals and
builtins and follows the chain through stored attributes only; a property
is reported and not run, and a chain off a call or an index, such as
`f().x` or `a[0].x`, is not looked up. A native adapter is asked with
`evaluate` in its `hover` context, again only for plain names and dotted
chains. Nothing else is evaluated on hover.

**Debug: Start native adapter** reads `.lightdown/debug.json` and uses the
same Debug tab, F9 breakpoints, stepping keys, frame picker and evaluation
command for native source. Build the executable with debug symbols first.
For an installed `lldb-dap`:

```json
{"adapter": ["lldb-dap"], "transport": "stdio",
 "launch": {"program": "/absolute/project/target/debug/app",
            "cwd": "/absolute/project", "args": [], "stopOnEntry": true}}
```

For CodeLLDB use `"adapter": ["codelldb", "--port", "{port}"]` and
`"transport": "tcp"`. For Go use `["dlv", "dap", "--listen=127.0.0.1:{port}"]`
with TCP and Delve launch arguments such as `{"mode":"debug","program":"."}`.
GDB 14 and newer include an adapter: use `["gdb", "-i", "dap"]` over stdio,
with `"stopAtBeginningOfMainSubprogram": true` in `launch` to stop in `main`.
Lightdown substitutes a loopback port, starts the adapter, then connects.
The `launch` object supplies arguments for the DAP request. Set top-level
`"request": "attach"` to attach instead of launch, with adapter-specific
arguments in `launch`, for example `{"pid": 1234}` for an adapter accepting
`pid`.

`"adapter"` can be left out when `"type"` names a known kind. Lightdown then
looks on PATH when the session starts: `lldb`, `rust`, `c` and `cpp` try
`lldb-dap` (or the older `lldb-vscode`), then `codelldb`, then GDB 14 or
newer; `lldb-dap`, `codelldb` and `gdb` ask for that adapter alone; `go`
uses `dlv`; `node` uses js-debug's `js-debug-adapter` or `dapDebugServer`
(or `dapDebugServer.js` run with `node`); `python` uses `debugpy.adapter`
in the project's interpreter when `import debugpy` works there. The error
names what to install when nothing is found.

**Debug: Attach to process** lists your own processes from `/proc`, newest
first, with pid, user and command line; type to filter. Choosing one sends
a DAP `attach` request: Python processes go to debugpy with `processId`,
Node processes to js-debug with `processId`,
Go programs (found by their `.go.buildinfo` section) to Delve with
`{"mode":"local","processId":pid}`, and other programs to lldb-dap,
CodeLLDB or GDB with `pid`. **Debug: Start Python script with debugpy**
runs the current file as `python -m debugpy --listen 127.0.0.1:PORT
--wait-for-client`, with the script arguments, and attaches over that port;
Stop ends the script. Both use the same Debug tab, breakpoints and stepping.
Install debugpy in the project's interpreter first (`python -m pip install
debugpy`).

Native attaching uses the kernel's ptrace facility, which Linux limits with
`kernel.yama.ptrace_scope` (see `/proc/sys/kernel/yama/ptrace_scope`). At 0
you may attach to any of your own processes; at 1, the default on most
distributions, only to your own child processes, so attaching to a program
started elsewhere is refused; 2 needs administrator rights and 3 forbids it.
When an adapter refuses to attach, the Debug tab says which setting is in
force. Start the program under the debugger instead, or allow attaching
until the next reboot with `sudo sysctl kernel.yama.ptrace_scope=0`.
debugpy's attach to a pid also needs ptrace, through GDB. Node PID attach
needs no ptrace: Lightdown sends the process `SIGUSR1`, which makes Node open
its inspector (on 127.0.0.1:9229 unless the program chose another port),
waits up to three seconds for it, and attaches js-debug to that port. A
process started with `--inspect` keeps its own inspector, which is found by
asking each port it listens on. A js-debug attach configuration can also name
an inspector port directly.

A file may instead contain `{"default":"app","configurations":[...]}`.
Each entry has a `name` and the same fields as the single-object form.
**Debug: Select named configuration** remembers a name per project; an empty
name uses `default`, or the first entry. **Debug: Start native adapter**
starts that entry. Strings anywhere in the selected configuration expand
`${workspaceFolder}`, `${file}`, `${fileDirname}`, `${relativeFile}` and
`${env:NAME}` literally, without a shell. Unknown variables, missing environment
values and unavailable file paths are errors. There are at most 100 entries
and the configuration file is limited to 1 MB.

`"preLaunchTask"` names an F6 task, such as `"cargo build"` or `"make
app"`, that runs first in the task runner. The adapter starts only if the
task succeeds; if it fails, the task's errors open (or its output, when it
reported no locations) and the launch stops. In a Cargo project,
`"cargo": {"args": ["build", "--bin", "app"]}` builds with
`--message-format=json` and uses the executable Cargo produced as
`launch.program`. Build scripts are ignored; a build producing several
executables asks for `--bin`, and compiler errors appear in the Debug tab.
`test --no-run` works too. A task runs before the build when both are set.
**Debug: Stop** cancels a task or build that is still running.

**Debug: Create configuration from template** adds an entry for Rust, C or
C++ with lldb-dap or GDB, Go with Delve, Node with js-debug, or Python with
debugpy (launching the current file, or attaching to debugpy listening on
port 5678). In a Cargo project the native templates build the package's
binary with `"cargo"`; elsewhere they point `program` at `a.out`. The entry
is added to `.lightdown/debug.json`, creating it if needed. Existing entries
are kept. A single-object file becomes the first entry, named `default`,
and a repeated name gets a number. The file then opens for editing.

The adapter must launch with its internal console; `runInTerminal` and
reverse requests other than `startDebugging` are refused. For Node, js-debug
opens child sessions over the same loopback DAP server. Each child receives
breakpoints and exception filters; threads, frames, variables, evaluation
and stepping follow the owning session. Stop closes the children with the
parent. Up to 64 simultaneous sessions are supported.

For example, `{"type":"node","launch":{"type":"pwa-node",
"program":"${file}","cwd":"${workspaceFolder}","console":"internalConsole"}}`
launches the current JavaScript file. TypeScript uses the emitted JavaScript
as `program`, with `"sourceMaps":true` and `"outFiles":["${workspaceFolder}/out/**/*.js"]`
in `launch`. Compile with `tsc --sourceMap` and set breakpoints in the `.ts`
file. Lightdown passes the project folder to js-debug as `__workspaceFolder`,
as VS Code does; js-debug needs it to place such breakpoints before the code
runs. For PID attach, use top-level `"request":"attach"` and
`"launch":{"type":"pwa-node","processId":"1234"}`, or pick the process
with **Debug: Attach to process**. `"processId":"1234:9230"` attaches to an
inspector already open on port 9230 without signalling the process.

The client follows the [DAP initialization sequence](https://microsoft.github.io/debug-adapter-protocol/overview.html),
with a twenty-second initialization limit and bounded frames. Stop kills the
adapter process group. It shows up to 100 stack frames and the first
inexpensive scope's variables. Breakpoint conditions and logpoints go to
the adapter as DAP conditions and log messages, so they use the adapter's
expression syntax, and watch expressions are evaluated in the selected
frame at each stop; one that fails shows its error.
Variables open in the same tree, with their children fetched through the
adapter's `variables` request, up to 100 open references per frame.
**Debug: Select thread by ID** switches to a thread listed in the paused
Debug tab and fetches its stack. Python's bdb bridge does not select threads.

In a native session, **Debug: Select exception breakpoints** lists the
exception filters the adapter advertises, with a check on those switched on;
picking one switches it and sends the new set with `setExceptionBreakpoints`
at once. Top-level `exceptionFilters` is the array of filter IDs to start
with, for example `["raised", "uncaught"]`; without it the adapter's default
filters are on. Changes during a session are not saved. Top-level
`breakOnPanic: true` sets function breakpoints on `rust_panic` and
`rust_begin_unwind`; this needs an adapter and binary that support those
symbols. When the adapter stops on an exception, its type and message come
from `exceptionInfo` where the adapter supports it, otherwise from the stop's
text, and show at the top of the Debug tab and on the stopped line. The stop
description appears above the stack. Adapter-specific UI remains unsupported.
No adapter ships in the binary.

## Agent screenshots and MCP tools

Focus the agent's message box and paste a PNG screenshot with Ctrl+V.
The composer shows an attachment count. Send it with an instruction or on its
own. **Agent: Remove screenshots from draft** clears attachments. Each message
accepts up to four PNGs, each at most 5 MB and 32 million pixels. Screenshots
persist in the conversation and go to the provider with the message.

Catalogue models report whether they accept images. For configured providers,
turn on **Provider accepts screenshots** in Settings only when the selected
model supports them. A model without image support leaves the draft intact
and reports the mismatch. Image messages work with Chat, Messages and
Responses request formats.

Configure stdio or HTTP MCP servers in `.lightdown/mcp.json`, for example:

```json
{
  "mcpServers": {
    "project-tools": {
      "command": "/absolute/path/to/mcp-server",
      "args": ["--stdio"],
      "env": {"SERVICE_MODE": "development"}
    }
  }
}
```

Run **Agent: Connect project MCP servers** to start the configured programs,
initialize them and discover their tools. Connections start explicitly by default. A server with `"autoConnect": true`
connects on project activation when **Agent may run commands** is already
Allow. Ask and Deny never auto-start programs or contact endpoints. Programs run in the project folder, inherit
its literal `.env`, and receive any explicit `env` overrides. Commands are
argv lists with no shell expansion. Set `disabled` to `true` to skip a server.

Discovered tools become available to all three provider request formats.
Every external tool uses the project's **Agent may run commands** policy,
including tools labelled read-only by their server. Calls return bounded
JSON results to the agent, including MCP errors and structured content.
Source checkpoints capture project file changes; external database or service
changes are outside those checkpoints.

**Agent: Disconnect MCP servers** stops the processes. Stopping an active
MCP call disconnects its servers too. Discovery times out after 20 seconds;
calls after 60 seconds. The client supports up to 16 servers and 256 tools,
using the [stdio initialize lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) through protocol version 2025-11-25.
For streamable HTTP, use `"url": "https://host.example/mcp"` instead of
`command` and `args`. Optional `headers` is an object of header names and
values, including authentication. Headers go to curl through stdin rather
than process arguments. HTTP supports JSON and SSE POST responses and session
IDs. The client refreshes tools on `notifications/tools/list_changed`;
**Agent: Refresh MCP tool lists** refreshes them explicitly while idle.
Reconnect after changing configuration. Persistent GET notification streams,
session resumption, newer stateless protocol versions and server requests
for sampling or user input are not supported. HTTP redirects are refused;
configure the final endpoint URL. Never commit authentication headers.

## Terminal history search

Run **Terminal: Find in history**, or Ctrl+Shift+F while the terminal has
focus. Enter literal, case-sensitive text. F3 and Shift+F3 select the next
or previous match and wrap through the results. The matching text becomes
the terminal selection and scrolls into view. Search includes the current
screen and all retained history, joining rows that wrapped at the terminal
edge. The status bar shows the selected match and total. The palette also
has next and previous match commands.

## Git

The **Source Control** view in the left sidebar lists the repository's
changes; see [Folder](#folder). Its head has **Explorer** and **Source
Control** tabs, and Ctrl+Shift+G (**Git: Go to source control and back**)
opens it. Changed files stand in sections with counts: merge conflicts,
staged changes, changes and untracked files. A file staged and then changed
again is in both. Click a file for its diff: staged files against HEAD,
changes against the index. A conflicted file opens itself. The buttons on
the file under the pointer stage (+), unstage (−), discard (↺) or delete
an untracked file (bin), or open the file; each section's head does the same
for all its files. Discarding asks first, lists the files, and is refused if
a file or the index changed since the question. **Git: Discard all unstaged
changes** does it for every tracked file, leaving untracked files alone.
Type a message in the field at the top and press Enter or **Commit**, or
Ctrl+Enter anywhere in the view, to commit what is staged. A failed commit
puts the message back. **Amend** opens the amend dialog with the typed
message, or HEAD's subject when the field is empty. The robot button drafts
a message from the staged diff with the suggestion model, into the field for
review. The view refreshes with the editor's git polling, and its head's
refresh button asks at once.

Open **Git: Source control, stage and commit** from Ctrl+Shift+P. The list
shows each changed file as staged, unstaged or both. Choose a file to view
its diff, stage it or unstage it. The top rows stage all saved changes,
unstage all, commit staged changes, or draft a message with the suggestion
model. The draft opens for review before committing. If staging changes
while the message is open, the commit is refused until you reopen it.
New repositories with no commits are supported.

**Git: Diff file against HEAD** and **Git: Diff file against staged** open
read-only unified diffs. They include unsaved text from open text tabs.
**Diff: Toggle side-by-side and unified view** shows any open text diff in
two columns, including git history and agent review diffs. Removed and added
words have stronger red and green backgrounds; matching words stay quiet.
The columns scroll together with the wheel, arrows, Page Up/Down and
Home/End. Left/Right pans long lines. Toggle back for text selection and
search in the unified patch. This view shows the patch's context, with hunk
headers between omitted ranges.
**Diff: Compare file with…** opens the file chooser and compares the current
buffer with a UTF-8 file. **Diff: Compare with open tab** lists other open
text tabs and includes their unsaved edits. **Diff: Compare selection with
clipboard** compares selected text with clipboard text. All three open
read-only patches titled with both sides, such as `a.rs ↔ b.rs`, with the
same unified and side-by-side toggle. Identical texts give a message
instead. They work outside a Git repository and leave source buffers
unchanged.

Text comparisons use the editor's line endings, so CRLF files keep their
line identity. The source-control menu's **Show staged diff** compares the
index with HEAD.
Green, amber and red marks beside line numbers show added, changed and
deleted lines against HEAD. **Git: Next change** (Alt+F8) and
**Git: Previous change** (Alt+Shift+F8) move between marked lines and wrap
at the ends of the file. They refresh in the background, including while
you edit. Ignored files and folders stay accessible in the explorer with
the same dim text as hidden files.

Click a gutter change mark to preview its saved hunk beside the source.
Use **Stage**, **Revert**, or **Close** there; Esc closes the preview.
Shift+click previews a staged hunk with **Unstage**. The **Git: Show saved
hunk at cursor** and **Git: Show staged hunk at cursor** commands do the
same with the keyboard; palette commands stage, unstage or revert the
previewed hunk. Save edits first. Operations refuse a changed file or index.
New files must be added before they have staged hunks. The preview shows up
to twelve lines; open the file diff for longer hunks.

**Git: Stage selected lines** and **Git: Unstage selected lines** apply only
selected changes within one saved hunk. Select whole source lines, then run
the command, or use **Stage lines** or **Unstage lines** in its preview.
Removed lines in a replacement correspond to added lines in order; extra
removed lines belong to the last added line. Save edits first. The operation
refuses changes to the file or index since it read the hunk.

**Git: Toggle blame for file** opens a column at the left of the gutter.
Each run of lines from one commit is marked by a hairline and labelled once
with the commit's short hash, author and date (UTC). It blames the buffer as
it stands, so edited lines read **Uncommitted** in amber. Click a label for
the commit's details and patch. It updates a moment after edits stop, and
after commits; it also works with line numbers hidden. A file outside a
repository turns it off; an untracked file shows no column and a message.
Inline blame, for the cursor's line only, remains a separate command.

Click the branch in the status bar, or run **Git: Switch, create or delete
branches**, for local branches and their upstream ahead/behind counts.
Create a branch from HEAD, switch branches, or confirm deletion of a merged
branch. Git refuses deletion of unmerged branches. Save open edits before
switching branches, pulling, stashing or staging the affected files.

The palette also has **Git: Fetch all remotes**, **Git: Pull with
fast-forward only**, **Git: Push branch**, **Git: Stash saved changes** and
**Git: Restore latest stash**. They use your existing Git remotes and
credentials and run in the background. Commands time out after two minutes;
errors appear in the status bar. Stashing includes untracked files. A pull
that needs a merge is refused by the fast-forward-only command.
**Git: Pull with rebase** confirms before replaying local commits;
**Git: Pull with merge** allows a merge commit.
**Git: Merge branch…** lists local and remote-tracking branches, says
whether each merge would fast-forward or make a merge commit, and
fast-forwards when possible. **Git: Merge branch with no fast-forward…**
always makes a merge commit. **Git: Rebase onto…** says how many commits it
would replay and confirms before rewriting them. Each asks before running.
Save open buffers first. Branch integration requires committed or stashed
working changes and refuses a HEAD, index or status that changed since the
list was made.

**Git: Interactive rebase…** first asks where to start: the commits not yet
on the upstream branch, or the commits after one of the latest 50. It then
lists those commits oldest first. Alt+Up and Alt+Down move the selected
commit, and p, r, s, f and d mark it pick, reword, squash, fixup or drop.
Reword asks for the new message at once. Enter checks the list (the first
kept commit cannot be squashed, and at least one must be kept), then asks
before rewriting history. Lightdown runs `git rebase -i` with
`GIT_SEQUENCE_EDITOR` copying the list in and `GIT_EDITOR` set to a small
script in the Git folder that writes reworded messages and keeps the
joined message of a squash. Ranges with merge commits are refused. The
working tree must be clean, and the list is refused if HEAD moved.

A merge, rebase, pull, cherry-pick, revert or Continue that stops on
conflicts lists the conflicted files; choose one to open it in the merge
editor. While a merge, rebase, cherry-pick or revert is in progress, the
status bar shows it in amber after the branch. Click it for **Continue**,
**Skip**, **Abort** and the conflicted files. **Git: Continue**, **Git:
Skip** and **Git: Abort** do the same from the palette. Skip and Abort ask
first, and a merge cannot be skipped. Continue keeps Git's prepared commit
message, and during an interactive rebase still applies later rewords.

**Git: List stashes** lists every stash with its message. Choose one to show
its tracked-file diff, apply it, pop it, or drop it. Mutations ask for
confirmation and refuse if the stash list changed. A failed apply keeps the
stash, including when it leaves conflicts. **Git: Stash saved changes with
message** asks for a message and includes untracked files. **Git: Stash
current file** stashes only the saved file in the active tab. Save open
edits before stashing or applying. Stash diffs omit untracked files held in
the stash's separate parent.

Merge-conflict markers have **Current**, **Incoming** and **Both** buttons
on the opening line. **Git: Resolve next conflict** moves to a block; the
three **Git: Keep … conflict change** commands resolve the block containing
the cursor with the keyboard. Diff3 ancestor sections are handled too.
Resolutions are single undo steps and stay unsaved until you save them.
**Git: Resolve previous conflict** moves backward and wraps. **Git: Keep
incoming then current conflict changes** accepts both in reverse order.
**Git: Keep base conflict change** uses the ancestor in diff3 markers.

**Git: Open merge editor** on a conflicted file shows ours, base and theirs
(index stages 2, 1 and 3) in three columns above the file's text, which is
the result and stays freely editable. The stages are merged again with
`git merge-file --diff3`, so every conflict has its base even when the file
has plain markers. The columns line up conflict by conflict, fold long
unchanged runs, tint lines that differ from the base, and follow the
result's cursor; the wheel scrolls them. Each unresolved conflict's heading
has **Ours**, **Base**, **Theirs**, **Ours + theirs** and **Theirs + ours**
buttons, and the **Git: Keep …** commands do the same for the conflict at
the cursor. Each choice is one undo step. **Git: Resolve next conflict** and
**Git: Resolve previous conflict** move between conflicts. Headings say
whether each conflict is still in the result. During a rebase the columns
say that ours is the branch rebased onto and theirs your commit.
**Git: Complete merge** refuses while markers remain, then saves the
result, stages it and closes the columns. It does not commit the merge;
Continue does. **Git: Close merge editor** hides the columns. A file one side
deleted has no merge editor; resolve it from source control.

**Git: Toggle inline blame** shows the author, commit date and subject at
the end of the cursor's line when there is room. It uses the current buffer,
so unsaved lines show as uncommitted. **Git: File history** lists up to 200
commits, follows renames and offers a read-only revision or a comparison
with the current buffer.

**Git: History of selected lines** follows the saved selection through up to
200 commits with `git log -L`. Choose a commit to open its patch for that
range. With no selection it uses the innermost enclosing function, method or
class from the outline, from its first line to the end of its block (a
signature over several lines included), then the smallest enclosing
foldable block, then the cursor line. Save edits first.

**Git: Repository history** lists the latest 200 commits across branches,
with graph prefixes, branch names, dates and authors. Type in the list to
filter its message and author text. **Git: Search repository history** asks
Git for matching messages, `author:name`, or `path:src/file.rs`. Choose a
commit for metadata and its patch, individual file diffs, Revert or
Cherry-pick. Both mutations ask for confirmation and require saved buffers;
conflicts remain on disk for the normal conflict-resolution workflow.
**Git: Amend HEAD message and staged changes** opens the current subject for
review and rewrites HEAD with the staged tree. It checks both HEAD and the
index again before committing. Amend rewrites history, including commits
already pushed. Merge commits need parent selection for revert/cherry-pick
and are currently refused by Git. The compact graph omits connector-only
rows between commits.

**Git: Choose branch review base** lists local branches to choose the base
for committed changes. Its first choice is `origin/HEAD` when configured,
then `main` or `master`. The review lists files changed since the merge base.
**Git: Compare branches or commits** asks for two revisions separated by a
space and opens their exact diff, with unified and side-by-side views.
Choose a file for its diff, or **Ask agent to review branch** to send the
diff to the current agent. Finish any ongoing
agent turn and clear its composer first. Reviews need the usual API key and
respect spending limits. Diffs above 100 KB require individual file review;
commit-message drafts have a 60 KB limit. Binary files can be staged and
committed; text comparisons require UTF-8 files.

**GitHub: List pull requests** uses an authenticated `gh` installation and
lists up to 100 open PRs. Choose one to check out its branch or list its
latest 100 current, right-side review comments, each marked Resolved or
Open from its thread. A comment opens its source
line and shows its body in the status bar. The checked-out commit must match
the PR head; outdated and left-side comments are omitted. While the PR's
branch is checked out, its comments also stand in amber after their lines,
with the reviewer's name and the comment's first line; comments on one line
share it. They hide while the file has unsaved edits, and **GitHub: Hide
review comments beside lines** hides them all.

A PR's **Review threads** list shows its latest 100 threads, read through
`gh api graphql`, with their resolved state, file, line, first comment,
author and comment count. Choose a thread to reply, resolve or unresolve it
by its `reviewThreads` ID. **Reply…** asks for the reply's text. Every send
asks for confirmation; reopen the list to refresh its state.

**GitHub: Add line comment to pending review** asks for a comment on the
current line or selection. Up to 100 comments are held locally in this
window. **GitHub: Submit review as Comment**, **GitHub: Submit review as
Approve** and **GitHub: Submit review as Request changes** ask for the
review's body, then confirm before sending it with the pending comments.
An approval may have an empty body. Bodies are limited to 60 KB.
Submission targets the recorded commit, refuses a changed HEAD, and
refuses when the pull request's head is not that commit. Pending comments are cleared after success or when
a comment is added on another commit. They are not persisted or synchronized
with GitHub draft reviews. Only current right-side diff lines are supported;
GitHub rejects lines outside the PR diff.

**GitHub: Refresh current PR CI status** reads the current branch's PR and
keeps its check summary in the status bar when no task or message occupies
it. Refresh is explicit; switching branches hides the old summary.
**GitHub: Show failed CI logs** downloads failed-job logs from the latest
failed Actions run on the current branch with `gh run view --log-failed`,
keeping the last 2 MB of a longer log. It removes job, step
and timestamp prefixes and opens a read-only tab. Compiler and traceback
locations use the task error parser and appear in Problems with source CI.
Standard `/home/runner/work/project/project/` paths map to the local project;
other layouts need relative paths or local absolute paths. Refresh replaces
previous CI problems. Only locations in existing local files are listed.
**GitHub: Ask agent to draft pull request description** sends the committed
diff against main to the current agent, with a 60 KB limit. Copy the reviewed
Markdown title and body into a text buffer, then use **GitHub: Create draft
pull request from current buffer**. It asks for confirmation and invokes
`gh pr create --draft`, sending the body on stdin. The first line is the
title, with an optional Markdown heading marker. Nothing is pushed by a
separate Lightdown command, but `gh` may offer its normal branch publication
workflow. Commands use `gh` credentials and time out after two minutes.

## How it works

There is no separate preview. The file on disk is plain markdown and stays the
document you edit; Lightdown renders each piece of it where it stands and
shows the underlying syntax only where the cursor is.

| Piece | Rendered | Source appears when the cursor is |
|:------|:---------|:----------------------------------|
| Bold, italic, strike, underline (`<u>`), code, links | styled text, markers hidden | inside or touching that span |
| Headings | sized, bold, `#` hidden | on that line |
| Lists, tasks, quotes | bullets, checkboxes, quote bars | on that line |
| Code blocks, front matter | monospace panel, fences hidden | inside the block |
| Tables | aligned grid | inside the table |
| Rules | a line | on that line |
| Images | the picture, under its line | on that line (its markdown shows above the picture) |
| Mathematics, `$…$` and `$$…$$` | typeset by TeX: in the line, or centred on a line of its own | inside the formula |

Fences labelled `mermaid`, `graphviz` or `dot`, and `plantuml` or `puml`
show a diagram below their source. Lightdown runs `mmdc`, `dot` or
`plantuml` on a background worker when needed, then draws the SVG with its
bundled renderer. Missing tools and renderer failures appear in the status
bar. **Markdown: Render document diagrams** retries after installing a
tool. Source is limited to 1 MB per diagram, rendering to sixty seconds,
and pictures to 32 million pixels. Rendered diagrams are cached by source.

PDF builds include diagrams already prepared in the cache, using a PNG
made by the same SVG renderer. Wait for the diagram to appear before
building or exporting a PDF; an unrendered fence stays code in the PDF.

Mathematics is set by the TeX engine that builds PDFs (see
[LaTeX and PDF](#latex-and-pdf)), a batch of formulas at a time in the
background, and drawn in the colour of the text. Until a formula is built,
or if TeX cannot build it, its source shows as code. Without a TeX engine all
of it stays source. *Draw mathematics* in the settings turns this off.

Press **Ctrl+E** for **Edit mode**, which shows all syntax at once while
keeping the styling. The status bar shows the current mode, *Live* or
*Source*, and a click on it switches; so does the first button at the
right of the tabs.

Images are loaded from local files (PNG, JPEG, WebP, GIF and SVG), scaled
down to the column with their colours preserved. An animated GIF plays while
it is in view, and an SVG is drawn sharp at whatever size it is shown.
Dark line art on a transparent background is shown light so it stays
visible. An image that cannot be loaded, or that points at the web, keeps its
alt text. Click a picture to jump to its markdown; Ctrl+click opens the file.

Markdown underlines broken local links, images and heading anchors in red.
Checks run in the background after input and refresh about every three
seconds. **Markdown: Check local links and anchors** also reports the
first failure in the status bar. Open notes contribute their unsaved
headings. Web addresses are left unchecked.

Moving or renaming a file or folder, with F2, the folder view or cut and
paste, looks for markdown links that led to it: inline links, pictures and
wikilinks, and the moved notes' own relative links. When there are some,
the list of files to update opens: choose one to read its diff, or apply
them all as unsaved, undoable edits, and save each after review; Esc leaves
the links as they are. Saving a markdown file whose headings were renamed,
in place and at the same levels, says how many files link to the old
anchors; **Markdown: Update links to renamed headings** opens the same
list for them. Reference-style link definitions are not updated.

Type `[[` in markdown to complete project note paths. Tab or Enter accepts
a completion. Ctrl+click `[[note]]`, or **Markdown: Follow or create
wikilink**, opens the note and creates a missing `.md` file. `[[note|label]]`
and `[[note#heading]]` work too. Names resolve beside the current file,
then from the project root, then by a unique filename. Use a folder for
ambiguous names. New notes stay within the project.

**Markdown: Show or refresh backlinks** opens the right panel's Backlinks
tab. It lists wikilinks to the current note, with their file, line and
source text; click a row to go there. Refresh after editing links or
switching notes. Scanning runs on demand in the background, includes open
unsaved text and reads at most 64 MB, with a 2 MB limit per saved note.

Inside a markdown table, Tab and Shift+Tab select the next or previous
cell, skipping the separator. Tab from the final cell adds a row. Columns
align as cell text changes. **Markdown table** commands align columns,
insert or delete rows and columns, and sort body rows by the current
column, numerically when both cells are numbers. The header and at least
one column stay in place. Each change is one undo step. Tables inside
lists or quotes keep their usual source editing.

**Markdown table: Paste CSV as table** reads plain clipboard CSV or TSV.
The first row becomes the header. Quoted commas, doubled quotes and
multiline cells work; line breaks in cells become `<br>`. Ragged rows or
incomplete quotes report an error. CSV accepts up to 4 MB, 10,000 rows and
200 columns.

Ctrl+V converts clipboard HTML into markdown: headings, lists, links,
emphasis, tables and code. Scripts, styles and remote resources do not
run. Tables accept up to 100 rows and 30 columns, with 400 characters per
cell; larger tables report a message instead of losing content.
Ctrl+Shift+V, **Edit: Paste plain text**, inserts the clipboard's plain text
without conversion or URL wrapping. HTML conversion has a 4 MB limit.

Select text and paste a single `https://`, `http://`, `ftp://` or `mailto:`
address with Ctrl+V to make `[text](url)` in markdown or `\href{url}{text}`
in LaTeX. **Edit: Paste URL over selection as link** is the palette action.
Undo restores the selection's text in one step. LaTeX documents need `hyperref`.

Paste a PNG with Ctrl+V into a saved markdown document, or drop local
pictures on its editor. Lightdown saves a copy in `assets/` beside the
document and inserts `![](assets/image-1.png)`, choosing a new name each
time. **Markdown: Paste image into assets** does the same from the palette.
Images are limited to 32 MB; clipboard PNGs also to 32 million pixels.
Save an untitled document first. Undo removes the inserted markdown and
keeps the asset file. Drops elsewhere still open files.

Other files open by their name: `.tex` as [LaTeX](#latex-and-pdf), `.pdf`
as [its pages](#reading-a-pdf), a PNG, JPEG, WebP, GIF or SVG as
[the picture](#viewing-a-picture), `.py`
as [Python](#python), `.txt` as plain text in the reading font, and
everything else as code in the system monospace font. The name of the kind
at the right end of the status bar is a button (Ctrl+Alt+M): it shows the
file as another kind, Markdown, LaTeX, Python, Plain text or Code. Only how
the file is shown changes, and only for as long as it is open. A notebook
always shows its cells.

## Files

The file's menu (Alt+F, or its name in the title bar, or ⋯ at the right of
the tabs) shows the actions that apply to the current document. PDF export
and preview appear for editable Markdown and LaTeX files, and LaTeX export for
Markdown. Notebook run controls follow the selected cell and kernel state;
read-only views omit editing actions. In editable views, Copy, Cut, Undo and
Redo remain visible but disabled when temporarily unavailable.

Text and notebook tabs check for changes on disk about once a second and
when the window regains focus. Clean tabs reload while keeping the cursor
and scroll. A dirty tab asks whether to keep edits or reload. Enter keeps
them, Shift+Enter reloads, and Esc dismisses the dialog. Keeping edits does
not write them until the next save. A deleted file also needs this choice
before saving recreates it. Saving compares the file with the version
loaded, so a change cannot silently overwrite newer disk contents.

Alt+Left and Alt+Right return to previous and next jump locations, including
files opened through search, project symbols and terminal links.

Files open in tabs along the editor's head. An unedited tab has an italic name and
is replaced by the next file opened. Editing keeps the tab, including after
saving, so opening another file creates a new tab. Opening a file already in a
tab switches to it. Each tab keeps its cursor, scroll, undo history and notebook
kernel. Unsaved changes have an amber dot, which turns into × under the pointer.

Click a tab to switch, its × to close, or middle-click it. Ctrl+Tab and
Ctrl+Shift+Tab switch forward and backward; Ctrl+W closes the current tab.
Closing a tab with unsaved changes asks before discarding them.
Ctrl+Alt+Shift+T, **File: Reopen closed tab**, brings back the last closed tab,
including its cursor, selection, scroll and undo history. The window keeps
up to 20 closed tabs in memory, including untitled tabs. Closed notebook
kernels stop and start afresh when a cell is next run. **File: Close saved
tabs** closes every clean tab, leaving edited tabs and running notebooks.
**File: Close the other tabs** keeps the current tab and any unsaved ones.

Closing the window or Ctrl+Q keeps unsaved text and untitled tabs in private
recovery files under `~/.local/share/lightdown/recovery`. The next launch
restores them, with their cursors, selections, scrolling and undo/redo history.
Notebook source and outputs are kept too; kernels start afresh. Checkpoints
run about every two seconds after startup, so a crash can lose the latest
two seconds. **File: Keep a recovery checkpoint now** writes one immediately.
A failed checkpoint reports an error; quitting waits until the
checkpoint succeeds or the files are saved. Other live windows keep their
own recovery files. Closing an individual tab explicitly discards its edits.

The status bar counts words, Unicode characters including spaces, and reading
time for markdown and plain text. With a selection it shows selected/total
words, selected characters and the selection's reading time. Reading time
rounds up at 200 words per minute. **Text: Show word, character and reading
counts** reports both selection and document counts for any text document;
notebook cell dividers are excluded.

Wayland input methods compose in editor and agent composer panes, dialogs,
the command palette and menu filters, find and replace fields, settings,
the file chooser, test filters, commit messages, Vim command lines and
terminal input. Preedit
text stays outside the document until committed. Candidate windows follow
the caret. Terminal commits go directly to the PTY as typed text.

Hebrew, Arabic and mixed-direction text use Unicode bidirectional ordering
in source, live markdown, reading panes, agent transcripts and text fields.
Text remains stored in logical order. Arabic uses contextual shaping from
the selected font or a system fallback. Paragraph direction follows the
first strong character; soft lines in a markdown paragraph share its
direction. RTL prose aligns to the right.
Left and Right follow visual positions; Home and End keep logical order.
Clicks map back to source positions, and a mixed-direction selection can
have several highlighted pieces on one row.

Markdown and notebook columns are up to 54 ems wide, 1.5 times the previous
width. The source line number appears in the left gutter. Notebook cell
headers show a cell number, and source line numbers restart in each cell.

The title bar holds the explorer's toggle at the left, the terminal's and
the right panel's at the right, and between them the file's name with its
folders in the project. Click the name (Alt+F) for what can be done with the
file, each with its keys; type to narrow the list.

| Keys | Action |
|:-----|:-------|
| Ctrl+Alt+N | Make a new file and open it; a name ending in `/` makes a folder |
| F2 | Rename or move the file: give a new name, or a folder to move it into |
| Ctrl+Alt+S | Save a copy under another name, and stay with this file |
| Ctrl+Alt+X | Move the file to the trash |
| Ctrl+Alt+O | Show the file's folder in the file manager |
| Ctrl+Alt+C | Copy the file's path |
| Ctrl+Alt+P | Export as PDF: see [LaTeX and PDF](#latex-and-pdf) |
| Ctrl+Alt+M | Show the file as another kind |

Names are typed in a dialog, starting from the file's folder, or
from the window's folder when no file is open, and complete with Tab. Folders that are not there yet are made. None of these
writes over a file. A name that is taken is refused. Renaming moves the file
on disk and leaves unsaved changes unsaved.

*Open* (Ctrl+O) puts up a file chooser on the file's folder. It lists what
the folder holds, folders first, each file with its size.

| Keys | Action |
|:-----|:-------|
| Up, Down, PgUp, PgDn, Home, End | Choose a file or folder |
| Enter, click | Open the file, or step into the folder |
| Ctrl+Enter, Ctrl+click | Open the folder chosen, or else the one shown |
| Ctrl+Shift+N | Make a new folder here |
| Right / Left | Step into the folder / out of this one |
| Backspace | Take back what was typed, then step out |
| Esc, a click outside | Close the chooser |

Typing narrows the list to the names that hold what is typed, and reads as
a path: `/` steps into the folder named, `../` out of this one, and a path
that starts with `/` or `~/` leads there from wherever the chooser is. A
path that is pasted leads to its file. Hidden files show once a `.` is typed
first.

The first row, *Open this folder*, opens the folder the chooser shows. An
opened folder becomes the window's project and shows in the sidebar, and
stays the project while the files opened are in it. Like a file, it opens
in this window if nothing would be lost, or else in a new one;
`lightdown some/folder` opens one from the command line.

The second row, *New folder…* (Ctrl+Shift+N), makes a folder in the one
shown: type its name, starting from whatever was typed to narrow the list,
and Enter makes it and steps into it, with *Open this folder* chosen. A name
like `notes/2026` makes the folders between too. A name that is taken is
refused; Esc goes back to the list.

Moving to the trash asks first, in a dialog: Enter or *Move to trash* moves
the file there, through `gio` or `trash-put`, so the file manager can
restore it; Shift+Enter or *Delete permanently* deletes it for good; Esc
keeps it. The window is left with an empty document.

## Saving and formatting

`auto_save` saves the active file by itself: `off` (the default),
`on_focus_loss` when the window loses the keyboard, or `after_delay` one
second after typing stops. Set it in Settings (*Auto-save*), or per project
in `.lightdown/settings.json`. It saves only files with a path, with the
usual cleanup and format on save. It never saves over a file changed on
disk. The status bar says it was skipped, and saving by hand offers to
reload. Tabs with an open conflict dialog, notebooks with running cells and
`--wait` windows are skipped. A failed save is reported once and waits for
the next edit or focus loss.

Saves trim trailing spaces and tabs and keep a final newline. These changes
are undoable. `.editorconfig` files override the defaults, from the file's
folder up to `root = true`. Matching sections support `*`, `**`, `?`,
character classes, brace alternatives and numeric ranges. Supported properties
are `indent_style`, `indent_size`, `trim_trailing_whitespace`,
`insert_final_newline`, `end_of_line` and `charset`. LF, CRLF and CR endings
are supported. The editor writes UTF-8, optionally with a BOM. Other charset
requests stop the save with a message. `unset` removes an inherited rule.
Indentation rules apply to Tab, outdent and Python's continued indentation.
Without a rule, opening a file infers tabs or 2, 4 or 8 spaces from its first
2,000 lines. Tab, outdent, Python auto-indent and indent guides use that hint.
Files without a consistent hint keep the language default.

The status bar shows the indentation unit, *Spaces: 4* or *Tabs*. Its menu,
or **File: Set indentation for this file**, sets tabs or 2, 4 or 8 spaces
for this file until it is closed, over EditorConfig and the hint. *Reindent
with this unit*, also **File: Reindent with this unit**, rewrites each
line's leading whitespace in that unit as one undo step; whitespace left
over, less than a unit, stays as spaces, and lines in multi-line strings
are reindented too. Beside it are the line endings, *LF*, *CRLF* or *CR*,
as the file had them when it opened (a file whose lines all end in a lone
CR reads as CR). Its menu, or **File: Change line endings**, converts the
file; the change is one undo step and is written on the next save. Then
the encoding, *UTF-8* or *UTF-8 with BOM*; its menu, **File: Change
encoding**, and **File: Add or remove the byte order mark** add or remove
the mark as one undo step. An `.editorconfig` `end_of_line` or `charset`
still applies on saving.

Run **File: Toggle format on save** in the command palette to enable
formatting. It starts off disabled. Set commands in
`~/.config/lightdown/settings.json`, or the XDG config location:

```json
{
  "format_on_save": true,
  "formatters": {
    "py": "ruff format -",
    "rs": "rustfmt --emit stdout",
    "ts": "prettier --parser typescript",
    "go": "gofmt"
  }
}
```

Set `"code_actions_on_save": ["source.organizeImports", "source.fixAll"]`
globally or in `.lightdown/settings.json` to request those source actions
before formatting. The default is an empty list. A kind also matches its
sub-kinds, such as `source.fixAll.ruff`. Actions have a shared ten-second
timeout and join formatting and whitespace cleanup in one undo step. They
use the editor's running server when it holds the same text, and otherwise
start a server for that save. Only edits to the saved file are accepted;
cross-file edits, file operations and actions requiring server commands stop
the save. When two actions change the same text, the first applies and the
other waits for the next save. Files without a language server save as usual.

Commands run in the project folder, read text on stdin and return formatted
text on stdout. They must be installed separately. An empty command disables
that extension. Formatting has a ten-second timeout; a failure leaves the
file unsaved. A successful result and whitespace cleanup form one undo step.
Editing or switching away while formatting requires saving again. Notebook
JSON is saved through the notebook writer without source formatting.

**File: Local history** lists snapshots of the current saved file, newest
first, independent of git. Open one read-only, compare it with the current
buffer, or restore it as one unsaved undo step. Save after reviewing a
restore. Comparisons use the same unified and two-column diff views as git.
Each save records the previous disk text and the saved text; identical
successive versions share one entry. Snapshots live under the XDG data
folder's `lightdown/history`, with private permissions, at most 100 versions
per path and 128 MB in total. Oldest snapshots go first. Text files up to
2 MB are kept. Notebook JSON is snapshotted, but buffer restore and comparison
currently require a text tab. History follows absolute paths, so renaming a
file starts a new timeline. Snapshots are written in the background.

## Project tasks and errors

F6 opens project tasks. Lightdown reads static Make targets, Cargo commands,
`package.json` scripts and `[project.scripts]` in `pyproject.toml`. Python
projects also get `python -m pytest`. Python entry-point discovery needs
Python 3.11 or newer for `tomllib`. Package scripts use pnpm, Yarn or Bun when
their lockfile is present, otherwise npm. Tasks run against files on disk.

Each task runs in its own terminal tab named after it, on a pseudo-terminal
with colour, input and the terminal's scrollback. When the job ends the tab
stays open and prints its exit status, such as `[Exited with status 1]`.
The status bar shows how many jobs are running and the last result. The
command palette has **Task: Show output**, **Task: Show errors**, **Task:
Rerun last task**, **Task: Stop job…** and **Task: Stop**. Compiler locations
in `file:line:column` or `file(line,column)` form and Python traceback
locations become error entries and Problems. F8 goes to the next, Shift+F8
to the previous. F8 includes locations from completed jobs in the current
project, retaining up to 512 locations.

Up to 16 jobs run at once. Foreground tasks end after an hour; background
tasks run until stopped. F6 lists running jobs first with their state
(running, waiting for a ready pattern, ready, or waiting for another task);
selecting one stops it and leaves the others running. **Task: Stop job…**
also offers each compound run configuration as a whole. **Task: Stop** stops
every job, waiting task and watch.

**Task: Show output** opens a job's captured output as plain text in a
read-only tab. It keeps the last 256 KB of each job and the last 32 logs,
and lists running jobs with Stop actions beside the finished logs. There is no setting to run a task without a terminal tab.

`.lightdown/tasks.json` adds named tasks beside discovered tasks:

```json
{"tasks": [
  {"name": "prepare", "argv": ["mkdir", "-p", "build"]},
  {"name": "check", "shell": "printf 'checking\\n'", "cwd": ".",
   "env": {"MODE": "test"}, "dependsOn": ["prepare"]}
]}
```

Each task has exactly one of `argv`, a literal string array, or `shell`, an
explicit shell command. `cwd` is relative to the project or absolute. `env`
contains string values. Dependencies name tasks in this file and run in
order, stopping on failure. Cycles, missing dependencies and invalid values
are reported when F6 opens. There are at most 128 tasks in a 256 KB file,
with dependency depth limited to 32 and expanded commands to 256 KB.
`problemMatcher` is an optional Rust regex string applied to output lines.
It requires named `file` and `line` groups; `column`, `severity` and `message`
are optional. Lines and columns are one-based. Existing files become Problems
and F8 locations. For example, `(?P<file>.+)@(?P<line>[0-9]+): (?P<message>.+)`
recognises `src/main.rs@12: failed`. Severity is retained in the message.

`watch` lists project-relative globs, such as `["src/**/*.rs", "*.toml"]`.
`*` stays within a folder and `**` crosses folders. After the task is
started from F6, saving a matching file reruns it, 300 ms after the last
save. A save while it runs queues one rerun for when it ends. **Task: Stop**,
or stopping the job, ends the watch.

`background` marks a server or watcher that keeps running:

```json
{"tasks": [
  {"name": "serve", "argv": ["npm", "run", "dev"],
   "background": {"readyPattern": "Local:.*http://"}},
  {"name": "e2e", "shell": "npx playwright test", "dependsOn": ["serve"]}
]}
```

The task counts as started once a line of its output matches
`readyPattern`, a Rust regex. Tasks that depend on it start then, each in its
own tab; a dependency already running is reused. If the background task ends
before it is ready, its dependents are not started. A watched background
task is restarted on a matching save. `background` must be an object with
only a nonempty `readyPattern`; `watch` must hold 1 to 32 globs of at most
256 characters, without `..` or a leading `/`. Errors name the task and are
reported when F6 opens. Callers that run a task as one command, such as a
run configuration's `preLaunchTask`, skip background dependencies; start
those first.

**Run: Choose configuration** reads `.lightdown/run.json`. F7, or **Run:
Start default configuration**, runs the named default:

```json
{
  "default": "app",
  "configurations": [
    {"name": "app", "command": "python3", "args": ["main.py", "--verbose"],
     "env": {"APP_MODE": "dev"}, "cwd": ".", "preLaunchTask": "make build"}
  ]
}
```

`command` and `args` are literal argv values. `cwd` is relative to the project
or absolute. Optional `env` overrides the project's inherited environment
for the program. `preLaunchTask` names an existing F6 task, runs in the project
folder, and must succeed before the program starts. Run configurations use
the task output, error navigation, rerun and stop commands, with the same
one-hour timeout and 16-job limit. They run saved files in named terminal tabs with colour, input and 5000 lines of scrollback.
An entry such as `{"name":"full stack","compound":["backend","frontend"]}`
starts the named configurations concurrently, each with its own log. Compound
members must be ordinary configurations, with no duplicates. **Task: Stop**
stops all active jobs. **Task: Stop job…** lists individual jobs and compound
groups, so a compound can stop without interrupting unrelated jobs. F6 also
lists running jobs with their state and an individual Stop action.
There are at most 128 entries, with unique names. Configuration is read only
when requested and does not affect startup.

## Editing code

**Code: Install language grammar** lists the known tree-sitter grammars,
each pinned to one release commit. Choosing one downloads its source from
GitHub and compiles a native library with `cc` in the background; `curl` and
`tar` are also required. Nothing is downloaded or compiled any other way.
The grammar is tried once before it is kept, so a broken build leaves
nothing behind. Open files switch to it as soon as it is ready.
**Code: List installed grammars** shows each installed one with its release,
its tree-sitter ABI version and the file types it serves. **Code: Remove
grammar** deletes one; its files return to lexical tools. Grammars and their
`highlights.scm`, `injections.scm` and `locals.scm` queries live in
`$XDG_DATA_HOME/lightdown/grammars/`,
defaulting to `~/.local/share/lightdown/grammars/`. They are native code, so
install only grammars you trust. A grammar built for an ABI this build does
not read is refused before it is loaded; reinstall it.

With a grammar installed, bundled per-language outline, fold and text-object
queries drive folding, the outline, sticky headers and selection expansion.
Outline queries capture items, names and context, including CSS selectors,
JSON and YAML keys and SQL declarations. One worker
thread parses, started on the first request. It keeps the last tree and
reparses only what an edit touched, including nested injection layers.
Embedded languages must also be installed; missing ones keep host highlighting.
Queries combine embedded regions and exclude interpolation children as directed
by the grammar. Requests superseded by a newer edit are
dropped before they run, and results for old text are never shown as current.
Files over 2 MB, a parse over 500 ms and query work over 300 ms keep
the lexical tools. The worker shares an 800 ms budget with nested languages
and caps injection nesting at eight layers.

Ctrl+/ toggles comments using the file's language. It supports line comments
for Rust, JavaScript, TypeScript, Go, C-family languages, Python, shell,
TOML, YAML, SQL, Lua and LaTeX, plus block comments for HTML, XML and CSS.
Files such as JSON that have no comment syntax are left alone.

Alt+Up/Down moves the current line or selected lines. Ctrl+Alt+D duplicates
them, and Ctrl+Shift+K deletes them. A selection ending at the next line's
first column excludes that line. Notebook cell headers remain protected.

Typing brackets or quotes closes the pair, or wraps selected text. Typing a
generated closing character advances over it. Backspace between an empty
pair removes both characters. Matching brackets are highlighted; the syntax
highlighter excludes strings and comments from matching.
Ctrl+Alt+B jumps to the matching bracket. Ctrl+Alt+Shift+B selects the pair
and the text between it, in either direction. The palette names these
**Edit: Jump to matching bracket** and **Edit: Select to matching bracket**.

## Finding and replacing

Ctrl+F opens the find bar at the top right of the editor, starting from the
word selected. It finds as you type, marks every match in the text and says
which one is chosen of how many. Enter or ↓ goes to the next match,
Shift+Enter or ↑ to the one before; F3 and Shift+F3 do the same from the
document. Case matters only if what you type has capitals, unless **Aa** is enabled.
The **ab** button matches whole words; **.*** enables regular expressions.
The Search commands in the palette offer the same options for project search. Click in the
document and the bar stays, giving the keys back; Ctrl+F returns to it and
Esc closes it.

Select a range, then turn on **[]** in the find bar, or **Search: Toggle
find in selection** (Alt+L), to limit matches, navigation and Replace all
to that range, shown as a faint band. The range moves and grows with edits;
closing the find bar or switching tabs clears it.

Ctrl+H, or the chevron at the bar's left, adds a second field, *Replace*;
Tab goes between them. Enter replaces the match chosen and goes to the
next, Shift+Enter goes back without replacing, and Ctrl+Enter replaces
every match at once, as one step for Undo. Both have buttons too.

Ctrl+Shift+F opens the **Search** tab of the right panel with the word
selected, and searches every file of the project as you type. In a git
repository that is the files git knows of and does not ignore; elsewhere,
everything outside hidden folders and folders such as `target` and
`node_modules`. Binary files and files over 2 MB are left out. Open tabs are
searched from their buffers, unsaved edits included; other files from disk.
Type at least two characters. Matches stream in while the search runs,
grouped by file with a count for each file and in total, up to 50,000
matches; long lines show only the part around each match.

**Aa**, **ab** and **.*** beside the query (Alt+C, Alt+W, Alt+R) match case,
whole words and regular expressions, shared with the find bar. *Include*
and *Exclude* under it open **Search: Set included paths** and **Search: Set
excluded paths**: comma-separated globs such as `src/**,*.py` or
`**/generated/**`. Empty filters include all paths or exclude none.

Up and Down in the query go through the queries searched this session.
Enter opens the first match with it selected, and the list keeps the keys:
Up and Down choose a row, Enter opens a match or folds a file, Left and
Right fold and unfold, Delete dismisses a match or a file from the list,
and typing goes back to the query. Tab goes between the query, the
replacement and the list; Esc gives the keys back to the document. A click
on a file folds it; a click on a match opens it.

The chevron at the query's left shows a *Replace* field. Each match then
shows what it becomes, the match struck through in red and the replacement
in green. Pointing at a match or a file shows buttons to replace it (also
Ctrl+Enter in the list), to preview a file's diff, and to dismiss it.
Replace all (Ctrl+Enter in the fields) lists the planned diffs to review
first; its Apply row applies them. Every replacement covers only the matches
listed, becomes unsaved edits in tabs, one undo step per file, and the
search then runs again. A file changed on disk or a tab edited since the
search is refused. Search again. Notebooks, pictures and read-only tabs are
refused too. **Search: Replace project matches** in the palette plans the
same for the last query, and **Search: Review pending project
replacements** returns to the list after viewing a diff. Regex
replacements accept `$1` and `${name}` capture references; literal
replacements keep `$` unchanged. **Search: Open results in editor**, or the
pen button beside the result count, opens the matches as a multibuffer (see
*Multibuffers*), where editing a match's line edits its file in place. The **aA** toggle, also **Search: Toggle
preserve case** (Alt+P in the panel), adapts replacement text to
lowercase, initial capital or all capitals for each match. Mixed-case
matches keep the replacement as entered.

## Editing selections, folds and snippets

Ctrl+D selects the word at the cursor, then adds each next matching
selection. **Edit: Select all occurrences** uses Ctrl+Alt+L and selects up to
10,000 literal, non-overlapping matches. **Edit: Skip occurrence** uses
Ctrl+Alt+Shift+D to move the current selection to the next match without
keeping the current one. Ctrl+Alt+Up/Down adds a cursor on the adjacent source line.
Alt+Shift+drag selects a rectangular range by character columns. Typing,
paste, deletion and movement apply to every selection. Each change is one
undo step; Esc returns to one cursor. Notebook cells use one selection.

Alt+Shift+Right expands the selection through a word, quoted string,
brackets, line and enclosing blocks. Alt+Shift+Left restores the previous
selection, including its direction. The palette has **Edit: Expand
selection** and **Edit: Shrink selection**. With a tree-sitter grammar installed, each
step also grows through query-defined function, class, parameter and comment
objects, both inside and around, and enclosing syntax nodes. Source files ask their server
for selection ranges in the background, used by subsequent expansions;
lexical ranges work immediately without a server. Editing or moving the
selection starts a new history. Notebook ranges stay within one cell.

**Code: Fold or unfold current block** hides the current brace block,
indented block, markdown section or LaTeX environment. Its header shows
how many lines are hidden. **Code: Unfold all blocks** reveals the file.
Folding uses lexical source rules, or the parse tree when a grammar is
installed: each multi-line syntax node folds from its first line. A jump into a hidden block reveals it;
editing unfolds the document.

User snippets live in `~/.config/lightdown/snippets.json`, or the XDG config
location. Project snippets in `.lightdown/snippets.json` merge with them;
the project wins for an identical prefix. Both are keyed by extension:

```json
{
  "rs": [
    {"prefix": "fun", "body": "fn ${1:name}() {\n    $0\n}"}
  ],
  "py": [
    {"prefix": "defn", "body": ["def ${1:name}(${2:args}):", "    $0"]}
  ]
}
```

Type a prefix and choose the snippet in the completion list, or ask with
Ctrl+Space. Tab or Enter expands it. Tab and Shift+Tab then move through
numbered stops; `$0` is the final position. Defaults are optional, as in
`$1`, and can hold other stops, as in `${1:name ${2:value}}`. A number used
again is a mirror: typing in the first `$1` changes every other `$1` as you
type. What is typed in one stop, mirrors included, is one step to undo.
`${1|one,two,three|}` offers a list of choices when the stop is reached;
the first is inserted. Malformed stops report an error without changing the
buffer.

Templates can use `$TM_FILENAME`, `$TM_FILENAME_BASE`, `$TM_DIRECTORY`,
`$TM_LINE_NUMBER`, `$TM_SELECTED_TEXT`, `$CLIPBOARD`, `$CURRENT_YEAR`,
`$CURRENT_MONTH`, `$CURRENT_DATE`, `$CURRENT_HOUR`, `$CURRENT_MINUTE`,
`$UUID` (a random version 4 UUID) and `$WORKSPACE_NAME` (the project
folder's name), with optional braces around each name. `${VAR:default}`
uses the default when the value is empty or unknown. Inserted values are
literal text, even when they contain dollar signs or braces.

## Vim mode

Modal editing is off by default. Choose **Modal editing: vim** in Settings,
**Preferences: Toggle Vim mode** in the palette, or `"modal_editing": "vim"`
in settings.json. Existing `"vim_mode": true` settings migrate to Vim;
`modal_editing` takes precedence. Editor panes use modal keys. The terminal,
agent input, dialogs and palette keep their usual keys. The status bar shows
the mode, incomplete commands and the `:` command line. Esc returns to normal.

Insert mode keeps completion, snippets, automatic pairs, suggestions and
Enter's indentation. Completion and suggestions stay hidden in other modes.

| Keys | Action |
| --- | --- |
| `i a I A o O`, `R` | Insert, append, open lines, replace mode |
| `v V`, Ctrl+V | Character, line, rectangular visual selection |
| `h j k l`, `w b e W B E`, `0 ^ $`, `gg G` | Character, source-line, word and file motions |
| `f t F T ; ,`, `%`, `{ }`, `H M L` | Find character, match bracket, paragraph, visible-line motions |
| `d c y > <`, `gu gU g~ gc` | Delete, change, yank, indent, case and comment operators |
| `x X r s S C D Y`, `p P`, `J gJ`, `~` | Character edits, paste, join, change case |
| `u`, Ctrl+R, `.` | Undo, redo, repeat the last change |
| `m`, backtick, apostrophe | Set a mark, jump to its position or line |
| `gv`, `g;`, `g,` | Reselect the last visual selection, older or newer change |
| `q{a-z}`, `q{A-Z}`, `q` | Record a macro, append to a macro, stop recording |
| `@{register}`, `@@` | Play a register as keys, repeat the last macro |
| Ctrl+E, Ctrl+Y | Scroll down or up by a line |
| Ctrl+D, Ctrl+U | Scroll down or up by half a screen |
| Ctrl+F, Ctrl+B | Scroll forward or back by a screen, with two lines of overlap |
| `zt zz zb` | Put the cursor line at the top, middle or bottom |
| `z<Enter> z. z-` | Same placement, moving to the first nonblank character |
| `/ ?`, `n N * #` | Find forward or backward, repeat, find the cursor word |

Counts work with motions, operators, scrolling and macros: `3dw`, `2d3w`,
`3ix`, `5@a`. Operators accept doubled forms and text objects `iw aw iW aW`,
`i" a" i' a'`, backtick quotes, `i( a( ib ab`, `i{ a{ iB aB`, `i[ a[`,
`i< a<` and `ip ap`. Bracket objects nest and accept an outer-pair count.
Installed tree-sitter grammars add `if af` for functions, `ic ac` for classes,
`ia aa` for parameters and `iC aC` for comments; counts choose enclosing
objects, with operators and in visual mode.
`j` and `k` pass over folded source lines. Visual block uses native multiple
selections, so `I`, `A` and `c` type on each selected line. Mouse selections
and the editor's multiple selections also accept visual operators.

Registers are per pane: unnamed, `"a` through `"z` with capitals appending,
`"0` for an unnamed yank, `"1` through `"9` for successive line or multiline
deletes, `"-` for a small delete, `"_` to discard and `"+` for the clipboard.
Macros live in those same registers: `"ap` pastes a recorded macro and
`"ay` can replace it with selected text. Recursive playback stops at 32
nested macros or 10,000 instructions. Each command and macro playback is
one undo step. Dot repeats inserted text, accepted completion edits and
visual changes, including visual changes followed by insertion.

| Ex command | Action |
| --- | --- |
| `:w`, `:q`, `:q!`, `:wq`, `:x`, `:qa` | Save, close, force close, save and close, close window |
| `:e path`, `:o path` | Open a file |
| `:buffer-close`, `:bn`, `:bp` | Close, next or previous tab |
| `:format`, `:noh`, `:42` | Format through the language server, clear search, jump to line |
| `:s/pattern/replacement/[giIenp#l]` | Substitute on the current line or a range |
| `:g/pattern/d`, `:v/pattern/d` | Delete matching or nonmatching lines |

Substitute accepts `%`, numbers, `.`, `$`, relative offsets such as `.+2`
and comma ranges such as `:3,$-1s/a/b/g`, as well as visual lines. Flags
are global `g`, ignore case `i`, force case `I`, suppress missing-pattern
errors `e`, count without editing `n`, print `p`, print with line number `#`
and print with an end marker `l`. Global commands default to the whole file,
accept a range, `g!` inverts the match, and `d`, `y`, `p` or `s` act on each
matching line. Patterns use Rust regex syntax. Groups are `( )`, with `\1`
or `&` in replacements. `*` and `#` find literal words without boundaries.

Vim takes Ctrl+R, Ctrl+V and the scrolling Ctrl keys in normal and visual
modes. Insert mode keeps the editor's shortcuts. Notebook cell headers stay
protected. Clipboard paste inserts at the cursor; a block yank pastes plain
text. Macro playback handles document keys and control keys; searches,
popup navigation, completions and file commands inside macros do not replay
through the window. Arrow keys recorded during insertion are not preserved.
Dot does not restore snippet placeholder navigation or disjoint visual shapes.
Visual block repeat uses the rectangle's dimensions. Remaining Vim gaps include
sentence motions and objects, search motions for operators, `''` and double
backtick jumps, `gi ge gj gk`, insert-mode Ctrl+O/Ctrl+W/Ctrl+R, substitute
confirmation, `:set`, `:w name`, `:e!` and arbitrary Ex commands in `:g`.

## Helix mode

Choose **Modal editing: helix**, **Preferences: Toggle Helix mode**, or
`"modal_editing": "helix"`. The status bar shows `NOR`, `INS` or `SEL`.
Normal mode selects text before editing it. `v` extends selections until
Esc or another `v`; Esc leaves insertion and keeps a character selected.
Selections use the editor's existing multiple cursors and share undo steps.

| Keys | Action |
| --- | --- |
| `h j k l`, `w b e W B E`, `f t F T` | Replace or extend each selection by a motion |
| `x X`, `%` | Select lines, extend lines upward, select the document |
| `;`, Alt+`;`, `,`, Alt+`,` | Collapse to heads, flip direction, keep primary, remove primary |
| `C` | Copy each selection to the line below, with a count for the distance |
| `s`, `S`, Alt+`s` | Select regex matches, split on a regex, split on newlines |
| `&`, `_` | Align selection starts, trim surrounding whitespace |
| `d c y p P r ~` | Delete, change, yank, paste after or before, replace, toggle case |
| `u U`, `> < J` | Undo, redo, indent, outdent, join lines |
| `i a I A o O` | Insert or append at selections, line starts or ends, new lines |
| `gg ge gh gl gs` | First line, last line, line start, line end, first nonblank |
| `gd gr gi gy` | Definition, references, implementation, type definition |
| `gn gp g.` | Next tab, previous tab, last change |
| `mm`, `mi{object}`, `ma{object}` | Match bracket, select inside or around an object |
| `ms{pair}`, `mr{old}{new}`, `md{pair}` | Surround, replace a surround, delete a surround |
| Space `f b s / k r a` | File finder, tabs, symbols, project search, hover, rename, code actions |
| Space `y p` | Copy selections to the system clipboard, paste the clipboard |
| `:` | Ex commands, including `:w :q :wq :o :buffer-close :format` |

For `s` and `S`, type a Rust regular expression and press Enter; Esc cancels.
Counts apply to motions, lines, character finds and paste. Match objects
reuse Vim's word, quote, bracket and paragraph objects. Language navigation,
formatting, rename and actions require the corresponding language server.

This is a Helix subset. Registers, macros, selection rotation, jump history,
syntax-tree selection, view commands, full match-mode menus and Helix's own
command/config language are not implemented. Search uses Lightdown's find.
New lines do not yet copy indentation in Helix. Joining and indenting may
collapse multiple selections. Paste repeats the entire yank at each selection
rather than pairing individual yanked selections with target selections.

## Language servers

Lightdown starts a language server in the background for Rust, TypeScript,
JavaScript, Go, C/C++, Python, Lua, Zig, Bash, YAML, JSON, CSS, HTML,
Markdown, TOML, LaTeX, Dockerfile, PHP, Java and Kotlin. Ruby, Swift and
Haskell use ruby-lsp, sourcekit-lsp and haskell-language-server when installed.
Python prefers basedpyright, then pyright or ruff. Interpreter completion
for Python and notebooks and the existing LaTeX features remain available.

Project `.venv/bin`, `venv/bin`, `node_modules/.bin` and PATH take precedence
over managed servers. A missing server shows **Install <server>** in the
status bar; click it and choose Install. The global `lsp_auto_install`
setting accepts `ask` (default), `always` or `never`. Installations go only
into `$XDG_DATA_HOME/lightdown/servers/<name>`, normally
`~/.local/share/lightdown/servers/<name>`. They use GitHub release binaries,
local npm packages, a private Python venv or `go install` with a private
GOBIN. Install tools must be available: curl, npm, python3 with venv, or Go;
zip releases need unzip and xz releases need tar. Java needs Java 21+.
Progress and failures appear in language status. **Code: Update language
servers** checks installed versions; choose Install again to update a server.
Servers stay alive while switching tabs and start after the first frame.

Per-language configuration in `.lightdown/settings.json` uses LSP IDs:

```json
{"language_servers":{"python":{"command":"basedpyright-langserver",
  "args":["--stdio"],"initialization_options":{},
  "settings":{"basedpyright":{"analysis":{"typeCheckingMode":"basic"}}}}}}
```

Omit command to keep automatic discovery; `false` disables a language.
Restart the server after changing options. The older `.lightdown/lsp.json`
command overrides remain supported.

F12 goes to a definition, Shift+F12 lists references, and Ctrl+F2 renames a
symbol. Ctrl+F12 goes to an implementation, Ctrl+Shift+F12 to a type
definition, and Ctrl+Alt+F12 to a declaration when the server offers it.
These commands open a location list when there are several results; its
first row, **Open all in editor**, shows every location in a multibuffer.
**Code: Find all references in editor** does that for references directly. Hover
or Ctrl+K shows documentation. Diagnostic ranges get red
underlines; **Code: Show language diagnostics** lists their messages. The first
message on each line also appears after the source and any inlay hints,
coloured by severity and truncated to fit. **Code: Show or hide inline diagnostics** toggles those labels.
Hover over an underline to read its message. When the server has code
actions for the cursor line, a lightbulb appears left of the line numbers:
amber when a quick fix is among them, grey otherwise. Click it to open the
same menu as Ctrl+Alt+.. Each line is asked about once per edit.
Alt+F12 (**Code: Peek definition**) and Alt+Shift+F12 (**Code: Peek
references**) show the target's lines in a block under the current line,
without leaving the file. With several results, the block lists them: Up and
Down choose one, Enter or a click opens it, and Esc closes the block. Open
tabs show their unsaved text; other files are read from disk, up to 2 MB
each and 200 results. Editing the file closes the block.
Alt+Shift+H shows the incoming calls of the function at the cursor in the
right panel's Hierarchy tab, and Ctrl+Alt+Shift+H its outgoing calls.
Alt+Shift+U shows a type's supertypes and Alt+Shift+D its subtypes. Each row
names the symbol and its file and line. Click a row to go to it and to load
or fold its own callers, callees or types; levels load only when opened, and
a tree holds at most 5,000 rows. Servers without call or type hierarchy
support report that in the status bar.
Rename validates the request's file snapshots, then opens changed files
as unsaved tabs. Review and save them, or undo each file. Targets outside
the project are refused. Rename snapshots have a 32 MB total limit and a
2 MB per-file limit. Workspace edits can create, rename and delete files,
including overwrite and ignore options and recursive directory deletion.
Resource edits apply in order as a checkpointed disk operation; open tabs
follow renamed paths and deleted files close. A failed write restores the
captured files. Text-only refactors stay in unsaved, undoable tabs.

When the server offers semantic tokens, they colour the code on top of the
lexical highlighting: functions, methods and macros as functions; types,
traits, namespaces and lifetimes in the type colour; enum members and
booleans as numbers. Variables and parameters keep the plain
colour. Tokens are asked for after each change, as deltas when the server
keeps them, as the whole file's range when it offers only ranges, and again
when the server asks for a refresh. Until the reply for an edit arrives the
file shows its lexical colours. A token adds its colour where the lexical
highlighter gave none; where both colour a word, comments, keywords and
strings keep theirs. At most 200,000 tokens are kept per file.

Ctrl+Space requests server completion. Tab or Enter accepts it, including
server text edits, imports and numbered snippet stops. Ctrl+Alt+Space shows
signature help; typing a call's `(` or `,` requests it too when autocomplete
is on. Ctrl+Alt+. opens **Code: Code actions and quick fixes**; text
actions open unsaved, undoable buffers after validating the request snapshot.
**Code: Format document with language server** formats the buffer in one
undo step. **Code: Format selection** requests range formatting for the
selected text, also in one undo step. Edits outside the selection are refused;
select complete lines for servers that format whole lines.
**Code: Show or hide inlay hints** requests type and parameter
labels, grouped at the ends of their source lines. Request hints again after
editing. Stale replies are discarded. Code actions run their server command
after applying any edit. **Code: Run code lens** lists the server's code
lenses and runs the selected command. Servers can request workspace edits
through `workspace/applyEdit`. Explorer create, rename, move and delete
operations ask registered servers for import edits before changing files,
and send the corresponding completion notification afterwards. Code actions
run on request, or through `code_actions_on_save` when configured.

Override a server command in `.lightdown/lsp.json`. Values are argument
arrays, with no shell expansion:

```json
{
  "rust": ["rust-analyzer"],
  "py": {"languageId": "python", "command": ["ruff", "server"]},
  "md": {"languageId": "markdown", "command": ["marksman", "server"]},
  "sh": {"languageId": "shellscript", "command": ["bash-language-server", "start"]}
}
```

Keys can be extensions, with or without a leading dot, or the existing
language IDs. Extension entries take precedence. An array uses the standard
language ID for known extensions and the extension itself for others. Set
an entry to `false` to disable it. Invalid configuration reports an error.
Restart language servers after changing the file.

The status bar names the file's language server with a dot: amber while it
starts or works, green when ready, red when it crashed or is not installed,
grey when stopped. Work the server reports replaces the name, such as
"rust-analyzer: Indexing · 12/40 45%". Click the item to restart or stop the
server or to read its log; **Code: Stop language server** and **Code: Show
language server log** do the same from the palette. The log keeps the newest
64 KB of the server's stderr and log messages, and the popup shows its last
200 lines. Errors the server asks to show also appear in the status bar.
Servers advertising a diagnostic provider are asked for pull diagnostics after
each change and when they request a refresh; published diagnostics remain
supported.

## Problems

**View: Show problems** opens the right-panel Problems tab. It groups the
current project's language diagnostics, task errors, CI errors, failed tests and PDF
build errors by file. Click a message to jump; F8 and Shift+F8 walk the same
list in either direction and wrap. Results refresh as their jobs finish.
Diagnostics for edited buffers disappear until the server publishes a new
version. Tests with no known source location appear under Project.
Right-click a row and choose **Ask agent to fix** to append its message,
file, location and nearby source to the agent composer for review. Nothing
is sent automatically. The code-actions menu also offers this for diagnostics
on the cursor line, as does **Code: Ask agent to fix diagnostic**.
**Problems: Open in editor** opens every problem with a file as a
multibuffer, each excerpt's messages under it. It refreshes as problems
change: new problems get excerpts, and excerpts left without problems go,
except the one the cursor is in. Edits made there stay in the files.

## Multibuffers

A multibuffer is one editable tab of excerpts from many files. Each excerpt
shows a few lines around a line of interest, two lines each way, under a
header naming its file and line; excerpts of one file that touch are joined.
The gutter shows each file's own line numbers, and each excerpt is
highlighted by its file's language. Search results, problems, references
and the agent's hunk review open as multibuffers.

The text is edited with the usual editor: cursors, selections, multiple
cursors, Find and Replace, completion. An edit stays within one excerpt;
one that would cross or remove a header is refused. Edits go to the file's
buffer. That is the open tab's, so unsaved text is shared both ways, or
else a buffer the multibuffer keeps for the file. A file opened later takes that
buffer's unsaved edits into its tab. Undo and Redo undo the multibuffer's
own steps in the files' buffers; a step edited since in the file's tab is
refused, so undo it there. Ctrl+S saves every file the multibuffer edited,
and refuses a file changed on disk since it was read. Closing the tab with
unsaved edits asks first, as for any tab. Edits made in a file's tab move
the excerpts with their lines, and an excerpt whose lines are deleted goes.

| Key | Action |
|---|---|
| Alt+Enter | Open the file at the cursor's line (**Multibuffer: Open file at cursor**) |
| Alt+PageUp / Alt+PageDown | Show five more lines above / below (**Multibuffer: Expand excerpt upward** / **downward**) |

A click on a header's name opens the file at that line; **↑** and **↓** at
its right expand the excerpt.

## Outline

When an enclosing heading, function or class scrolls out of view, up to
three headers stay at the editor's top. Click one to return to its source.
**View: Toggle sticky headers** switches this for the current pane. Headers
use the outline and block ranges, from the parse tree when a grammar is
installed, and are off in notebook cells.

*Outline* in the right panel's head, or Ctrl+Shift+H, lists the document's
headings and definitions there: markdown's `#` headings, LaTeX's `\part` to
`\subparagraph`, and Python's classes and functions. In a notebook it lists
the headings of markdown cells and the functions of code cells. Each is
indented by its level, the one the cursor is under is marked, and a click
goes to it. Ctrl+R finds one by name from the palette. Source files also list lexical
declarations such as functions, classes and structs. With a tree-sitter
grammar installed, the outline comes from the parse tree: functions,
methods, classes, structs, enums, traits, impls, interfaces, modules and
namespaces, nested by where they are declared, plus TOML tables, CSS selectors,
JSON and YAML keys and SQL declarations. Ctrl+Alt+R parses files with installed
grammars on the project index thread and caches symbols by path, mtime, size
and grammar generation. Other files use the lexical scan, including notebook cells. Unsaved open tabs
replace the corresponding disk entries. Files over 2 MB are skipped, and
each indexing pass reads at most 64 MB. This is a text index, so it does not
resolve types or references. When the running server offers document symbols,
Outline and Ctrl+R use its symbols instead, including nested definitions.
When it offers workspace symbols, Ctrl+Alt+R asks it for the name typed so
far, one request at a time, and lists up to 10,000 results with their file
and line. While replies are pending, unsupported or stale, both use the
local index.

## Folder

The explorer's toggle at the left of the title bar, or Ctrl+Shift+E, opens
the left sidebar with a view of the project's folder: the window's folder holding the file, if
there is one; or else the nearest directory with a `.git`, or else the file's own.
Folders come first and unfold when clicked; the file that is open is
marked, and the way to it is unfolded. Its head makes a new file or folder,
folds every folder and reads the folder again. Files stay on the left while the
agent or PDF preview opens on the right, so both sides can stay open. Each
sidebar has its own toggle on the title bar and its own divider, and
remembers its width and visibility. Ctrl+Shift+L hides both sidebars. Below 640 logical
pixels, the most recently opened sidebar fills the available width; selecting
the other sidebar shows it, and widening the window restores both.

The left sidebar's head switches between the **Explorer** and **Source
Control** (see [Git](#git)); the choice is remembered. Ctrl+Shift+E goes to
the explorer and Ctrl+Shift+G to Source Control, and each goes back to the
document when pressed again. A narrow sidebar names only the view on show.
In Source Control:

| Keys | Action |
|:-----|:-------|
| Up, Down, Home, End | Choose a file or section; Up from the top goes to the message |
| Enter | Open the file's diff |
| Alt+Enter | Open the file |
| Space | Stage the file or section, or unstage a staged one |
| Delete | Discard the changes, after asking |
| Tab | Go to the message field, and back |
| Enter, in the message | Commit the staged changes |
| Ctrl+Enter | Commit, from anywhere in the view |
| Esc | Give the keyboard back to the document |

In the explorer:

| Keys | Action |
|:-----|:-------|
| Up, Down | Choose a file or folder |
| Right / Left | Unfold a folder or step into it / fold it or step out |
| Enter | Open the file, or fold and unfold the folder |
| Ctrl+Enter, Ctrl+click | Open the file in a new window |
| F2 | Rename or move the file or folder chosen |
| Delete | Move it to the trash; Shift+Enter then deletes it for good |
| Esc | Give the keyboard back to the document |

Right-click a file or folder for what can be done with it: open it, make a
new file or folder there, rename or move it, duplicate it beside itself
(`notes copy.md`), move it to the trash, copy its path or its path within
the project, open a terminal there or show it in the file manager. *Cut* or
*Copy* it, then right-click a folder and *Paste* it there; a copy never
writes over a name that is taken, and a move is refused instead. When the
open file, or the folder it is in, is moved or renamed, the window follows
it; when it is deleted, it is closed, unless it holds unsaved changes, which
stay to be saved anew. Right-click the empty space under the list for the
project's own folder.

A file opens in a tab in the window it was picked in. Edited tabs and
notebooks with running cells stay open when another file is picked. Ctrl+click
or Ctrl+Enter explicitly opens a separate window. A PDF opens as its pages
and a picture as itself. Any other file that is not text, such as an audio
file, is handed to the application the desktop opens it with.

Everything in the folder is listed but `.git`; hidden files are dimmed.
The list is read again when a file is saved, made or copied, when the agent
has used a tool, and when the window gets the keyboard back.

In a git repository, the branch shows at the left of the status bar, and the
folder view marks what git counts as changed: **M** modified, **A** added,
**U** untracked, **R** renamed, **D** deleted and **!** in conflict. A
folder holding changes has a dot. Git is asked again every few seconds while
the window is in use, and after each save.

A folder opened in a window remembers what the window held when it was
closed: its tabs, each with its cursor, the tab that was shown, its
terminals, each started again in the folder its shell was last in, and the
other folders added to the window. Opening the folder again brings them
back. Unsaved changes return through crash recovery on the next launch.

### Several folders in one window

A window can hold several folders, each with its own name at the top level
of the explorer, unfolded under it. `lightdown app lib` opens two folders in
one window; files named beside them open in tabs.

| Command | Action |
|:--------|:-------|
| **File: Add folder to window…** | Choose a folder in the file chooser and add it after the others |
| **File: Remove folder from window** | Choose one to take out; its open tabs stay |
| **File: Save folders as workspace** | Write the folders to a workspace file |
| **Agent: Choose the folder it works in** | Fix the agent to one folder, or let it follow the active file |

Right-click a folder's name in the explorer to remove it. Its name cannot be
renamed or trashed from there. The window title and the title bar's project
label show the active file's folder, or the first folder and how many more,
such as `app +1`.

The folder holding the active file is the project: its settings, Python
interpreter, project environment, dev container, git branch in the status
bar, tests panel and run configurations. Switching to a tab in another
folder switches all of these. What is project-wide covers every folder:

- The file finder (Ctrl+P) and project symbols list every folder's files. A
  path found in two folders, such as `README.md`, shows its folder's name.
- Project search and replace, and the TODO scan, search every folder;
  results show the folder's name before the path.
- **Task: Run** lists the active folder's tasks, then each other folder's
  after its name, and runs a task in its own folder. Run configurations and
  the tests panel are labelled with the active folder.
- A new terminal starts in the active file's folder; with no file from the
  window's folders open, it asks which folder.
- Each folder has its own language server, kept running while the window
  holds it; Problems lists all of them.
- Source Control groups changed files by repository, under each folder's
  name. Staging, discarding, diffs and commits act on the row's repository;
  the commit field commits the repository of the chosen row.
- The agent works in the active file's folder, or in the folder chosen with
  **Agent: Choose the folder it works in**; a turn stays in the folder it
  started in. That folder's permissions, `AGENTS.md` and model apply. It can
  read files in the window's other folders, by full path or as
  `name/path`, but edits and commands stay in its own folder. The
  conversation stays when switching between the window's folders.

A file opened from outside the window's folders keeps its own project. A
window of one folder follows such a file out, as before; a window of
several keeps its folders.

A **workspace file**, `name.lightdown-workspace`, lists folders that open
as one window: `lightdown my.lightdown-workspace`, or pick it from the
recent list. Paths are absolute, start with `~/`, or are relative to the
file; `name` replaces the folder's name in the explorer and labels:

```json
{"folders": [{"path": "app"}, {"path": "../shared-library", "name": "library"}]}
```

Adding or removing a folder in such a window rewrites the file. The window's
session is kept under the workspace file. **File: Save folders as
workspace** writes to that file, or else to `<first folder>.lightdown-workspace`
beside the first folder.

## Python

`.py` files use colour highlighting in both themes: purple keywords, green
strings, amber numbers, cyan functions and builtins, and pink decorators.
Keywords stay bold and comments use readable secondary text. Enter keeps the
indentation, deepens it after a colon or an open bracket and steps back
after `return` and its like; Backspace in the indentation goes back one
level; Ctrl+/ comments lines out and in.

Completions appear as you type. Names used in the file, keywords and
builtins come at once; members after a dot (`math.sq`) and module names in
`import` lines come from a small Python helper started on first use. The
helper uses [jedi] if the interpreter has it and otherwise looks things up
itself, importing only installed modules, never the project's own files.
Up and Down choose, Tab or Enter accepts, Esc dismisses, Ctrl+Space asks.

Documentation comes from the same helper, as in an IDE:

- **Hover** over a name for its signature, where it comes from and its
  docstring. This works for builtins (`len`), installed libraries
  (`json.dumps`, through the file's own imports), and the file's own
  functions, classes and `self.` methods, which are read, not run. The wheel
  scrolls a long docstring.
- **Ctrl+K** shows the same for the name at the cursor. It stays until the
  cursor leaves the name or Esc is pressed.
- **Parameter hints** appear when you type a call's `(` or `,`: the
  signature, with the parameter being typed in bold blue, and the start of
  the docstring. Keyword arguments (`sep=`) and `*args` are followed. The
  hint goes when the cursor leaves the call, or with Esc.
- **The completion list** shows the chosen name's documentation beside it.

In a notebook the documentation comes from the running kernel, so names the
cells have defined are known as well, with their types. With jedi installed,
signatures include type hints and variables in files are known too.

Python source files can also contain cells separated by `# %%` lines.
Ctrl+Enter runs the current cell, including unsaved edits; Shift+Enter
runs it and moves to the next marker. `# %% [markdown]` cells are skipped,
and markers inside strings stay part of the string. The kernel retains
variables between runs in the same project and uses its interpreter and
`.env`. **Python: Show cell output** opens the log; **Notebook: Variable
explorer** lists its variables. The Python palette commands interrupt or
restart this kernel. Restarting clears its variables.

Cells run on the same kernels as notebooks (see below): the interpreter's
ipykernel when it has one, else the built-in helper. Other languages with
an installed Jupyter kernel get cells too, once the file has a marker:

| File | Marker | Kernel |
| --- | --- | --- |
| `.py` | `# %%` | ipykernel or the built-in helper |
| `.R` | `# %%` | IRkernel (`ir`) |
| `.jl` | `# %%` | IJulia |
| `.ts`, `.js` | `// %%` | Deno (`deno jupyter --install`) |

**Notebook: Select kernel** in such a file picks the kernel for its
language in this project. Output goes to the same log. A cell that calls
`input()` reads an empty line.

## Notebooks

An `.ipynb` file opens as one text: every cell under a thin divider, markdown
cells rendered like any markdown, code cells on a panel with their output
beneath. The cursor moves through the cells as through one document, so
selecting, finding and undoing work across them; the dividers themselves
cannot be typed over. Copying cells gives other programs `# %%` lines.

Each notebook cell has mouse controls in its header. ▶ runs a code cell,
the cell's kind (*Python* or *Markdown*) converts it, + adds a code cell
below it, and the ⋯ menu includes adding cells above or below, deleting a
cell, running all cells and kernel actions. Run all and Restart the kernel
are at the right of the tabs too. Narrow windows keep the
menu and fit the other buttons where there is room. Right-clicking a cell
opens its context menu too.

Cells run on a Jupyter kernel when one is there, as in JupyterLab or Zed,
and otherwise in a Python helper that Lightdown starts itself, which needs
only `python3`. The interpreter is the project's `.venv` if there is one,
or whatever the settings name.

Kernels are the kernelspecs installed in `$JUPYTER_PATH`,
`~/.local/share/jupyter/kernels`, the project's `.venv` and `venv`, the
interpreter's prefix, `/usr/local/share/jupyter/kernels` and
`/usr/share/jupyter/kernels`, plus the interpreter's own ipykernel. A
notebook gets its kernel this way:

- A Python notebook runs on the interpreter's ipykernel if it has one, so
  the project's packages are there; on another Python kernel its
  `kernelspec` names; else on the built-in helper.
- Any other language (R, Julia, Deno/TypeScript, Rust with evcxr, ...)
  runs on the kernel its `kernelspec` names, or the first installed for its
  language. Code cells are highlighted in that language.
- **Notebook: Select kernel**, or a click on the kernel in the status bar,
  lists the built-in helper and every kernel found. The choice holds for
  the notebook; saving writes its `kernelspec` and `language_info`, and a
  chosen built-in helper is remembered under `metadata.lightdown`.

Jupyter kernels are started through a small bridge that runs on the
project's interpreter and needs its `jupyter_client` (ipykernel brings it).
Without it a Python notebook falls back to the built-in helper and the
status bar says how to install it; other languages cannot run. Interrupt
(Ctrl+.) and restart (Ctrl+Shift+R) act on the kernel, and closing the
notebook shuts it down. Completion and Ctrl+K documentation come from the
kernel. When a cell calls `input()` a field opens under it: type, then
Enter sends the line and Esc interrupts. Passwords show as dots.

Outputs are shown and saved as Jupyter saves them, with execution counts.
The richest form that can be drawn is used: HTML tables (below), PNG,
JPEG and SVG images, markdown drawn like the document's own, LaTeX set by
the TeX engine that sets formulas in markdown, JSON pretty-printed, then
plain text. Tracebacks and streams keep their terminal colours.
`clear_output` (also with `wait=True`) and updated displays
(`display_id`) work as in Jupyter. The built-in helper offers the same
forms from `_repr_png_`, `_repr_svg_`, `_repr_markdown_`, `_repr_latex_`
and the like.

DataFrame HTML representations and saved `text/html` tables render as
native rows and columns under cells. Lightdown reads table text only;
scripts, styles and linked resources do not run. The view shows at most
100 rows, 30 columns and 400 characters per cell, with a truncation notice.
Notebook saves preserve the original HTML. Other HTML falls back to plain
text when that representation is available.

## Tests

**Tests: Show tests** (Ctrl+Shift+Y) opens the right panel's Tests tab and
lists the project's tests the first time. Lightdown finds each runner a
project uses, and a project may have several:

- **cargo test** in a Cargo project, or **cargo nextest** instead when
  `cargo-nextest` is installed. Tests are grouped by module.
- **pytest** where there is a `pytest.ini`, `conftest.py`, pytest settings
  in `pyproject.toml`, `setup.cfg` or `tox.ini`, or `test_*.py` files. It
  runs under the project's interpreter, which must have pytest.
- **Jest** and **Vitest** when `package.json` depends on them, started
  through the package manager its lockfile names: `pnpm exec`, `yarn`,
  `bun x` or `npx`, none of which installs anything. Jest lists only its
  files at first; a file's tests appear once it has run.
- **go test** in a Go module, grouped by package.
- **CTest** where a configured build folder is found: the project itself,
  `build`, `cmake-build-*` or `out/build`. Each test's place is its
  `add_test` line.

**Tests: Discover cargo tests** and **Tests: Discover pytest tests** add
that runner where it is not detected, and **Tests: Refresh tests** lists
everything again.

The tab is a tree of runners (when there are several), files, modules or
packages, and tests. Each shows its state, green for passed, red for
failed, amber while running, and each test the time it took. Type in the
field at the top to filter by runner, file or name; `is:failed`,
`is:passed`, `is:skipped` and `is:running` filter by state. Under the
pointer, a row has a button to run it, and pytest tests one to debug them
with **Debug: Run one pytest test**; other runners cannot debug a test.
Clicking a test opens its declaration, or the line it failed on. The head
runs all tests, reruns failed ones, runs all with coverage and refreshes
the list; while tests run it has Stop. **Tests: Run tests in this file**,
**Tests: Run test at cursor** and **Tests: Debug test at cursor** act on
the open file, **Tests: Pick a test to run** lists the tests in the
palette, and the gutter's play marks run the test declared on their line.
Tests operate on disk, so save project edits first. Pass/fail marks reset
when their source changes. **Tests: Show output**, or Output in the tab,
opens the log and **Tests: Stop tests** cancels the run.

A failed test's message is shown, dimmed red, after the line it failed on,
as inline blame is, and failed tests are listed in Problems. **Tests:
Compare expected and actual** opens the side-by-side diff of the two
values when the runner reports both: pytest's `assert actual == expected`,
the left and right of Rust's `assert_eq!`, Jest's and Vitest's Expected
and Received, and Go messages written `got X, want Y`. It compares the
test chosen in the tab, or the one at the cursor, or lists the failed
tests that have values.

**Tests: Show or hide coverage in the gutter** shades the line numbers of
measured lines: green for lines that ran, red for lines that did not. It
reads the newest report in the project's folder: `lcov.info`,
`coverage.lcov`, `coverage/lcov.info`, `target/lcov.info`,
`target/llvm-cov/lcov.info`, `coverage.json` or `coverage/coverage.json`.
Write one with your usual tools, such as `cargo llvm-cov --lcov
--output-path lcov.info`, `coverage lcov`, or `coverage json` after
`coverage run -m pytest`. **Tests: Run all with coverage**, also in the
Tests tab's head, runs `cargo llvm-cov --lcov --output-path lcov.info`
(`cargo llvm-cov nextest` with nextest), and the selected Python
interpreter's `coverage run -m pytest` followed by `coverage json`. Other
runners have no coverage run. Install cargo-llvm-cov or Python coverage
first. Existing reports at those
paths are replaced. Python writes a report even when tests fail. These runs
use the same worker, cancellation and saved-source checks as other tests. The status bar
gives the total and the current file's share. While coverage is shown, a
report written again is read within a couple of seconds; **Tests: Reload
coverage report** reads it at once. A file edited, or saved after the report
was written, shows no marks until the next report. Reports over 128 MB are
refused.

## HTTP requests

A `.http` or `.rest` file holds requests as REST clients write them, parted
by lines that start with `###`:

```http
@host = https://api.example.com

### List items
GET {{host}}/items
  ?page=2
Accept: application/json

### Create one
POST {{host}}/items
Authorization: Bearer {{$dotenv API_TOKEN}}
Content-Type: application/json

{"name": "first"}
```

Put the cursor in a request and press Ctrl+Alt+Enter, or run **HTTP: Send
request at cursor**. A request is its method and URL (GET when only a URL is
given), lines that start with `?` or `&` continuing the URL, headers up to
a blank line, then the body. Lines that start with `#` or `//` before the
body are comments. `@name = value` lines define variables for the requests
below them; `{{name}}` uses one, and `{{$dotenv NAME}}` or `{{$env NAME}}`
takes a value from the project's `.env` or the environment. An unknown
variable stops the request before anything is sent.

The response opens in a read-only tab: the status line, the time taken and
size, the headers, and the body, with JSON laid out. Bodies over 1 MB are
cut there; binary bodies give their size. The status bar says the code and
time. **HTTP: Stop request** cancels one in flight.

Requests go through `curl` on this machine, also in an SSH project, and only
to `http://` and `https://` addresses, redirects included (up to five). They
time out after 60 seconds, and responses over 50 MB are refused. Proxies set
in the environment apply, except for this machine's own addresses. The body
sent and the response are written to a private folder under the cache and
removed once shown. File includes (`< file`), response scripts, cookies kept
between requests and environment files are not supported.

## LaTeX and PDF

`.tex` files use purple, bold commands, green mathematics and quieter comments. Enter after `\begin{itemize}` puts `\end{itemize}` below
and an indented line between; Ctrl+/ comments lines out with `%`; Ctrl+B and
Ctrl+I wrap the selection in `\textbf{}` and `\emph{}`. Completion offers
commands after a backslash, those the document uses before common ones;
environments after `\begin{`, the open ones first after `\end{`; and labels
after `\ref{` and its kin.

Project `.bib` files supply citation completion after `\cite{` in LaTeX
and `[@` in markdown. Tab or Enter accepts the key. Hover a citation, or
Ctrl+K / **Writing: Show citation at cursor**, shows its title, authors,
year and publication. **Writing: Refresh project bibliography** reloads
the entries; lookups otherwise cache them for twenty seconds.

Markdown PDF builds turn `[@key]` and `[@one; @two]` into author-year
citations and append References for the entries used. Unknown keys stay
source. Inline code and fenced code stay untouched. BibTeX reading accepts
braced or quoted fields and numeric years, up to 1 MB per file and 4 MB per
project. String macros, concatenated fields and BibLaTeX citation styles
are not interpreted. LaTeX files keep their own bibliography commands.

F5 builds the document into a PDF and shows its pages in the right panel,
which the preview button at the right of the tabs opens and closes too. While the preview is open,
saving builds again. A badge at its top says how the last build went. When a build fails, the
preview lists the errors the engine reported, and a click on one goes to
its line; the status bar says the first.

- **LaTeX** is saved first and built where it is, so `paper.pdf` lands
  beside `paper.tex`, as with other TeX editors.
- **Markdown** is built by way of LaTeX, in a folder of Lightdown's own, so
  the document's folder is left as it is. Headings, emphasis, lists and
  tasks, quotes, code, tables, footnotes, links, local PNG, JPEG and PDF
  pictures, `$math$` and the `title`, `author` and `date` of front matter
  all have their LaTeX counterparts.

A LaTeX or markdown document is built with SyncTeX, which records where each line of the
source went. Ctrl+Alt+J, or *Show in the PDF* in the document's menu, shows
the line the cursor is on in the preview, lit. Ctrl+click on a page of the
preview goes back to the line it came from, opening the file if it is
another one, such as a chapter brought in with `\input`. Markdown builds
keep a source map beside the cached PDF. It maps prose lines and block
starts back to the saved markdown path, including unsaved text used in the
build. A changed buffer requires a new build before jumping. Code listings
and footnotes map to their containing block; untitled markdown needs saving
before source jumps. Exported PDF copies do not carry these cached maps.

*Export as PDF* (Ctrl+Alt+P) builds the document and writes the PDF where
you say; an earlier export of the same name is written over. *Export
markdown as LaTeX*, in the File menu, writes the LaTeX that markdown is
built from, to carry on with by hand.

**File: Export markdown as HTML** writes a standalone document with
headings, lists, tables, footnotes, links and embedded local images.
Mathematics is typeset by the configured TeX engine and exported as SVG
paths, with no web service or browser. A missing image or failed formula
stops the export with a message. Raw HTML other than `<u>` and `<br>` is
escaped. Remote images cannot be embedded. Images have a 32 MB limit and
HTML output a 64 MB limit.

Ctrl+Shift+C, **Markdown: Copy as rich text**, copies the selected markdown,
or the whole document when nothing is selected, as rendered HTML and plain
source text. Email and document editors can choose the formatted HTML.
It uses the same renderer, embedded pictures and SVG mathematics as HTML
export. Preparation runs in the background; a failed formula or missing
picture leaves the clipboard unchanged. Ordinary Ctrl+C still copies source.

**File: Export markdown as DOCX** and **File: Export markdown as EPUB** use
`pandoc` when installed, including unsaved source and local resources.
These commands run in the background and time out after a minute. Choose
the output path in the dialog. A failed export preserves the previous
output. HTML export is built in; no new library is needed.

Building needs a TeX engine. Lightdown uses the first it finds of its own
Tectonic, a `tectonic` on the `PATH`, `latexmk`, `xelatex`, `lualatex` and
`pdflatex`; *TeX engine* in the settings picks one, and `latex_engine` in
the settings file takes a program's path as well. With none installed, the preview offers
**Set up Tectonic**, which downloads the latest release for the machine
from GitHub, about 10 MB, into `~/.local/share/lightdown/bin`. It needs no
administrator rights. Tectonic fetches the TeX packages a document
needs the first time it is built, which takes a minute or two, and keeps
them under `~/.cache/Tectonic`. Lightdown draws the pages itself, at the
size they are shown, as they scroll into view. The ↗ button in the panel's head shows
the PDF in the desktop's viewer instead.

### Reading a PDF

A `.pdf` file opens as its pages, in their own colours, one under another
down the middle of the window, at their printed size or narrower if the
window is. The status bar says which page is in view. Nothing in it is
edited, and *Save as* writes a copy. When the file is written again, by a
build in a terminal for instance, the window shows it anew once it has the
keyboard back.

| Keys | Action |
|:-----|:-------|
| Up, Down, Page Up, Page Down, Space, Home, End | Move through the pages |
| Left, Right, Shift+scroll | Move across pages wider than the window |
| Ctrl+Plus, Ctrl+Minus, Ctrl+0, Ctrl+scroll | Zoom, up to four times the printed size |
| Drag, double-click, triple-click | Select text, a word, a line |
| Ctrl+A / Ctrl+C | Select all the text / copy what is selected |
| Ctrl+F | Find in the text; Enter for next, Shift+Enter for previous |
| Click a link | Go to the place it names, or open its web or mail address |

The text is read from the pages in the background as the PDF opens; the
agent gets it too when it reads the file.

Links and bookmarks are read in the background too. The pointer becomes a
hand over a link. A click that does not drag follows it: a link within the
document scrolls to its page and place, and a web or mail address opens in
the desktop's browser or mail program. Other link actions, such as opening
other files or launching programs, are ignored. A drag that starts on a
link selects text instead. The Outline tab (Ctrl+Shift+H) lists the PDF's
bookmarks, nested as the PDF nests them; a click goes to one, and the
outline marks the last bookmark above the top of the view, or the one just
chosen.

### Viewing a picture

A PNG, JPEG, WebP, GIF or SVG file opens as the picture, in its own colours,
in the middle of the window: at its actual size, one pixel of it to a pixel
of the screen, or smaller to fit the window. What it lets through shows a
checkerboard. The status bar gives its size in pixels and how large it is
shown. An animated GIF plays, and a JPEG is turned upright as its camera
recorded. Nothing in it is edited, and *Save as* writes a copy; when the
file is written again, the window shows it anew once it has the keyboard
back.

An SVG is drawn afresh at the size it is shown, so it stays sharp however
far it is zoomed. Its text is set in the fonts it names, or else in those
fontconfig picks for its generic family and for characters they lack. The
kind at the right of the status bar (Ctrl+Alt+M) shows the SVG as *Code* to
edit, and as its *Picture* again once it is saved.

| Keys | Action |
|:-----|:-------|
| Ctrl+Plus, Ctrl+Minus, Ctrl+scroll | Zoom, up to eight screen pixels to each of its own |
| Ctrl+0 | Back to its actual size, or to fit the window |
| Drag, arrows, scroll, Shift+scroll | Move about a picture larger than the window |
| Page Up, Page Down, Home, End | Move up and down a tall picture |

Zoomed in far, its pixels show as squares; an SVG can be zoomed to four
times the window. The agent is told the picture's name and size.

## Agent

Ctrl+L opens a sidebar for talking to a coding agent through [OpenCode] Zen
or Go. It needs an API key (in the settings, or `OPENCODE_API_KEY`) and
`curl`. While the agent works with its panel hidden, the status bar says
so, and a click shows it.

- **The composer.** The message box sits in a rounded card with the model,
  the reasoning and the @ context toggle under it, and a round send button,
  which turns into a red stop button while the agent works. Your messages
  stand in bubbles at the right; what the agent did is listed under a thin
  rule, and its answer follows.
- **IDE context.** While the @ toggle under the message box is on, each
  message carries what you are looking at: the open file and its contents,
  the cursor, the selection, and for a notebook every cell with its output
  and the state of the kernel. Click it, or press Ctrl+K, to turn it off.
- **Tools.** The agent can read, list and search the project's files, edit
  and write them, run shell commands, and read, change and run the cells of
  the open notebook. The open file is read as the editor holds it, unsaved
  changes and all, and a document never saved is there for the agent as
  `<untitled>`. Edits to the open file happen in the editor, as one undo
  step. They are saved if the file was saved before them; changes you have
  not saved, and a document without a file, are left for you to save.
- **Checking its work.** The `problems` tool reads the Problems list:
  source, file, line, severity and message for up to 200 entries. The
  `run_tests` tool runs `cargo test` in Cargo projects, or pytest in the
  project's Python otherwise, for one test ID or name filter, one test file
  (a Cargo integration test in `tests/`), or everything. It returns pass,
  fail and skip counts, each test's status and up to 2,000 characters of each
  failure, at most 200 tests, with the output itself when no test result
  could be read (a build error, say). It runs files as saved on disk, under
  the permission to run code, for up to 15 minutes, and is refused in plan mode.
- **Per-project controls.** A project is the nearest directory with a
  `.git`. For each project you choose whether the agent may edit files and
  whether it may run code: *ask* (the default), *allow* or *deny*. When
  asked, a card above the message box shows what it would do: Enter or
  *Allow once* allows it, A or *Always allow* allows it from then on, Esc
  or *Deny* declines. Reading is
  confined to the project. A project's `AGENTS.md` is given to the agent
  unless you turn that off, and each project remembers its model.
- **Models and reasoning.** Click the model's name (Ctrl+M) to pick from
  the models of both services, with their prices; type to filter. Click
  "reasoning" (Ctrl+R) to choose how hard the model thinks, from what that
  model offers. The list comes from [models.dev] and is refreshed daily.
- **Threads.** Conversations are kept per project. The pen in the panel's
  head starts one (Ctrl+Shift+N); the clock, or the thread's title above
  the conversation, lists them (Ctrl+T), and Delete removes the selected one.
- **Usage limits.** The line above the conversation shows how full the
  model's context is and what the thread has cost, by the model's list price. In the settings you
  can cap spending over five hours, a week and thirty days, the windows
  OpenCode Go counts in, and per project; at a cap the agent stops and says
  so. A refusal from the service itself, such as a Go limit, is shown as it
  came.

While a turn runs, Enter in the composer or **Agent: Queue follow-up** queues
your message. The count appears above the conversation. At the next tool
boundary, the running tool finishes, unstarted calls are skipped and the
follow-up goes to the model. At the end of a reply it continues immediately.
Up to eight messages can wait. **Agent: Return queued follow-ups to draft**
lets you revise them; stopping also returns them to the composer.

Enter sends, Shift+Enter starts a new line, Esc stops a reply or gives the
keyboard back to the document. Text in the conversation can be selected and
copied.

For a selected source range, Ctrl+Alt+I opens **Agent: Edit selected text
inline**. Enter an instruction; the suggestion model returns a temporary
replacement in the editor. Tab keeps it as one undo step, Esc restores the
original text. Typing or navigating also discards the preview before
acting. Replies for a changed buffer are discarded. This uses the configured
suggestion provider and the same spending limits as other agent requests.

The agent summarizes older completed turns before a request when context
usage reaches 75%. It keeps two recent user turns and their tool results
verbatim, and sends the summary with subsequent requests. The full
transcript stays visible and saved. **Agent: Summarize older turns** does
this explicitly. Summary requests use the current agent model and count
toward spending limits. A failed summary leaves the conversation intact
and reports the failure before continuing.

A project's `.lightdown/settings.json` can name a check for the agent's
work, as a shell command line or an argv array:

```json
{"check": "cargo clippy --all-targets && cargo test", "check_attempts": 2}
```

When a turn has changed files and the project allows the agent to run
code, the agent runs the check before the turn ends, in the project folder for up to 15 minutes; the thread shows each run
and its result. If it fails, its output (up to 12,000 characters) goes back
to the model as a message, and the model gets another round to fix it, up to
`check_attempts` times (0 to 10, default 2). A check still failing after
that, or when a spending limit is reached, ends the turn with its output.
The extra rounds are billed and limited like any other request. Stop
cancels a running check. The check does not run in plan mode, after turns
that changed nothing, or when **Project check** is off in the settings. It
runs with the agent's checkpoint, so files it changes can be reviewed and
reverted with the turn's other changes. With that permission at *ask* or
*deny*, the thread says the check was skipped; the check cannot pause for
approval at the end of a turn.

**Agent: Toggle plan mode** starts read-only investigation. The thread title
shows Plan. File edits, commands, notebook execution and MCP calls are blocked
until approval, including calls a model invents. The agent finishes with a
plan in its answer. **Agent: Edit proposed plan** puts that answer in the
composer; revise it and send to refine the plan. **Agent: Approve plan** sends
it for implementation and leaves plan mode. Ordinary edit and command
permissions still apply. These commands require an idle agent. Turning plan
mode off discards its pending plan; it does not start implementation.

## Agent edits, providers and project context

**Agent: Review changed files** lists the files changed in the latest agent
turn. Pick a file for its diff, **Keep**, or **Revert**. **Review hunks in
editor** opens every text file of the turn as an editable multibuffer (see
*Multibuffers*): each change is an excerpt of the file as it is now, with
its added lines marked green in the gutter and its removed lines listed in
red under it. **Keep** and **Revert** on the excerpt's header, or **Agent:
Keep current hunk** and **Agent: Undo current hunk** with the cursor in it
(Ctrl+Alt+Y and Ctrl+Alt+U), act on one change. Keeping removes it from
review; whole-turn rollback still restores the original checkpoint.
Reverting puts the removed lines back as an unsaved edit of the file, which
Ctrl+Z in the review undoes and Ctrl+S saves. Once saved, whole-turn
rollback refuses that file like any later edit. Binary files and notebooks
use per-file review. **Agent: Turn
checkpoints** opens an earlier turn, and **Agent: Roll back latest turn**
restores the whole latest turn. Checkpoints retain the last ten turns in the
current window, including unsaved text and notebook contents. Reverts check
all affected files before writing and refuse later edits. They restore new
files, deleted files and file permissions too. A rollback restores files and
buffers; it does not undo commands' external effects or live Python objects.
Command and notebook-run checkpoints cover up to 64 MB and 10,000 project
source files. They exclude `.git`, `target`, `node_modules`, `.venv`, `venv`,
`__pycache__`, `.cache` and symlinks. Explicit file edits checkpoint their
target even in an excluded directory. Checkpoints are kept in memory and
end when the window closes.

**Agent: New worktree window** asks for a short name, creates
`lightdown/<name>` from HEAD, and opens it in another Lightdown window.
Each window can run its own thread, terminal and tools at the same time,
with independent project permissions. Creation is explicit and does not send
a prompt. Worktrees live under the app's data directory, outside the project.

**Agent: Review and merge worktrees** lists these branches. Open a window,
review its committed diff and uncommitted status, or merge its committed
branch by fast-forward. Save editor buffers and commit the worktree first;
both working directories must be clean. Diverged branches require a manual
git merge. Untracked files appear in status and need individual review.
Worktrees and branches remain until you remove them with git.

**Agent: Worktree overview** lists these worktrees with their branch,
commits ahead of the recorded creation base, committed changed-file counts
and uncommitted status counts, including untracked files. Older worktrees
use their merge base with the current window's HEAD. Each row says whether
a window has the worktree open and whether its agent is working, and what
the worktree's saved threads cost, or that no cost is recorded. Pick a row
for the same Open, Review and Merge actions. Every window keeps a private
status file (mode 600) under `windows` in the XDG data folder with its
project, process ID, agent state and current thread cost. Files of
processes that are gone are ignored and removed, and a window removes its
own on close. The view refreshes when invoked; it lists Lightdown-created
worktrees under its data directory.

**Agent: Edit and resend earlier message** lists user turns. Picking one
restores its files, removes that turn and later messages, and puts its text
and screenshots in the composer. Edit and press Enter to continue.
**Agent: Fork thread after a turn** keeps the original conversation and
creates a new thread through the selected turn, restoring files changed by
later turns. Both actions validate all affected files before writing.
Restoration requires checkpoints still held in this window and refuses
later manual edits. Queued follow-ups share their running turn's checkpoint;
choose that parent turn when rewinding. Commands' external effects remain.

In Settings, choose **Configure provider**, enter its **Provider model ID**,
and set its key and base URL as needed. OpenAI, Anthropic, OpenRouter,
Ollama and custom servers have separate settings. The request format can be
`responses`, `messages` or `chat`. Configured models join the model picker;
select one as the suggestion model to use it for suggestions and commit
messages too. Empty keys use `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
`OPENROUTER_API_KEY`, `OLLAMA_API_KEY` or `CUSTOM_API_KEY` as appropriate.
Local Ollama and custom endpoints accept an empty key. Token prices are
user-configured dollars per million input, output, cache-read and
cache-write tokens. Set them for cost tracking and spending limits; no cost
is recorded for a provider whose prices remain zero. Actual provider billing
and limits still apply.

Type **@** in the composer, or use **Agent: Attach file, folder or symbol**,
to search attachments. `@src/main.rs` and `@{path with spaces.py}` attach a
file; `@{src/main.rs::main()}` attaches a symbol. The picker inserts the
notation for you. Open tabs contribute their unsaved text. Folders attach up
to 20 text files, with up to 4 KB per file, and list any skipped content.
Explicit attachments work even with IDE context switched off. Attachments
are confined to the project and total at most 64 KB per message.

Save reusable prompts as `.lightdown/prompts/test.md`, `explain.md` or
`review.md`, then send `/test`, `/explain` or `/review`. Type **/** in an
empty composer or use **Agent: Saved project prompts** to list them.
`/test the parser` substitutes `the parser` for `$ARGUMENTS` in the file;
without that placeholder, it appends the arguments to the prompt.

The `web_fetch` and `web_search` tools use the project's separate **Agent may
access the web** permission, ask by default. Plan mode can use them after
that permission is granted. Reads allow only HTTP and HTTPS, cap responses
at 2 MB and 30 seconds, remove HTML tags and scripts, and retain source URLs.
Sources appear under the answer. Pages are data; their instructions are not
trusted. Search uses DuckDuckGo's HTML endpoint without a key. It may rate
limit or show a challenge. Set `LIGHTDOWN_SEARCH_URL` to an HTML search URL
containing `{query}` to use another service. This is text extraction; it
does not run JavaScript or render pages.

**Agent: Ask about terminal output** attaches the selection, or the most
recent output when nothing is selected. The terminal's right-click menu
has the same action. **Agent: Ask about failed task** uses the task log,
and **Agent: Ask about failed PDF build** uses the preview's errors. These
commands append a question to the composer for review before sending.

The agent also has `lsp_diagnostics`, `lsp_definition`, `lsp_references`,
`lsp_hover`, `lsp_document_symbols`, `lsp_workspace_symbols` (by name) and
`lsp_rename`. They use the same server configuration, read unsaved source
tabs and run on a cancellable worker without changing the visible file.
When the editor's server for the project is running and holds the same text,
a query goes to it, so a large project needs no second server and no second
indexing pass; its replies are routed apart from the editor's own requests.
Otherwise the query starts its own server and stops it afterward. Position
arguments start at 1 and count Unicode characters; results keep LSP
zero-based UTF-16 ranges. A missing server or a timeout reports an error,
after 30 seconds on the running server or 20 seconds on a server of its own.
Diagnostics come from pull reports, or from the server's published report on
that text. Output is truncated to 38,000 characters.

`lsp_rename` needs edit permission. The server's edit is checked as a whole
before anything changes: every target must be editable source inside the
project, the requested file must still hold the text the server saw, and
other files must have no unsaved changes. Then each file is checkpointed and
changed as `edit_file` changes it: open tabs in one undo step, saved again if
they were saved, and other files on disk. File operations are refused.

## User commands

Add `.lightdown/commands.json` to run project commands from the palette:

```json
[
  {"name": "Sort selection", "command": "printf '%s' \"$SELECTION\" | sort", "output": "replace", "key": "ctrl+alt+k"},
  {"name": "File info", "command": "wc -l -- \"$FILE\"", "output": "show"}
]
```

**User: List project commands** lists them, and each appears as **User: name**
in the palette. `key`, or an array `keys`, binds Ctrl, Alt or function-key
shortcuts in the editor, taking priority over its usual binding. The shell
gets `$FILE` as the absolute filename or empty for untitled text, `$LINE` as
the cursor's one-based line, and `$SELECTION` as literal selected text. Quote
these variables in shell commands. Project `.env` values are loaded too.

`output` is `replace`, `insert` after the selection, or `show` in a read-only
tab. It defaults to `show`. Insert and replace require a text document and
make one unsaved, undoable edit. If the buffer or selection changes while a
command runs, output opens for review instead. Failures also open their log.
Commands combine stdout and stderr, retain final newlines, time out after
120 seconds and refuse outputs above 30 KB. **User: Stop command** cancels the job.
Configuration is read when listing or running commands; malformed files are
reported when a named command runs.

## Project environments and watch mode

**Python: Select project interpreter** lists system Python, project `.venv`
and `venv` environments, matching cached Poetry environments and registered
conda environments. uv projects normally appear as `.venv`. A custom path
can still be set in the Python interpreter setting. The project choice
wins over that global setting. Switching requires idle cells and restarts
the project's notebook kernels and completion helpers, including inactive
tabs.

Tasks, agent commands, notebook kernels, language servers and newly started
terminal shells load the project's `.env`. Values are literal, with optional quotes, an
`export` prefix and comments after unquoted values. Shell expressions and
variable substitutions are not evaluated. Existing session values win.
Restart a kernel or terminal shell to reload its environment after changing
`.env`. Python runs in the terminal use the chosen interpreter; tasks and
agent commands also put its executable directory first on PATH.

Projects can also take their environment from direnv, mise or Nix. Loading
it runs the project's own code, so it is off until you run **Environment:
Use direnv/mise/Nix for this project** (run it again to turn it off). The
choice is kept per project folder in `~/.local/share/lightdown/environments.json`,
never in the project. Lightdown then uses the first of these that the
project has and whose tool is installed:

- an `.envrc`, with `direnv export json`; direnv still needs `direnv allow`
- `mise.toml`, `.mise.toml` or `.tool-versions`, with `mise env --json`
- `flake.nix` or `shell.nix`, with `nix print-dev-env --json`; Nix's own
  `PATH` goes in front of yours, and its build-sandbox variables such as
  `HOME` and `TMPDIR` are left out

The loader runs in the background, for up to 60 seconds (300 for Nix), and
its result is kept. It applies to tasks, run configurations, tests, newly
started terminals, notebook kernels, debuggers, language servers, MCP servers
and agent commands, underneath the literal `.env`. A value in `.env` wins over the
loaded one. It loads again when its source files change (`.envrc`, the mise
files, `flake.nix`, `flake.lock` or `shell.nix`), or on **Environment:
Reload project environment**. A status-bar item names the tool, says when it
is loading, and turns red when it failed; resting on it shows the tool's
message and a click loads it again. Programs already running, and those
started while the first load is still under way, keep the environment they
started with.

After running `cargo check`, `cargo test`, pytest or a tasks.json task with
`watch` globs, **Task: Toggle watch on save for tests or cargo check**
watches that task. With no earlier matching task, it chooses `cargo check`
for Cargo projects or pytest for Python projects. Those built-in watches
rerun on any save; tasks.json watches rerun on saves matching their globs.
Saves debounce and queue one rerun while the current job runs. The status
bar shows the watch result in green or red. **Task: Stop** cancels the jobs
and every watch. Watching pauses while another project is active.

**Notebook: Variable explorer** lists variable names, types, collection
lengths and shapes for supported array libraries after a cell finishes.
Cell headers show elapsed seconds, and their output includes the last run's
local date and time. Saving the notebook keeps duration and run time in
`metadata.lightdown`. Restarting the kernel clears the variable list.

## Terminal

Tasks and run configurations each open a terminal tab named after the job.
The tab stays open after exit and prints the exit status. Close it with the
normal terminal controls; closing a running job's tab ends the job. Task logs
remain available through **Task: Show output**. All jobs use terminals;
there is no per-task presentation setting. The agent's terminal tools cannot
read or stop job tabs.

Ctrl+` (or Ctrl+J) opens a terminal under the editor, running your shell
(`$SHELL`, or `sh`) in the project's folder, and goes back and forth between
it and the document. The terminal toggle in the title bar opens and hides it;
the shell keeps running while it is hidden. Drag the hairline at its top to
make it taller or shorter. Lines are wrapped anew when the panel gets wider
or narrower.

A toolbar floats at the panel's top right: + opens another terminal in the
project (Ctrl+Shift+T), ◫ shows two side by side (Ctrl+Shift+J), ⟲ starts
the shell afresh, the bin closes the terminal and × hides the panel. With
more than one open, a list at the panel's right names each by its number
and the shell's folder. Click one to go to it, its × or a middle-click to
close it. Closing a terminal where a program is at work asks first. When a
shell exits, it closes; the panel closes with the last one.

Split, a click in either terminal gives it the keyboard. Going to another
terminal, or opening a new one, puts it in the place of the one with the
keyboard.

**Terminal: Run selected text** sends the editor selection, or current line,
to the active terminal and presses Enter. It opens a terminal if needed and
uses bracketed paste when the program enables it. This explicitly executes
text in the active program, so select the intended terminal first.
**Python: Run selection in kernel** runs the selection or current line in the
same persistent project kernel as percent cells. **Python: Show cell output**
opens its log. Neither command saves or changes the source buffer.

Ctrl+click opens what a program printed: a web address in the browser, or a
file in a tab. A file named with a line and column, as compilers print them
(`src/main.rs:12:5`), or the way Python's tracebacks name them
(`File "app.py", line 40`), opens at that line. Relative paths are taken from
the shell's folder.

It is a full terminal. It tells programs it is an xterm with 256 colours, so
editors, pagers and other full-screen programs work in it, and those that use
the mouse (vim with `mouse=a`, htop) get the clicks and the wheel.
The first sixteen ANSI colours follow the light or dark palette. Extended
256-colour and truecolour output keep their hues, with text adjusted when
needed to stay readable against its background.

While the terminal has the keyboard, keys go to the program running in it,
Ctrl+C and Esc included. The window keeps Ctrl+Shift with a key, and the
terminal's own shortcuts:

| Keys | Action |
|:-----|:-------|
| Ctrl+` / Ctrl+J | Back to the document, and to the terminal again |
| Ctrl+Alt+` | Hide the terminal |
| Ctrl+Shift+T | Open another terminal |
| Ctrl+Shift+W | Close this terminal |
| Ctrl+Shift+PgDn / Ctrl+Shift+PgUp | Next / previous terminal |
| Ctrl+Shift+J | Two terminals side by side, or one |
| Ctrl+click | Open a printed file or web address |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy the selection / paste |
| Shift+PgUp, Shift+PgDn, Shift+Home, Shift+End | Scroll through the history |
| Ctrl+Shift+Up / Ctrl+Shift+Down | Previous / next command's prompt |

Drag to select text, double-click for a word and triple-click for a line;
Shift selects even where a program takes the mouse. The wheel scrolls back
through the last 5000 lines, or moves through a full-screen program such as
`less`. Files dropped on the terminal are typed in as quoted paths. Right-click
for Copy, Paste, Clear and the rest. *Open a terminal here* in the folder
view's menu goes to that folder, in a new terminal if a program is at work
in the one shown, and *Run in the terminal* in a Python file's
menu saves the file and runs it with the project's interpreter.

### Shell integration

When the terminal starts bash, zsh or fish, the shell marks where each
prompt, command line and command output begins, and how each command ended
(the OSC 133 marks), and says which folder it is in (OSC 7). A small dot
beside each prompt whose command has finished is green for exit status 0
and red otherwise; resting the pointer on it shows the status. The marks
stay with their lines as they scroll into the history and wrap anew.
**Terminal: Previous command** and **Terminal: Next command** (Ctrl+Shift+Up
and Ctrl+Shift+Down in the terminal) scroll from prompt to prompt. **Terminal:
Copy last command output** and **Terminal: Select last command output** take
the output of the last command that has finished. **Terminal: Rerun last
command** types the last command line again and runs it, when the shell is
waiting at its prompt. The folder the shell reports is the one that new split
terminals start in, that the terminal list names, that saved sessions reopen
and that relative paths in its output are opened from.

Your startup files are never changed. What each shell gets:

- bash is started with `--rcfile` pointing at a file of Lightdown's that
  reads `~/.bashrc` and then adds a function to the start and end of
  `PROMPT_COMMAND`, the command mark to `PS1` and the output mark to `PS0`
  (bash 4.4 or newer). The command line is taken from the screen.
- zsh is started with `ZDOTDIR` pointing at a folder of Lightdown's whose
  `.zshenv` puts your `ZDOTDIR` back, reads your `.zshenv`, and adds
  `precmd` and `preexec` hooks; zsh then reads your `.zshrc` and the rest as
  usual. A `ZDOTDIR` set in `/etc/zsh/zshenv` bypasses this.
- fish 4 marks its prompts itself. fish 3 is given `--init-command` that
  sources a file of Lightdown's adding event handlers.

The files are written to `$XDG_RUNTIME_DIR/lightdown/shell`, or to
`~/.cache/lightdown/shell`, readable only by you. Terminals of an SSH project
start the remote shell as before, without hooks. Turn **Shell integration**
off in the settings for shells started from then on.

Other shells can print the marks themselves: `ESC ] 133 ; A BEL` as the
prompt starts, `ESC ] 133 ; B BEL` at its end, `ESC ] 133 ; C BEL` before a
command runs, `ESC ] 133 ; D ; status BEL` once it has ended, and
`ESC ] 7 ; file://host/path BEL` with the folder, its spaces as `%20`.
VS Code's `ESC ] 633 ; E ; command line BEL` and fish's `cmdline_url=` give
the command line for rerunning.

The agent has `start_terminal`, `read_terminal` and `stop_terminal` tools
for dev servers and watchers. Starts use the command permission and appear
in ordinary terminal tabs immediately. The returned ID stays valid when tabs
move or close; a thread can read and stop only its own command terminals.
**Agent: Show running command terminals** focuses one. The user can interrupt
or close it with the normal terminal controls. Up to eight can run. They
continue after a reply ends or Stop cancels the model; stop them explicitly.
Output retains the terminal's usual 5000-line history, with 20 KB per read.
Checkpoints capture changes during the initiating turn; later asynchronous
writes need normal git review and are not guaranteed by that checkpoint.

For an SSH project, keep a local mirror of the folder and create
`.lightdown/remote.json` there:

```json
{"host": "user@build-machine", "path": "/srv/project"}
```

**Remote: Connect project over SSH** checks key-based login and enables remote
execution in this window. An SSH label in the status bar opens the remote
commands. Newly opened terminals, project tasks, tests, language servers and
agent shell commands then run in that remote folder. Language-server file
URIs map between the mirror and remote folder, including workspace edits.
The host uses its installed tools. Existing terminals keep their original
shell. **Remote: Disconnect project** returns new execution to this machine.
No connection or sync starts automatically. Configure ports, identity files
and host aliases in `~/.ssh/config`; host keys must already be trusted.
Password prompts and accepting unknown host keys are disabled.

**Remote: Pull files into local mirror** and **Remote: Push saved files to
remote** copy files with `rsync` over SSH. Save buffers first. These explicit
commands overwrite destination files of the same name, keep extra files,
and exclude git metadata, the remote profile, build output and environments.
Pulls take a local source checkpoint for review and rollback. **Remote: Stop
connection or sync** stops an in-progress operation. SSH and rsync must be
installed locally; the remote host needs a shell, rsync and `timeout`.

This is a mirror workflow, not a remote filesystem. File editing and git
remain local. Push before remote builds; pull after remote commands change
files. Remote writes and pushes have no remote checkpoint. Notebook kernels,
Python completion, PDF builds and MCP processes remain local. Local `.env`
values and interpreter paths are not translated to remote equivalents; use
remote-relative task commands and set up the remote environment separately.
Remote cancellation closes the SSH session; detached remote processes may
outlive it. Remote file browsing is not implemented.

`"pushOnSave": true` in `.lightdown/remote.json` turns on push on save for
the connection, and **Remote: Toggle push on save** turns it on or off for
this window. Each save then queues that file, and 500 ms after the last
save one `rsync --relative` copies just the queued files to the same places
in the remote folder. Git metadata and the remote profile are never pushed.
Failures show in the status bar. Pull remains explicit.

**Remote: Forward port…** offers ports printed by remote terminals and tasks
(see Ports below) and runs a managed `ssh -N -L 127.0.0.1:local:127.0.0.1:port
host`. The local port is the same as the remote one when it is free, and
any free port otherwise. Forwards appear in **Ports: Show detected ports**,
where selecting one stops it. They also stop on **Remote: Disconnect
project**, when ssh exits (the status bar says why), and when the window
closes. Up to eight run at once.

### Ports

Lines a terminal or job prints are scanned for local server addresses:
`http://localhost:3000`, `http://127.0.0.1:8000/`, `0.0.0.0:5173`,
`[::]:8443`, and phrases such as "listening on port 8080". Wildcard hosts are
shown as `localhost`. **Ports: Show detected ports** lists the last 64 per
window, newest first, with the terminal or task that printed them, plus
forwarded SSH ports and a container's `forwardPorts`. Selecting one offers
**Open in browser** (with `xdg-open`) and **Copy address**; a port seen on
an SSH host offers **Forward to this machine** instead. Nothing is opened or
forwarded automatically.

### Dev containers

A container is an execution target beside SSH. Lightdown reads
`.lightdown/container.json` or, failing that, `.devcontainer/devcontainer.json`.
Comments and trailing commas are allowed. The supported keys:

```jsonc
{
  "image": "python:3.12-slim",          // or "build" below, not both
  // "build": {"dockerfile": "Dockerfile", "context": ".."},
  "workspaceFolder": "/workspaces/app", // default /workspaces/<folder name>
  "containerEnv": {"APP_MODE": "dev"},
  "forwardPorts": [8000],
  "postCreateCommand": "pip install -e .", // a string for sh -c, or an argv array
  "runArgs": ["--network=host"],        // only host networking is used
  "remoteUser": "vscode"                // the user commands run as
}
```

`hostNetwork: true` is a Lightdown shorthand for the host network. Build
paths are relative to the config file and must stay inside the project.
Other keys, such as `features`, `mounts` and `customizations`, are ignored
and listed in the log.

**Container: Start or connect** uses docker, or podman when docker is not
installed. It builds the Dockerfile or pulls the image when missing, then
starts or reuses a container named `lightdown-<folder>-<hash>` and labelled
with the project. The project is bind-mounted at `workspaceFolder`. With
host networking, `forwardPorts` are reachable directly; otherwise each is
published on `127.0.0.1` with the same port. `postCreateCommand` runs once,
when the container is created. A changed config recreates the container on
the next start. Builds may take an hour and pulls 30 minutes before they
time out.

While connected, new terminals (`docker exec -it`), tasks, tests, language
servers and agent shell commands run inside the container in the matching
folder. Language-server file URIs map between the project and
`workspaceFolder` as in SSH mode. Files stay on this machine, so nothing is
synced. A status-bar item names the container and opens the Container
commands. **Container: Stop** stops it and returns execution to this
machine; it also cancels a start in progress. **Container: Rebuild** removes
the container and starts it again, rebuilding a Dockerfile. **Container:
Show log** opens the build, pull, start and `postCreateCommand` output, the
last 256 KB, in a read-only tab. Closing the window stops the container;
the next start reuses it.

A killed `docker exec` client leaves its process running in the container,
so each command carries a `LIGHTDOWN_EXEC` tag. Every two seconds Lightdown
checks which tagged clients still run here and ends the container processes
of those that have gone, such as a stopped job or a closed terminal. The
container image needs `sh`, `timeout`, `tr` and `sed`, which Alpine and
Debian images have. Notebook kernels, Python completion, PDF builds and MCP
processes stay on this machine. Rootless podman maps user IDs differently,
and files created as root in the container are owned by root on the host;
set `remoteUser` to avoid that.

## Spelling

Words misspelt in prose are underlined with a red wave: in markdown and
plain text, the prose of LaTeX outside its commands, mathematics and
comments, and the markdown cells of notebooks. Code, links, addresses,
paths, words in capitals and words with capitals or digits inside them are
left alone, as is the word being typed. Right-click a marked word for what
it may have been meant to be, or *Add to the dictionary*, which keeps it in
hunspell's personal dictionary.

Spelling is checked by `hunspell`, with the dictionary of the session's
language, or the one *Spelling dictionary* in the settings names, such as
`en_GB` or a path to a `.dic` file without its extension. Dictionaries come
in packages such as `hunspell-en_us` on Arch Linux and `hunspell-en-us` on
Ubuntu. Without hunspell or the dictionary nothing is marked, and the
setting says *not found*. *Spell checking* in the settings turns it off.

Grammar and style checks are optional. Start a local LanguageTool HTTP
server yourself and configure `.lightdown/grammar.json`:

```json
{"url": "http://127.0.0.1:8081", "language": "en-US"}
```

**Writing: Check grammar and style** checks the current Markdown, LaTeX or
plain-text buffer. Matches use the spelling wave underline; right-click
for the explanation and replacements. Each fix is one unsaved undo step.
Code and marked non-prose spans are blanked before sending. Checks are
explicit, limited to 256 KB and 15 seconds, and only accept a local HTTP
host and port. Nothing starts or leaves the machine on launch. Editing
hides stale results; run the command again to refresh. **Writing: Clear
grammar and style marks** clears results or stops a check. Notebook grammar
checks and Vale integration are not implemented.

Wayland text-input-v3 lets fcitx5 and IBus compose text in the editor and
agent's message box when the compositor exposes the protocol. Preedit text
appears underlined at the caret, with the input method's selection and caret.
It stays outside the file and undo history until committed. Esc or **Edit:
Cancel input composition** cancels it. Candidate windows follow the caret,
including on scaled displays. Compose and dead keys still work when no input
method is available. IME input in dialogs, the palette and the terminal is
not yet supported; multiple cursors use ordinary keyboard input.

## Mouse

Everything the keys do can be reached with the pointer as well.

- **Rest** on a control that shows only an icon to see its name and keys.
- **Right-click** for a menu of what can be done with what is under the
  pointer, each with its keys: in the document cut, copy, paste, undo,
  formatting, the notebook's cells, find and build; on a link, open or copy
  it; on a PDF, copy, find and zoom; on a picture, zoom; in the folder
  view, the file operations (see [Folder](#folder)); in the agent's conversation and message box, and in the
  terminal. A right-click outside the selection moves the cursor there first.
  On a title bar Lightdown draws itself, it brings up the desktop's window
  menu; in the settings, it steps a choice back.
- **Middle-click** pastes what was last selected with the pointer, here or in
  another program (the primary selection), where you click.
- **Hover** lights up the row of a menu, list, file chooser, completion list
  or the explorer under the pointer, and every button.
- **Scrollbars** can be dragged, and a click on one jumps there. Text
  that does not wrap has one along the bottom as well.
- **Completions** are taken with a click.
- **Resting on a Python name** shows its documentation; see [Python](#python).
- **Files dropped** on the window are opened; on the agent's message box
  their paths are written in.

## Suggestions

Alt+\ (or Ctrl+Shift+Space) asks for the next small edit. A prediction can
insert, replace or delete text in the excerpt around the cursor. Removed
text stays visible in red with a strike; inserted text is dimmed. An edit
away from the cursor shows **Tab to jump** and a green gutter marker.

| Keys | Action |
|:-----|:-------|
| Tab | Jump to a nearby prediction, then accept on the next Tab |
| Ctrl+Right | Take the next word of an insertion at the cursor |
| Esc | Dismiss it |
| Alt+\ | Ask again |

Typing an insertion's matching prefix keeps its remainder visible. Other
input dismisses it. Predictions stay out of saves and undo history until
accepted; accepting a replacement or deletion is one undo step.

Enable **Edit predictions** in Settings or **Suggestion: Toggle edit predictions**
to request while typing. `edit_predictions` is `off` by default or `on`;
the old `auto_suggest` preference migrates when it is loaded. The pause is
`edit_prediction_delay_ms`, 300 by default, configurable from 100 to 5000.
Automatic model requests are at least one second apart. Superseded requests
are canceled, identical buffer states use a small in-memory cache, and
idle editing does not repeat requests. Navigation, dialogs, other tabs and
agent work cancel pending requests. Model requests include the last six
edits with file and position, the cursor excerpt and up to three related
open files. Replies contain an edited excerpt which Lightdown diffs locally.
Notebooks keep cursor insertions. Model requests obey the spending limits.

After replacing one occurrence, **Suggestion: Suggest next matching edit**
previews the same replacement at the next occurrence. Tab accepts, Esc
restores it. Whole-word replacements skip longer names. This uses the last
undoable replacement locally, sends no request, and works in source files;
notebook cells use ordinary suggestions.

Choose `edit_prediction_provider: model|copilot` in Settings. **Copilot: Sign in**
asks before installing `@github/copilot-language-server` with npm into
`~/.local/share/lightdown/servers/copilot`. Node.js and npm must be installed.
The device code appears in the status bar, is copied to the clipboard, and
`xdg-open` opens GitHub's verification page. Finish sign-in there.
**Copilot: Sign out** signs out through the server. Copilot's status stays
in the status bar. Its server stores credentials; Lightdown never puts
GitHub tokens in settings. Copilot receives open document synchronization
and requests `textDocument/inlineCompletion`. These requests use your
Copilot account, while Lightdown's model predictions use its spending limits.
The device-flow protocol follows [GitHub's language-server documentation](https://github.com/github/copilot-language-server-release#authentication).

Suggestions come from their own model, separate from the agent's, so they
can be quick. The default is DeepSeek V4.1 Flash on OpenCode Go with
reasoning off unless the settings say otherwise. They count toward the spending limits.

## Keybinds

F1 lists shortcuts and palette commands with their contexts and keys; so
does the *Keybinds* row in Settings. Type to filter. Enter records a new
binding: press each stroke, then Enter to save. Tab cycles its context.
Esc cancels; Backspace removes the last stroke, or unbinds when the
recording is empty. Delete restores the base binding. Potential conflicts,
including shared chord prefixes, appear before saving. A more specific
context can intentionally reuse another context's keys.

**Preferences: Open keymap file** opens
`~/.config/lightdown/keymap.json`. It uses Zed's array shape, with stable
command ids from the keybind list. For example:

```json
[
  {
    "context": "Editor && mode == source",
    "bindings": {
      "ctrl-k ctrl-s": "save",
      "ctrl-k ctrl-o": "keymap_open",
      "ctrl-alt-s": null
    }
  },
  {
    "context": "Explorer",
    "bindings": {"r": "rename"}
  }
]
```

Spaces separate strokes; `ctrl-k ctrl-s` means Ctrl+K followed by Ctrl+S.
Sequences may have more than two strokes. Legacy `ctrl+k` spelling and
Emacs `C-x` / `M-x` spelling also work. The status bar shows a pending
sequence and its possible completions. Esc or an unmatched key cancels
and consumes that key; no text is inserted. Changing focus also cancels
the pending sequence. There is no chord timeout.

Contexts include `Editor`, `Terminal`, `Palette`, `Explorer`, `Notebook`,
`AgentComposer`, `Dialog`, `SourceControl`, `SearchPanel` and `Panel`.
`Editor && mode == source` selects source editing; `mode == markdown`
selects live markdown. Modal contexts use `vim_mode == normal`, `insert`
or `visual`; `modal_mode` describes the active modal editor independently
of its kind. `helix_mode` uses the same values when Helix is active.
`Main` means no dialog or palette; `Document` preserves the legacy editor
scope. An omitted context is global. Expressions support `&&`, `||`, `!`,
`==`, `!=` and parentheses. Matching predicates determine specificity;
the most specific binding wins, then later blocks win at the same
specificity. `null` consumes a sequence without running a command.

A plain first stroke is allowed when focus does not accept text, such as
the explorer, a panel's result list, or modal normal and visual modes.
Typing fields keep their text keys. Later strokes of a chord may be plain
keys. The recorder reserves Enter, Tab, Esc and Backspace for its controls;
use the file to bind these keys explicitly in non-typing contexts.
Input handling such as Tab and
Enter remains built in unless a matching contextual binding overrides it.

The `base_keymap` setting chooses `lightdown`, `vscode`, `jetbrains`,
`sublime` or `emacs`. Settings also has a *Base keymap* row. Each alternative
starts with Lightdown's command set and replaces familiar shortcuts for
navigation, editing and panels. The default `lightdown` map preserves all
the keys in the tables below. The legacy `keys` setting remains readable,
as in `"find": ["ctrl+g"]`, and accepts chord sequences too. It overrides
the base; `keymap.json` applies last at equal specificity. The keybind
editor writes only `keymap.json` and preserves unrelated contexts.

| Emacs keys | Command |
| --- | --- |
| C-x C-s, C-x C-f, C-x C-w | Save, open, save as |
| C-x b, C-x k | Choose file, close tab |
| C-x 2, C-x 3 | Split below, split right |
| M-x, C-g | Command palette, cancel |
| C-a, C-e, C-f, C-b, C-n, C-p | Line start/end, right/left/down/up |
| M-f, M-b | Next/previous word |
| C-space | Set or clear selection mark |
| C-w, M-w, C-y | Cut, copy, paste |
| C-s, C-/ | Find, undo |

Saving the keymap in Lightdown reloads it immediately. External saves are
noticed while the window is active. Invalid JSON, contexts, key sequences
or command ids report an error and keep the last valid map.
**Preferences: Reload keymap file** reloads it explicitly.

## Claude Code and Codex

Claude Code and OpenAI Codex can be the agent in the sidebar, beside the
built-in one. They run as programs on this computer under your own sign-in
(`claude` and `codex login`), so they cost your subscription, not an API key.
When `claude` or `codex` is on PATH, the model list (Ctrl+M) shows their
models with **Claude Code** or **Codex** beside them, and the reasoning list
(Ctrl+R) their efforts. Lightdown asks each program for its models the first
time the list opens in a run and remembers them.

| Agent | Started as | Permissions | Plan mode |
| --- | --- | --- | --- |
| Claude Code | `claude` with the stream-JSON protocol of the Claude Agent SDK | its permission requests | `--permission-mode plan` |
| Codex | `codex app-server` | its approval requests | read-only sandbox |

A thread belongs to the agent it began with. Picking a model of another
agent starts a new thread; opening a saved thread picks its agent's model
again. The thread keeps the agent's own session or thread id, and the next
message resumes it, also after a restart.

The transcript shows the agent's text, its summarised reasoning, the tools
it runs and their results as for the built-in agent. **Stop** (Esc)
interrupts Claude Code and cancels Codex's turn. The IDE context (@ toggle,
Ctrl+K), `@` attachments and pasted screenshots go with the message.
Follow-ups typed during a turn are sent when it ends.

Permission requests follow the project's settings for editing, running
commands and web access: **Allow** answers yes, **Deny** answers no, and
**Ask** shows the approval card (**Allow once**, **Always allow**, **Deny**).
For Claude Code, editing set to Allow starts it in `acceptEdits` mode, and
a denied permission removes its tools (`Edit`, `MultiEdit`, `Write`,
`NotebookEdit`; `Bash`; `WebFetch`, `WebSearch`). Codex works in a sandbox
that may write inside the project and asks for anything outside it, or for
every command not known to be safe unless both editing and commands are
allowed. Denied editing gives Codex a read-only sandbox, and denied web
access turns off its web search.

The agents write files themselves. Lightdown takes a checkpoint of the
project's source files before each turn and compares after each tool and at
the end of the turn, so **Agent: Review changed files**, hunk review and
rollback work as for the built-in agent. Open tabs whose files changed on
disk reload. The usage line shows how full the context is and, for Claude
Code, what the turn would cost at API prices. These costs are not counted
against the spending limits. The agent runs in the window's project folder,
which is the worktree in a worktree window.

## Settings

`minimap` accepts `off`, `on` or `auto`, and defaults to `auto`. Auto shows the overview when
a source file is longer than the view. `minimap_width` is 72 logical pixels
by default, from 32 to 160. Both are in Settings. **View: Toggle minimap**
switches it from the palette. Each source view, including markdown edit
mode, has its own overview. Click to jump, drag the shaded viewport to
scroll, or use the wheel. Edge lanes mark git changes, diagnostics, search
matches and the cursor. Narrow panes hide it. Large files sample source
lines and dense marker lists; the cached overview fills in over a few frames.

Ctrl+, opens the settings, in groups, each with what it does written under
it; type to narrow them, Enter or a click changes one. They are: theme
(dark, light or system), reading font (Inter or Libron), font size, modal editing (`none`, `vim`, `helix`),
base keymap, keybinds, autocomplete,
spell checking and its dictionary, whether to draw mathematics, the Python
interpreter, the TeX engine, the API key, the default model and reasoning,
whether to send IDE context and show the model's reasoning, whether to run
the project check after agent edits, the model and
reasoning for suggestions, edit predictions (`edit_predictions`: `off|on`),
their provider (`edit_prediction_provider`: `model|copilot`) and pause
(`edit_prediction_delay_ms`: 100 to 5000, default 300), the spending limits, and the current project's
permissions. They are kept in
`~/.config/lightdown/settings.json`, readable only by you; threads and the
record of spending are under `~/.local/share/lightdown`.

The global `lsp_auto_install` setting accepts `ask`, `always` or `never`.
Project `language_servers` entries configure command, args,
initialization_options and settings by language ID.

A project's `.lightdown/settings.json` overrides these defaults without
changing global preferences. For example:

```json
{
  "font_size": 18,
  "ruler": 100,
  "format_on_save": true,
  "formatters": {"py": "ruff format -", "rs": "rustfmt --emit stdout"},
  "spell_dictionary": "en_GB",
  "latex_engine": "tectonic",
  "model": "go/deepseek-v4.1-flash"
}
```

Omitted values inherit global preferences; formatter entries merge by
extension, and an empty command disables one. Font size is 8 to 40; the
ruler is 0, 80 or 100. The model is the default until one is chosen explicitly
for that project. Settings load when the project first opens in a window.
**Preferences: Reload project settings** applies file edits or removal;
invalid JSON or values report an error and keep the last valid settings.
The Settings dialog continues to edit global defaults. Formatter commands
run when you save with formatting enabled, so review settings in unfamiliar
projects before enabling it.

These are kept in `settings.json` only, and take effect when the file is
saved:

| Key | What it does |
|---|---|
| `theme` | The theme by name; `dark` and `light` are the built-in pair. Used while `theme_mode` is empty |
| `theme_mode` | Empty, `system` (follow the desktop's `org.freedesktop.appearance color-scheme`, asked through `busctl` or `gdbus`), `dark` or `light` |
| `dark_theme`, `light_theme` | The themes the mode picks between |
| `theme_overrides` | Roles to colours over every theme, as `{"bg": "#000000"}`, or under a theme's name for that theme only, as `{"Nord": {"keyword": "#88c0d0"}}` |
| `icon_theme` | `Lightdown` (empty), `Lightdown Lines` or an icon theme's name |
| `buffer_font_family` | Code's font family; empty for the system's monospace |
| `buffer_font_size` | Code's size in pixels, 6 to 60; 0 follows `font_size` |
| `buffer_line_height` | Rows of code as a multiple of its size, 1 to 3; 1.5 by default |
| `buffer_font_features` | OpenType features of code by tag, `true`, `false` or a number |
| `ui_font_family` | The interface's family; empty for Inter |

### Config files

Theme files (`themes/*.json`, in `~/.config/lightdown` and a project's
`.lightdown`) and icon themes (`~/.config/lightdown/icon-themes/*/icon-theme.json`)
are described under Themes, icons and fonts; they are read when they
change but not checked as they are typed.

Lightdown knows the shape of its own JSON config files: `settings.json` and
`keymap.json` and `snippets.json` in `~/.config/lightdown`, and `settings.json`, `lsp.json`,
`run.json`, `debug.json`, `tasks.json`, `commands.json`, `snippets.json`,
`mcp.json`, `remote.json`, `container.json` and `grammar.json` in a
project's `.lightdown` folder, and `*.lightdown-workspace` files anywhere. `.devcontainer/devcontainer.json` belongs to
the Dev Containers format, of which Lightdown reads a part, so it is not
checked. While one is open:

- Typing offers the keys allowed where the cursor is, with what each does,
  leaving out keys already written; after a colon it offers the values an
  enum or a switch allows, such as `ruler` 0, 80 or 100, `transport` stdio
  or tcp, `output` replace, insert or show, and the shortcut ids under
  `keys`. Ctrl+Space asks for them anywhere.
- Hover or Ctrl+K on a key shows its description, what it takes and its
  default.
- Mistakes are underlined and listed in Problems: JSON syntax errors, wrong
  types, numbers out of range, values outside an enum, missing required
  keys and malformed values such as a remote path or a command key, in red;
  unknown and repeated keys, in amber. The user's `settings.json` is
  forgiving when it loads, ignoring or clamping bad values, so its marks are
  all amber.

The checks follow what each file's loader accepts, only read the text, and
skip files over 256 KB. **Preferences: Create project config file** lists
the `.lightdown` files that do not exist yet, writes the chosen one with
the example from this README, and opens it. It never overwrites a file.

To give a new config file a schema, add its schema file to
`src/config_schema/schemas/`, as `{"description", "schema", "example"}`, and
one line to `FILES` in `src/config_schema/mod.rs`, naming the file and
whether it lives in the user's config folder or the project's. A new key is
one entry in that schema's `properties`, with a `type` and a
`description`. The supported subset of JSON Schema and its few extensions
are listed at the top of `src/config_schema/mod.rs`; the example is checked
against its schema by the tests.

## Install

Lightdown needs `libxkbcommon` at run time, which every Wayland desktop
already has. Notebooks and completion need `python3`; the agent needs
`curl`; the trash needs `gio` (part of GLib) or `trash-put`; spell checking
needs `hunspell` and a dictionary; Git integration needs `git` and the
coreutils `timeout` command. Building PDFs needs a TeX engine, which Lightdown can set up itself with `curl` and
`tar`; showing them needs nothing more. Building Lightdown needs Rust 1.89
or newer, a C compiler and linker (tree-sitter's runtime is C) and the
libxkbcommon headers.

```sh
# Arch Linux
sudo pacman -S --needed rust libxkbcommon

# Ubuntu (the packaged Rust is too old; use rustup)
sudo apt install build-essential pkg-config libxkbcommon-dev curl
curl -sSf https://sh.rustup.rs | sh
```

```sh
make install     # binary, launcher entry and icon into ~/.local
make default     # optional: open .md files with Lightdown by default
```

After `make install`, Lightdown appears under *Open With* in file managers
for markdown, LaTeX, text and PDF files. To install system-wide instead, run
`sudo make install PREFIX=/usr/local`. `make uninstall` removes everything.

## Use

```sh
lightdown notes.md other.py
lightdown src/main.rs:12:5
lightdown --wait .git/COMMIT_EDITMSG
lightdown ~/notes    # a folder, in the sidebar
lightdown app lib    # two folders in one window
lightdown my.lightdown-workspace
```

Each file opens in its own pinned tab. Line and column numbers start at one;
columns count Unicode characters. An existing filename containing colons is
opened literally. `--` ends options for names beginning with `-`.

Set `EDITOR="lightdown --wait"` or `git config core.editor 'lightdown --wait'`
to use it for editor commands. The command waits for its own window to close.
Unsaved edits ask before closing; discarding them returns failure to the
caller. These windows do not restore or checkpoint unrelated recovery tabs.
Several folder arguments open as one window of several folders; see
[Several folders in one window](#several-folders-in-one-window).

| Keys | Action |
|:-----|:-------|
| Ctrl+P | Go to a file: see [Command palette](#command-palette) |
| Ctrl+Shift+P | Run a command |
| Ctrl+G / Ctrl+R | Go to a line / a heading or definition |
| Ctrl+E | Toggle Edit mode |
| Ctrl+S / Ctrl+Shift+S | Save / save as |
| Ctrl+O / Ctrl+N | Open a file / new window |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous file tab |
| Ctrl+W / Ctrl+Alt+Shift+T | Close / reopen a file tab |
| Alt+F | Everything else to do with the file: see [Files](#files) |
| F5 | Build and preview markdown or LaTeX: see [LaTeX and PDF](#latex-and-pdf) |
| Ctrl+F | Find; Enter or F3 for next, Shift+Enter or Shift+F3 for previous |
| Ctrl+H | Find and replace: see [Finding and replacing](#finding-and-replacing) |
| Alt+L | Find in selection, with the find bar open |
| Ctrl+Shift+F | Search the project's files in the Search tab |
| Ctrl+Shift+H | Show the outline: see [Outline](#outline) |
| Ctrl+Alt+J | Show the line in the PDF, for LaTeX or markdown |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+B / Ctrl+I | Bold / italic |
| Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A | Copy, cut, paste, select all |
| Tab / Shift+Tab | Indent / outdent list items and selections |
| Ctrl+Plus, Ctrl+Minus, Ctrl+0, Ctrl+scroll | Zoom |
| Alt+Z | Toggle word wrap for this kind of file |
| Ctrl+click | Follow a link |
| Ctrl+Space | Complete the name at the cursor (Python or language server) |
| Ctrl+K | Documentation of the name at the cursor (Python or language server) |
| Alt+\ / Ctrl+Shift+Space | Ask a model for a suggestion at the cursor |
| Ctrl+/ | Toggle comments using the file's language |
| Alt+Up / Alt+Down | Move selected lines up / down |
| Ctrl+Alt+D / Ctrl+Shift+K | Duplicate / delete selected lines |
| Ctrl+Alt+B / Ctrl+Alt+L | Jump to the matching bracket / select all occurrences |
| Alt+Left / Alt+Right | Previous / next jump location |
| F12 / Ctrl+F12 / Ctrl+Shift+F12 | Go to definition / implementation / type definition |
| Alt+F12 / Alt+Shift+F12 | Peek definition / references under the line |
| Ctrl+Alt+K / Ctrl+Alt+] / Ctrl+Alt+[ | Toggle / next / previous bookmark |
| F6 | Project tasks |
| F8 / Shift+F8 | Next / previous problem |
| Alt+F8 / Alt+Shift+F8 | Next / previous change against Git |
| Ctrl+L / Ctrl+Shift+L | Go to the agent and back / hide the side panels |
| Ctrl+Shift+E | Go to the folder view and back: see [Folder](#folder) |
| Ctrl+Shift+G | Go to Source Control and back: see [Git](#git) |
| Ctrl+Shift+Y | Show the Tests tab: see [Tests](#tests) |
| Ctrl+` / Ctrl+J | Go to the terminal and back: see [Terminal](#terminal) |
| Ctrl+, | Settings |
| F1 | Keybinds |
| Ctrl+Q | Quit |

In a notebook:

| Keys | Action |
|:-----|:-------|
| Shift+Enter | Run the cell and go to the next |
| Ctrl+Enter / Alt+Enter | Run the cell / run it and add a cell below |
| Alt+Enter (multibuffer) | Open the file at the cursor's line |
| Alt+PageUp / Alt+PageDown (multibuffer) | Expand the excerpt up / down |
| Ctrl+Shift+Enter | Run all cells |
| Ctrl+Shift+A / Ctrl+Shift+B | Add a cell above / below |
| Ctrl+Shift+M | Switch the cell between code and markdown |
| Ctrl+Shift+D | Delete the cell |
| Ctrl+. | Interrupt the running cell |
| Ctrl+Shift+R | Restart the kernel |
| Ctrl+Shift+O | Clear all outputs |

Enter continues the current list or quote, and ends it on an empty item.
Clicking a rendered checkbox ticks it. Double-click selects a word,
triple-click a line. Prompts for a file's name complete paths with Tab.

## Design

The goals are a fast start, a small binary and few dependencies, without
rewriting what good libraries already do.

- **Window and input.** [smithay-client-toolkit] speaks the Wayland
  protocol directly. There is no GUI toolkit and no GPU. Frames are drawn
  in software into shared memory, which is why startup is quick.
- **Text.** [swash] shapes and rasterizes. Lines are laid out only when
  they scroll into view.
- **Markdown.** [pulldown-cmark] parses it, with tables, task lists,
  strikethrough, footnotes, front matter, math spans and wikilinks.
- **Images.** The small zune-jpeg, zune-png, image-webp and gif decoders
  are used rather than a general imaging library, and [resvg] draws SVG. Sizes come
  from file headers when a line is laid out; pixels are decoded on a worker
  thread, so pictures never delay the first frame. A GIF's frames are all
  decoded, up to 256 MB, and the window is drawn again only when a frame
  of one in view is due. The system's fonts are read for SVG text when the
  first SVG is drawn, not at launch.
- **Panes.** The document, the conversation and the message box are the
  same text pane three times over, so the sidebar renders markdown, scrolls
  and selects with the code the editor already had. A notebook and a
  conversation are both one text with a header line per cell.
- **Python.** The editor does highlighting and the first completions.
  Running cells and the rest of completion go to one small script,
  embedded in the binary and run by the system's `python3`, which speaks
  JSON over pipes. There is no ZeroMQ and no Jupyter to install.
- **TeX.** Lightdown holds no TeX of its own. It runs the engine there is,
  on the background job the agent's commands use, and reads its errors from
  what it prints. Markdown becomes LaTeX from pulldown-cmark's events.
- **PDF.** [hayro], a PDF renderer in Rust that needs no GPU and no C
  library, reads the pages' sizes when a PDF is opened and draws each page
  on the image worker, at the size it is shown, when it scrolls into view.
  Pages are cached with the pictures and are named by the file's time of
  writing, so a PDF built again is never shown from an older drawing. The
  same interpreter, given a device that only notes where each glyph lands
  and what character it stands for, yields the text to select and search.
- **Agent.** Requests go out through `curl`, which every system has and
  which brings TLS without adding a megabyte of it to the binary. The API
  key is passed on curl's standard input, never on its command line.
  [serde_json] is the one library added for it.
- **Terminal.** The shell runs on a pseudo-terminal opened with the C
  library's own calls. [vte] splits what it writes into text and escape
  sequences, and Lightdown keeps the screen itself, on the thread that reads
  them, so a flood of output never holds up the window.
- **Fonts.** [Inter] for the interface and prose, and [Libron] for serif
  reading, are embedded in the binary, so nothing has to be installed.
  Inter is cut down to Latin, Greek and common symbols.
  Monospace text and characters they lack (CJK, emoji) use system fonts
  found through `fc-match`. Emoji keep their colours from COLR outlines or embedded CBDT/PNG strikes.
  Colour fonts load on first use, using the same fontconfig fallback as other
  missing characters; a system colour emoji font is needed.
- **Icons.** They are drawn in the manner of Lucide, as T3 Code's are, from a few
  lines of SVG each, rasterized once per size by the SVG renderer the
  binary has anyway.

The release binary is about 15 MB, of which 0.9 MB is the four Libron faces,
0.5 MB the five Inter faces,
0.4 MB the image decoders, 2.1 MB the SVG renderer with its text shaping and
4.5 MB the PDF renderer with its fonts and CJK character maps, and links only libc and libxkbcommon. Set `LIGHTDOWN_TIMING=1` to print the
time from launch to the first frame.

## Development

```sh
make test
# Render one frame to an image without a compositor, after replaying keys:
cargo run --features dump -- --dump out.ppm 900x1000 "down*3,ctrl+e" notes.md
```

The key script also takes `type:text`, `click:x:y`, `press:x:y` (held, for
drags), `rclick:x:y` (the right button), `move:x:y`, `scroll:n` and `wait`,
which waits for running cells, for the agent and for builds.

Set `LIGHTDOWN_DAP_TRACE` to a file path to append every Debug Adapter
Protocol message to it: `->` for what Lightdown sends, `<-` for what the
adapter sends. The real-adapter debugger tests are opt-in: set
`LIGHTDOWN_TEST_JS_DEBUG` to js-debug's `src/dapDebugServer.js` and run
`cargo test -- --ignored node_debug_tests` (the TypeScript one needs `tsc`).

## Limits

- Vim macros replay document keys only; window commands and popup interaction
  inside a recording are limited. Helix implements the selection-first subset
  documented above, with shared yank text rather than per-selection registers.
- Alternative base keymaps cover common Lightdown commands, rather than
  every command of their source editors. Emacs has one selection mark and
  uses the system clipboard, without a kill ring. Keymaps accept string
  command ids and null, without Zed command arguments or platform keys.
- Edit predictions change one contiguous range of at most 40 lines in the
  cursor excerpt. They do not edit another file. Notebooks use model
  insertion suggestions. Recent edits and prediction caches last for the window.
  Copilot needs a GitHub account with access to Copilot. Installation and
  the start of device sign-in are tested; signed-in completions need an
  account and have not been live-tested.
- A window of several folders runs one language server per folder rather
  than one server with several workspace folders, and does not send
  `didChangeWorkspaceFolders`. Task output, test results and the agent's
  thread list follow the active folder. A window holds at most 64 folders.
- The Source Control view lists files flat, without folders, and its
  message field holds one line; use the commit dialog's review or a
  terminal for longer messages. Selected-line staging handles one hunk at a
  time, and needs saved files already tracked by Git.
- Whole-file blame labels each run with hash, author and date only; the
  commit's subject shows when it is opened.
- Project search matches within one line; its history lasts for the
  window's session. Replacing a match opens its file in a tab.
- Multibuffers are not kept across restarts, and crash recovery does not
  keep unsaved edits held in a multibuffer's own buffers; save them first.
  A multibuffer follows edits made elsewhere when it is shown; one in a
  side-by-side view is brought up to date when it is focused. Expanding or
  changing excerpts cannot be undone, and Undo there covers only edits
  made in that multibuffer. Notebooks, PDFs, pictures and files over 2 MB
  are left out, and each excerpt is highlighted on its own, without the
  lines around it. At most 2,000 excerpts are shown.
- Auto-save covers the active tab only; switching tabs cancels a pending delay.
- Config file schemas check one file at a time. A `preLaunchTask` naming a
  missing task, or a run `cwd` that does not exist, shows only when used.
- Tree-sitter grammars need `curl`, `tar` and a C compiler. Embedded languages
  highlight only when their grammar is installed; unsupported languages such
  as regex and JSDoc keep host highlighting. Injection nesting stops at eight
  layers and shares the worker's time budget. Grammar queries support the
  runtime's text predicates and injection properties; custom query directives
  such as `#offset!` are not interpreted. Markdown keeps its renderer.
  Query symbols describe syntax, without resolving types or references.
  Project indexing skips files over 2 MB and reads at most 64 MB per pass.
- `cargo test` reports no per-test times on stable Rust; nextest does.
  Vitest tests are placed by searching their file for the test's name. Go
  subtests count toward their parent test. Only pytest tests can be
  debugged from the Tests tab.
- The agent's `run_tests` tool runs `cargo test` or pytest; the Tests tab's
  other runners are not used by it yet.
- Dev containers support the subset of `devcontainer.json` described under
  Terminal.
- Interactive rebase cannot edit, split or exec commits, or replay merge
  commits. The merge editor works on text files both sides kept; the
  columns are read-only and do not wrap long lines.
- Pending PR reviews live only in the window, not as GitHub draft reviews.
  Thread and comment lists show the latest 100.
- Wayland only; there is no X11 backend.
- Remote images are not fetched, and HTML `<img>` tags are not rendered.
- Terminal output follows the application's cell order; bidirectional reordering
  applies to text panes and fields. Menu and chooser filters edit at the end.
- Documents open in tabs and editor views in nested rows and columns, with
  one tab strip for all views; a text file can have at most two views. Diffs have unified and side-by-side
  views.
  Semantic completion uses Python, LaTeX or an installed language server;
  other languages have lexical highlighting, or tree-sitter's with an
  installed grammar.
- Language-server semantic colours add to the lexical highlighting but do
  not replace it, and they wait for the server after each edit.
- Save-time code actions remain restricted to text edits in the file being
  saved; use the Code actions menu for commands and workspace operations.
- Managed server downloads target Linux x86_64 or aarch64 when upstream
  publishes a matching asset. Ruby, Swift and Haskell servers must already
  be installed. Resource workspace edits reject symlinks, paths outside the
  current project, and operations exceeding 64 MB or 10,000 paths. Restoring
  a resource-edit checkpoint may leave newly created empty directories.
- Markdown PDF source jumps map code listings and footnotes to their block.
  Markdown built into a PDF leaves out HTML other than `<u>` and `<br>`,
  and keeps the alt text of WebP, GIF, SVG and remote pictures.
- A formula taller than a line is made smaller to fit in it; one that is
  displayed is not. Mathematics in a table stays source.
- A PDF's text is in the order the PDF draws it, which in some documents
  is not the order it is read in. Links to other files are not followed. Text in the
  preview's pages cannot be selected; open the PDF to do that. Encrypted
  PDFs are not shown.
- Without a Jupyter kernel and `jupyter_client`, notebooks run on
  Lightdown's own helper, not IPython: `!command`, `%pip` and `%matplotlib`
  lines work, other magics do not. HTML other than tables shows its plain
  text; widgets (ipywidgets) and JavaScript outputs are not run. Formulas
  inside markdown outputs stay source. Remote kernels (Jupyter servers)
  are not supported; kernels run on this machine. Script cells cannot
  answer `input()`.
- The agent speaks to the models served in the OpenAI chat, OpenAI
  responses and Anthropic messages forms. Gemini models are not offered.
- Claude Code and Codex use their own tools, not Lightdown's: they read
  and write files on disk, so unsaved changes in open tabs are not seen,
  and they have no notebook, `problems`, `run_tests` or language-server
  tools. Their edits to a file with unsaved changes raise the usual
  "changed on disk" question.
- Claude Code and Codex work only in local projects. Their own settings,
  rules, MCP servers and instruction files (`CLAUDE.md`, `AGENTS.md`) apply
  as when run in a terminal; Lightdown's MCP servers and `AGENTS.md` setting
  do not.
- Forking a Claude Code or Codex thread, or editing an earlier message,
  starts a fresh session that is given the earlier conversation as text.
  Commands Claude Code's own settings allow run without Lightdown's
  approval card.
- Codex reports tokens but no cost. A live Codex turn was not run end to end
  here (the account was at its usage limit); its client was checked against a
  recorded app-server session. Subagents' work inside Claude Code is
  not shown, only its result. Codex's questions to the user and MCP
  elicitations are declined.
- The terminal leaves out marks that combine with the letter before them
  and shows no images (sixel, kitty).
- Bundled themes keep their authors' syntax colours, held to a
  contrast of 3 rather than the built-in themes' 4.5. Importing reads one
  file: VS Code `include` and `semanticTokenColors` are not followed, and
  scope selectors with spaces (`source.python keyword`) are skipped. The
  terminal does not use ligatures or `buffer_font_features`; its glyphs
  are one per cell. The desktop's preference is asked every two seconds
  while `theme_mode` is `system`, not watched as a signal. An interface
  family needs regular, italic, bold and medium faces that fontconfig can
  match. Icon themes have no per-language or per-folder-name icons and
  are not checked by a schema.

## Licenses

Libron is by Nico Verbruggen and distributed under the SIL Open Font License;
see `assets/fonts/OFL.txt`. Inter is by The Inter Project Authors and
distributed under the SIL Open Font License; see `assets/fonts/OFL-Inter.txt`.

The bundled themes in `assets/themes` take their colours from these
themes, all under the MIT License: [One Dark and One Light][atom-one] by
GitHub Inc. (Atom), [Solarized] by Ethan Schoonover, [Gruvbox] by Pavel
Pertsev (morhetz), [Nord] by Arctic Ice Studio and Sven Greb,
[Catppuccin] by the Catppuccin organisation, and [Tokyo Night] by enkia.
Some interface roles were adjusted for contrast; each file names its
source in `license`.

[smithay-client-toolkit]: https://github.com/Smithay/client-toolkit
[swash]: https://github.com/dfrg/swash
[pulldown-cmark]: https://github.com/pulldown-cmark/pulldown-cmark
[Libron]: https://github.com/nicoverbruggen/libron
[Inter]: https://github.com/rsms/inter
[serde_json]: https://github.com/serde-rs/json
[vte]: https://github.com/alacritty/vte
[hayro]: https://github.com/LaurenzV/hayro
[jedi]: https://github.com/davidhalter/jedi
[OpenCode]: https://opencode.ai
[resvg]: https://github.com/linebender/resvg
[models.dev]: https://models.dev
[atom-one]: https://github.com/atom/atom/tree/master/packages/one-dark-syntax
[Solarized]: https://github.com/altercation/solarized
[Gruvbox]: https://github.com/morhetz/gruvbox
[Nord]: https://github.com/nordtheme/nord
[Catppuccin]: https://github.com/catppuccin/catppuccin
[Tokyo Night]: https://github.com/enkia/tokyo-night-vscode-theme

