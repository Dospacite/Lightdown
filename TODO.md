Completed the second round of 50 tasks on 2026-11-05, on branch `round-2`. Five Codex agents started them, one per area, and got through about a fifth before stopping early and then hitting their usage limit. Twelve Opus agents finished them in separate worktrees, each taking over its area's committed and uncommitted work. The twelve branches were merged and the seams joined afterwards: one process spawner for jobs and shells, the pre-launch task on the job runner, line-end text and gutter marks that respect horizontal scrolling and the blame column, nine right-panel tabs behind one overflow, the project environment for language servers, and schemas for every new setting and config file. The previous round's list is in Git history. Commands, configuration and limits are documented in README.md.

## Core features

1. [x] **Source Control panel.** A left-sidebar view beside the explorer with staged, changed, untracked and conflicted files, per-file and per-section stage, unstage and discard, a commit message box with Commit, Amend and a drafted message. Ctrl+Shift+G.
2. [x] **Search panel.** A right-panel tab with replace, case, word and regex toggles, include and exclude globs, results grouped by file and streamed in, unsaved tabs searched as their buffers, previewed replacements and history.
3. [x] **Tests panel.** A tree by runner, file and test with status and durations, run, debug, rerun failed and run with coverage, failure messages at the failing line, and expected-vs-actual in the side-by-side diff. Ctrl+Shift+Y.
4. [x] **Several jobs at once.** Up to 16 tasks and run configurations together, each stopped on its own, and compound run configurations that start and stop together.
5. [x] **Jobs in terminal tabs.** Tasks and run configurations run on a pseudo-terminal with colour, input and scrollback, while their errors still feed Problems and F8.
6. [x] **Tree-sitter grammars.** 23 pinned grammars, installed on demand, parsed incrementally on one worker, drive highlighting, folding, the outline, sticky headers and expand-selection. Highlight queries only; project symbol search stays lexical.
7. [x] **Dev containers.** `.devcontainer/devcontainer.json` (a documented subset) or `.lightdown/container.json` runs terminals, tasks, tests, language servers and agent commands inside docker or podman, with paths mapped.
8. [x] **Vim mode.** Modes, motions, operators with text objects and counts, registers, dot repeat, search, marks and ex commands, off by default. Macros and scroll commands are not included.

## Code intelligence

9. [x] **Go to implementation, type definition and declaration.** Ctrl+F12, Ctrl+Shift+F12, Ctrl+Alt+F12.
10. [x] **Call and type hierarchy.** A Hierarchy tab, expanded lazily.
11. [x] **Server symbols.** The outline and project symbols come from the server when it has them.
12. [x] **Semantic tokens.** Server colours where the lexical highlighter had none.
13. [x] **Peek definition and references.** Alt+F12 and Alt+Shift+F12, in a block under the line.
14. [x] **Inline diagnostics and lightbulb.** Messages at line ends and on hover, and a lightbulb for code actions.
15. [x] **Language server status.** Progress, state, restart, stop and log in the status bar, and pull diagnostics.
16. [x] **Code actions on save and format selection.** `code_actions_on_save`, and Code: Format selection.

## Debugging

17. [x] **Expandable variables.** A tree for DAP and the Python bridge, with paging, that never runs the program's code.
18. [x] **Exception breakpoints.** Raised and uncaught for Python, the adapter's filters for DAP, and Rust panics.
19. [x] **Values while paused.** Locals at line ends, and hover evaluation of names and attribute chains.
20. [x] **Threads and attach.** A thread picker, attach to a process by pid, and Python through debugpy.
21. [x] **Debug configurations.** Named configurations, preLaunchTask, variables, adapter discovery, Cargo builds and templates. Node starts but does not stop at breakpoints.

## Git and GitHub

22. [x] **Whole-file blame.** A blame column; clicking a run opens its commit.
23. [x] **History of selected lines.** `git log -L` for the selection or the enclosing function.
24. [x] **Stage selected lines.** Partial hunks staged and unstaged with `git apply --cached`.
25. [x] **Three-way merge editor.** Ours, base and theirs above the editable result.
26. [x] **Rebase and merge.** Merge, rebase, interactive rebase, pull with rebase or merge, and Continue, Skip and Abort in the status bar.
27. [x] **PR review threads.** Reply, resolve and unresolve, and submit reviews.
28. [x] **Failed CI logs into Problems.**
37. [x] **Worktree overview.** Every agent worktree with its state, cost and actions.
47. [x] **Compare files.** Two files, two tabs, or the selection with the clipboard.
49. [x] **Stash list and review base.** Every stash, and any base branch or commit.

## Run, terminal and environment

29. [x] **More test runners.** Jest, Vitest, go test, cargo-nextest and CTest beside cargo and pytest.
30. [x] **`.lightdown/tasks.json`.** Own tasks with dependencies, problem matchers, watch globs and background tasks.
31. [x] **Shell integration.** Command marks with exit status, jumps between commands, last output and rerun, set up for bash, zsh and fish.
32. [x] **Send the selection to the terminal or the Python kernel.**
33. [x] **direnv, mise and Nix.** Opt-in per project, for everything Lightdown starts, language servers included.
34. [x] **Ports and SSH sync.** Detected ports, SSH port forwarding and push on save.

## Agent

35. [x] **Language-server tools on the running server.** Plus hover, symbols and rename.
36. [x] **A check before the agent finishes.** `problems` and `run_tests` tools and a project `check` command. `run_tests` uses cargo test or pytest, and the check runs only when commands are allowed.
38. [x] **Fix with agent.** From a Problems row or the code actions menu.

## Small tweaks

39. [x] **Detect indentation.**
40. [x] **Status-bar indentation, line endings and encoding,** each a control.
41. [x] **No-wrap mode with horizontal scrolling.** Alt+Z.
42. [x] **Line bookmarks.** Ctrl+Alt+K, Ctrl+Alt+] and Ctrl+Alt+[.
43. [x] **Matching bracket and next or previous Git change.** Ctrl+Alt+B, Alt+F8.
44. [x] **Select all occurrences, skip one, find in selection, preserve case.**
45. [x] **Auto-save.**
46. [x] **User snippets, mirrored stops, choices and variables.**
48. [x] **Schemas for Lightdown's config files.**
50. [x] **README fixes.**
