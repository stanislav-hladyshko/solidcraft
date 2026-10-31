# The scan-to-solid fork

This branch (`scan-to-solid`, from v0.26.1) adds one ribbon tab, **Scan To Solid**, to SolidCraft:
a scan turned into a parametric solid by the scan-to-solid scripts (the `scan-to-solid` repository,
ADR 0018 there). Everything else is upstream SolidCraft.

## What it adds

| Where | What |
|---|---|
| `crates/engine/src/cmd/scan_to_solid.rs` | 23 commands `scan.<action>` on the tab `SCAN TO SOLID`, in six panels that are the process's stages left to right (ADR 0027 of the scan-to-solid repository): Prepare, Recognise, Model, Review, Output, and Known Part for the commands that need a part's build script. The engine only names the action; `action_of` maps an id to it. |
| `crates/ui-egui/src/workspace.rs` | the tab in `TABS`, its six sections in `SECTIONS`. |
| `crates/ui-egui/src/scan_to_solid.rs` | the tab's actions as command dialogs: their picks and fields, the scripts they run, what those say on the status line. |
| `crates/ui-egui/src/dialogs.rs`, `dialogs_dense.rs` | `Kind::Scan`: a tab button's command dialog, opened by `Dialog::for_command` like any other; its OK runs the script. |
| `crates/ui-egui/src/lib.rs` | `SolveApp::scan_to_solid`; `start` waits for the actions once, `poll` each frame. |
| `apps/solidcraft/src/main.rs` | the scripts stop when the app closes. |
| `crates/ui-egui/src/file_load.rs`, `lib.rs` | Insert Mesh puts a PLY in as a mesh (`file.insert_mesh`), as its name says. Upstream decides by the extension and makes a PLY a point cloud, which Section cannot cut; Creality writes its scans as `Mesh.ply`. |
| `crates/ui-egui/src/command_help.rs`, `assets/locales/pending/scan-to-solid.json` | the tab's names, the button labels and their help. English only: the other complete catalogs (ar, he, ja, vi, zh) carry the English text until the app is translated as a whole. |
| `.github/workflows/scan-to-solid.yml` | a Windows x64 build of this branch (unsigned), with the tab's tests. |

## How a button works

There is no window of our own: each button opens a command dialog, as Extrude does (ADR 0019 of
the scan-to-solid repository).

1. At startup the tab runs `solidcraft_run.py describe` once, in the background: for each action,
   what its dialog asks to pick (`select`) and its fields (`solidcraft_actions.PANEL`). A new
   action is a change in the scripts, not here.
2. The ribbon button `scan.<action>` reaches `SolveApp::start`, and `Dialog::for_command` opens the
   action's dialog: its selection inputs, then its fields (the button's row, and its group's rows
   without a button). What is selected before becomes the inputs, as for any command.
3. OK runs `python -u scripts/solidcraft_run.py run <action> '<fields json>'` in the scan-to-solid
   folder, off the window thread, with the picks under `selection`, and closes the dialog. The
   script changes the open design through the control channel, which the window keeps serving;
   the window gives it its own port in `SOLIDCRAFT_CONTROL_PORT`, since `current.port` names the
   SolidCraft started last and goes when that one closes.
   One run at a time: while one goes on, OK of every dialog of the tab is greyed (Enter too), and
   the footer says the run, how long it has taken and its latest `say`, as a feature's dialog says
   "Computing… N s"; OK comes back when the run ends.
4. Each line the script writes is JSON: `say` while it works, then `answer`, `refused` or `error`.
   They go to the status line and Messages. A run that ends without its last word says so, with
   the tail of its error output.
5. Check with Live on runs `solidcraft_run.py watch check …`: Check again after every design change
   it did not make, a change made by another button of the tab included. Check with Live off stops
   it; a watch that stops by itself says so.

The scan-to-solid folder is `SCAN_TO_SOLID_HOME`, else `C:\Repo\hardware\scan-to-solid`, with its
Python in `.venv`. Without it the window says where it looked.

## Build

On Windows with Rust (MSVC) and the Visual Studio C++ tools:

```
cargo build --release --locked -p solidcraft -p solidcraft-cli
cargo test -p solidcraft-engine scan_to_solid
cargo test -p solidcraft-ui-egui scan_to_solid
```

The binaries are `target/release/solidcraft.exe` and `solidcraft-cli.exe`. A push of the branch
builds the same in GitHub Actions (`scan-to-solid.yml`). On Linux the Windows exe cross-builds as
upstream's `docs/building.md` says (`cargo xwin build --release --target x86_64-pc-windows-msvc -p solidcraft`),
which is how this branch has been built so far, unpushed.

## Keeping up with upstream

```
git fetch upstream --tags
git merge v<next>
```

The tab's code sits in its own files; the shared files (`workspace.rs`, `lib.rs`, `main.rs`,
`cmd/mod.rs`, `engine/src/lib.rs`, `command_help.rs`) change in a few marked lines.
