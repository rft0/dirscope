#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevation;
mod icons;
mod scan;

use icons::{Icon, icon};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, ClickEvent, Context, ElementId, FocusHandle, Hsla, KeyDownEvent,
    PathPromptOptions, Pixels, PromptButton, PromptLevel, Rgba, ScrollStrategy, SharedString, Stateful, TitlebarOptions,
    UniformListScrollHandle, Window, WindowBounds, WindowOptions, div, prelude::*, px, relative,
    rgb, size, uniform_list,
};

use scan::{NodeId, Progress, ROOT, Tree, format_count, format_size};

// ---- theme ---------------------------------------------------------------

const BG: u32 = 0x1e1f22;
const PANEL: u32 = 0x2b2d30;
const ROW_ALT: u32 = 0x232427;
const BORDER: u32 = 0x3c3f44;
const TEXT: u32 = 0xdfe1e5;
const DIM: u32 = 0x8c9099;
const ACCENT: u32 = 0x3574f0;
const SELECTED: u32 = 0x2e436e;
const FOLDER: u32 = 0xe8b04a;
const FILE: u32 = 0x7f8794;
const DANGER: u32 = 0xc94f4f;
const ERROR_TEXT: u32 = 0xe07a7a;

/// Windows 11's UI font; gpui falls back to the system default if it's missing.
const UI_FONT: &str = "Segoe UI Variable Text";

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 16.0;
const COL_SIZE: f32 = 90.0;
const COL_BAR: f32 = 140.0;
const COL_PCT: f32 = 60.0;
const COL_FILES: f32 = 90.0;

/// Bar color shifts from blue to red as an entry takes a bigger share of its parent.
fn bar_color(frac: f32) -> Hsla {
    let hue = (0.6 - 0.6 * frac.clamp(0.0, 1.0)) * 1.0;
    gpui::hsla(hue, 0.65, 0.55, 1.0)
}

// ---- app state -----------------------------------------------------------

struct Row {
    id: NodeId,
    depth: usize,
}

struct Scanning {
    generation: u64,
    path: PathBuf,
    progress: Arc<Progress>,
    started: Instant,
}

struct DirScope {
    tree: Option<Tree>,
    /// Bumped on every new scan so stale async results are ignored.
    generation: u64,
    rows: Vec<Row>,
    selected: Option<NodeId>,
    scanning: Option<Scanning>,
    deleting: bool,
    status: String,
    drives: Vec<PathBuf>,
    elevated: bool,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl DirScope {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            tree: None,
            generation: 0,
            rows: Vec::new(),
            selected: None,
            scanning: None,
            deleting: false,
            status: "Pick a folder or drive to scan.".into(),
            drives: list_drives(),
            elevated: elevation::is_elevated(),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    // ---- elevation -------------------------------------------------------

    /// Asks whether to restart elevated, since without admin rights many
    /// system folders can't be read (sizes come out low) or deleted.
    fn offer_elevation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // DIRSCOPE_NO_ADMIN_PROMPT=1 skips the dialog (scripted runs, screenshots).
        if self.elevated || std::env::var_os("DIRSCOPE_NO_ADMIN_PROMPT").is_some() {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Info,
            "Restart DirScope as administrator?",
            Some(
                "Without administrator rights, folders like Windows, Program Files, \
                 ProgramData and other users' profiles can't be fully read or deleted, \
                 so their sizes will be incomplete.",
            ),
            // The first button is the dialog default, so Enter must never elevate.
            &[
                PromptButton::cancel("Continue Without"),
                PromptButton::new("Restart as Administrator"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(1) = answer.await {
                this.update(cx, |this, cx| this.restart_as_admin(cx)).ok();
            }
        })
        .detach();
    }

    fn restart_as_admin(&mut self, cx: &mut Context<Self>) {
        // Carry over the current folder so the elevated copy rescans it right away.
        let folder = self
            .scanning
            .as_ref()
            .map(|s| s.path.clone())
            .or_else(|| self.tree.as_ref().map(|t| t.root_path.clone()));
        let args: Vec<_> = folder.into_iter().map(|p| p.into_os_string()).collect();

        if elevation::relaunch_as_admin(&args) {
            cx.quit();
        } else {
            self.status = "Didn't restart as administrator (UAC prompt was declined).".into();
            cx.notify();
        }
    }

    // ---- scanning --------------------------------------------------------

    fn open_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Scan".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(path) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.start_scan(path, cx)).ok();
            }
        })
        .detach();
    }

    fn start_scan(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(old) = &self.scanning {
            old.progress.cancel.store(true, Ordering::Relaxed);
        }
        self.generation += 1;
        let generation = self.generation;
        let progress = Arc::new(Progress::default());

        self.scanning = Some(Scanning {
            generation,
            path: path.clone(),
            progress: progress.clone(),
            started: Instant::now(),
        });
        self.status = format!("Scanning {}…", path.display());
        cx.notify();

        // The walk itself runs on rayon's pool; this background task just waits for it.
        cx.spawn(async move |this, cx| {
            let work = cx
                .background_executor()
                .spawn(async move { scan::scan(&path, &progress) });
            let tree = work.await;
            this.update(cx, |this, cx| this.finish_scan(generation, tree, cx))
                .ok();
        })
        .detach();

        // Repaint a few times a second so the live counters move.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let still_running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.scanning
                        .as_ref()
                        .is_some_and(|s| s.generation == generation)
                });
                if !matches!(still_running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_scan(&mut self, generation: u64, tree: Tree, cx: &mut Context<Self>) {
        let Some(scanning) = self.scanning.take_if(|s| s.generation == generation) else {
            return; // a newer scan superseded this one
        };
        let elapsed = scanning.started.elapsed().as_secs_f32();
        let cancelled = scanning.progress.cancelled();
        let errors = scanning.progress.errors.load(Ordering::Relaxed);
        let root = tree.node(ROOT);
        self.status = format!(
            "{}{} in {} files, {} folders · {:.1}s{}",
            if cancelled { "Cancelled — partial: " } else { "" },
            format_size(root.size),
            format_count(root.files),
            format_count(scanning.progress.dirs.load(Ordering::Relaxed)),
            elapsed,
            if errors > 0 {
                format!(" · {} folders unreadable", format_count(errors))
            } else {
                String::new()
            }
        );
        self.tree = Some(tree);
        self.selected = Some(ROOT);
        self.rebuild_rows();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = &self.scanning {
            s.progress.cancel.store(true, Ordering::Relaxed);
            self.status = "Cancelling…".into();
            cx.notify();
        }
    }

    /// Rescans the current folder, or the first drive (C:\) if nothing was scanned yet.
    fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning.is_some() {
            return;
        }
        let path = match &self.tree {
            Some(tree) => tree.root_path.clone(),
            None => match self.drives.first() {
                Some(drive) => drive.clone(),
                None => return,
            },
        };
        self.start_scan(path, cx);
    }

    // ---- tree navigation -------------------------------------------------

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let Some(tree) = &self.tree else { return };
        let mut stack = vec![(ROOT, 0usize)];
        while let Some((id, depth)) = stack.pop() {
            self.rows.push(Row { id, depth });
            let node = tree.node(id);
            if node.expanded {
                for &child in node.children.iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
    }

    fn selected_row(&self) -> Option<usize> {
        let sel = self.selected?;
        self.rows.iter().position(|r| r.id == sel)
    }

    fn select_row(&mut self, ix: usize) {
        if let Some(row) = self.rows.get(ix) {
            self.selected = Some(row.id);
            self.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.selected_row().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.rows.len() as isize - 1);
        self.select_row(next as usize);
    }

    fn set_expanded(&mut self, id: NodeId, expanded: bool) {
        let Some(tree) = &mut self.tree else { return };
        let node = tree.node_mut(id);
        if node.is_dir && !node.children.is_empty() && node.expanded != expanded {
            node.expanded = expanded;
            self.rebuild_rows();
        }
    }

    fn toggle(&mut self, id: NodeId) {
        let Some(tree) = &self.tree else { return };
        let expanded = tree.node(id).expanded;
        self.set_expanded(id, !expanded);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ctrl = ev.keystroke.modifiers.control;
        match ev.keystroke.key.as_str() {
            "up" => self.move_selection(-1),
            "down" => self.move_selection(1),
            "pageup" => self.move_selection(-20),
            "pagedown" => self.move_selection(20),
            "home" => self.select_row(0),
            "end" => self.select_row(self.rows.len().saturating_sub(1)),
            "right" => {
                if let (Some(id), Some(tree)) = (self.selected, &self.tree) {
                    let node = tree.node(id);
                    if node.is_dir && !node.expanded {
                        self.set_expanded(id, true);
                    } else if let Some(&first) = node.children.first() {
                        self.selected = Some(first);
                        if let Some(ix) = self.selected_row() {
                            self.select_row(ix);
                        }
                    }
                }
            }
            "left" => {
                if let (Some(id), Some(tree)) = (self.selected, &self.tree) {
                    let node = tree.node(id);
                    if node.expanded {
                        self.set_expanded(id, false);
                    } else if let Some(parent) = node.parent {
                        self.selected = Some(parent);
                        if let Some(ix) = self.selected_row() {
                            self.select_row(ix);
                        }
                    }
                }
            }
            "enter" | "space" => {
                if let Some(id) = self.selected {
                    self.toggle(id);
                }
            }
            "delete" => self.request_delete(window, cx),
            "f5" => self.rescan(cx),
            "o" if ctrl => self.open_folder(cx),
            "escape" => self.cancel_scan(cx),
            _ => return,
        }
        cx.notify();
    }

    // ---- deleting --------------------------------------------------------

    fn request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.deleting || self.scanning.is_some() {
            return;
        }
        let (Some(tree), Some(id)) = (&self.tree, self.selected) else {
            return;
        };
        if id == ROOT {
            self.status = "The scan root can't be deleted from here — pick something inside it.".into();
            cx.notify();
            return;
        }
        let node = tree.node(id);
        let path = tree.path_of(id);
        let is_dir = node.is_dir;
        let message = format!(
            "Delete {} \"{}\"?",
            if is_dir { "folder" } else { "file" },
            node.name
        );
        let detail = format!(
            "{}\n\n{} · {} files",
            path.display(),
            format_size(node.size),
            format_count(node.files)
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some(&detail),
            // The first button is the dialog default, so a stray Enter cancels.
            &[
                PromptButton::cancel("Cancel"),
                PromptButton::new("Move to Recycle Bin"),
                PromptButton::new("Delete Permanently"),
            ],
            cx,
        );
        let generation = self.generation;

        cx.spawn(async move |this, cx| {
            let permanent = match answer.await {
                Ok(1) => false,
                Ok(2) => true,
                _ => return,
            };
            let started = this.update(cx, |this, cx| {
                if this.generation != generation || this.deleting {
                    return false;
                }
                this.deleting = true;
                this.status = format!("Deleting {}…", path.display());
                cx.notify();
                true
            });
            if !matches!(started, Ok(true)) {
                return;
            }

            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { delete_path(&target, is_dir, permanent) })
                .await;

            this.update(cx, |this, cx| {
                this.finish_delete(generation, id, &path, permanent, result, cx)
            })
            .ok();
        })
        .detach();
    }

    fn finish_delete(
        &mut self,
        generation: u64,
        id: NodeId,
        path: &Path,
        permanent: bool,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.deleting = false;
        match result {
            Err(err) => {
                self.status = format!("Couldn't delete {}: {err}", path.display());
            }
            Ok(()) => {
                let freed = self
                    .tree
                    .as_ref()
                    .filter(|_| self.generation == generation)
                    .filter(|t| t.path_of(id) == path)
                    .map(|t| t.node(id).size);
                if let Some(freed) = freed {
                    // Pick the row below (or above) so the user can keep pruning.
                    let ix = self.selected_row();
                    let tree = self.tree.as_mut().unwrap();
                    let parent = tree.node(id).parent;
                    tree.remove(id);
                    self.rebuild_rows();
                    self.selected = ix
                        .and_then(|ix| self.rows.get(ix).or(self.rows.last()))
                        .map(|r| r.id)
                        .or(parent);
                    self.status = format!(
                        "{} {} · freed {}",
                        if permanent { "Deleted" } else { "Recycled" },
                        path.display(),
                        format_size(freed)
                    );
                } else {
                    self.status = format!("Deleted {}", path.display());
                }
            }
        }
        cx.notify();
    }

    fn show_in_explorer(&mut self) {
        let (Some(tree), Some(id)) = (&self.tree, self.selected) else {
            return;
        };
        reveal(&tree.path_of(id));
    }

    // ---- rendering -------------------------------------------------------

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let scanning = self.scanning.is_some();
        let can_delete =
            !scanning && !self.deleting && self.selected.is_some_and(|id| id != ROOT);
        let has_tree = self.tree.is_some();

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_2()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                button("open", Icon::FolderOpen, "Open Folder", true, ACCENT)
                    .on_click(cx.listener(|this, _, _, cx| this.open_folder(cx))),
            )
            .children(self.drives.iter().enumerate().map(|(i, drive)| {
                let drive = drive.clone();
                let label = drive.display().to_string();
                button(("drive", i), Icon::Drive, label, true, PANEL).on_click(cx.listener(
                    move |this, _, _, cx| this.start_scan(drive.clone(), cx),
                ))
            }))
            .child(div().w(px(12.)))
            .child(
                // With nothing scanned yet, Rescan scans the first drive (C:\).
                button("rescan", Icon::Refresh, "Rescan (F5)", !scanning, PANEL)
                    .when(!scanning, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| this.rescan(cx)))
                    }),
            )
            .when(scanning, |bar| {
                bar.child(
                    button("cancel", Icon::Close, "Cancel (Esc)", true, PANEL)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))),
                )
            })
            .child(
                button("reveal", Icon::External, "Show in Explorer", has_tree, PANEL).when(has_tree, |b| {
                    b.on_click(cx.listener(|this, _, _, _| this.show_in_explorer()))
                }),
            )
            .child(
                button("delete", Icon::Trash, "Delete (Del)", can_delete, DANGER).when(can_delete, |b| {
                    b.on_click(cx.listener(|this, _, window, cx| this.request_delete(window, cx)))
                }),
            )
            .when(!self.elevated, |bar| {
                bar.child(div().flex_1()).child(
                    button("admin", Icon::Shield, "Restart as Admin", true, PANEL)
                        .on_click(cx.listener(|this, _, _, cx| this.restart_as_admin(cx))),
                )
            })
    }

    fn render_header(&self) -> impl IntoElement {
        let cell = |label: &'static str, w: f32| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .child(label)
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(ROW_HEIGHT))
            .px_2()
            .gap_2()
            .text_xs()
            .text_color(rgb(DIM))
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().flex_1().min_w_0().child("NAME"))
            .child(cell("SIZE", COL_SIZE))
            .child(div().w(px(COL_BAR)).flex_shrink_0().child("SHARE OF PARENT"))
            .child(cell("%", COL_PCT))
            .child(cell("FILES", COL_FILES))
    }

    fn render_row(
        &self,
        ix: usize,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let tree = self.tree.as_ref()?;
        let row = self.rows.get(ix)?;
        let id = row.id;
        let node = tree.node(id);
        let parent_size = node.parent.map(|p| tree.node(p).size).unwrap_or(node.size);
        let frac = if parent_size == 0 {
            0.0
        } else {
            node.size as f32 / parent_size as f32
        };
        let selected = self.selected == Some(id);
        let expandable = node.is_dir && !node.children.is_empty();

        let bg = if selected {
            rgb(SELECTED)
        } else if ix % 2 == 1 {
            rgb(ROW_ALT)
        } else {
            rgb(BG)
        };

        let chevron = div()
            .id(("chevron", ix))
            .size(px(16.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .when(expandable, |c| {
                c.cursor_pointer()
                    .hover(|s| s.bg(rgb(BORDER)))
                    .child(icon(
                        if node.expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        px(12.),
                        rgb(DIM),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.selected = Some(id);
                        this.toggle(id);
                        cx.notify();
                    }))
            });

        let glyph = match (node.is_dir, node.expanded) {
            (false, _) => Icon::File,
            (true, true) => Icon::FolderOpen,
            (true, false) => Icon::Folder,
        };
        let tint = if node.error {
            ERROR_TEXT
        } else if node.is_dir {
            FOLDER
        } else {
            FILE
        };
        let kind_icon = icon(glyph, px(16.), rgb(tint));

        let mut name = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .child(SharedString::from(node.name.clone()));
        if node.error {
            name = name.text_color(rgb(ERROR_TEXT));
        }

        let bar = div()
            .w(px(COL_BAR))
            .h(px(10.))
            .flex_shrink_0()
            .rounded_sm()
            .bg(rgb(0x3a3d42))
            .child(
                div()
                    .h_full()
                    .w(relative(frac))
                    .rounded_sm()
                    .bg(bar_color(frac)),
            );

        let num = |text: String, w: f32, color: Rgba| {
            div()
                .w(px(w))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .text_color(color)
                .child(text)
        };

        Some(
            div()
                .id(ix)
                // uniform_list lays rows out as roots, so w_full() has nothing to fill.
                .w(width)
                .flex()
                .flex_row()
                .items_center()
                .h(px(ROW_HEIGHT))
                .px_2()
                .gap_2()
                .bg(bg)
                .when(!selected, |r| r.hover(|s| s.bg(rgb(0x2a2c30))))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    this.selected = Some(id);
                    if ev.click_count() >= 2 {
                        this.toggle(id);
                    }
                    window.focus(&this.focus);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1p5()
                        .pl(px(row.depth as f32 * INDENT))
                        .child(chevron)
                        .child(kind_icon)
                        .child(name),
                )
                .child(num(format_size(node.size), COL_SIZE, rgb(TEXT)))
                .child(bar)
                .child(num(format!("{:.1}%", frac * 100.0), COL_PCT, rgb(DIM)))
                .child(num(
                    if node.is_dir {
                        format_count(node.files)
                    } else {
                        String::new()
                    },
                    COL_FILES,
                    rgb(DIM),
                )),
        )
    }

    fn render_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(s) = &self.scanning
            && self.tree.is_none()
        {
            return placeholder(scan_progress_text(s)).into_any_element();
        }
        if self.tree.is_none() {
            let drive = self
                .drives
                .first()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "a drive".into());
            return placeholder(format!(
                "Nothing scanned yet. Press Open Folder or {drive} above to start a scan."
            ))
            .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(self.render_header())
            .child(
                uniform_list(
                    "rows",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                        // The list spans the whole window, so rows match the header.
                        let width = window.viewport_size().width;
                        range
                            .filter_map(|ix| this.render_row(ix, width, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .track_scroll(self.scroll.clone()),
            )
            .into_any_element()
    }

    fn render_status(&self) -> impl IntoElement {
        let left = match &self.scanning {
            Some(s) => scan_progress_text(s),
            None => self.status.clone(),
        };
        let right = match (&self.tree, self.selected) {
            (Some(tree), Some(id)) => tree.path_of(id).display().to_string(),
            _ => String::new(),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .px_3()
            .h(px(26.))
            .text_xs()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(div().flex_shrink_0().text_color(rgb(TEXT)).child(left))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .flex()
                    .justify_end()
                    .text_color(rgb(DIM))
                    .child(right),
            )
    }
}

impl Render for DirScope {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .flex()
            .flex_col()
            .size_full()
            .font_family(UI_FONT)
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .text_sm()
            .child(self.render_toolbar(cx))
            .child(self.render_body(cx))
            .child(self.render_status())
    }
}

// ---- helpers -------------------------------------------------------------

fn button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    enabled: bool,
    color: u32,
) -> Stateful<gpui::Div> {
    div()
        .id(id.into())
        .flex()
        .flex_row()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(color))
        .text_sm()
        .child(icon(glyph, px(15.), rgb(TEXT)))
        .child(label.into())
        .when(enabled, |b| {
            b.cursor_pointer().hover(|s| s.border_color(rgb(DIM)))
        })
        .when(!enabled, |b| b.opacity(0.4))
}

fn placeholder(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgb(DIM))
        .child(text.into())
}

fn scan_progress_text(s: &Scanning) -> String {
    let p = &s.progress;
    format!(
        "Scanning {} · {} files · {} folders · {} · {:.1}s",
        s.path.display(),
        format_count(p.files.load(Ordering::Relaxed)),
        format_count(p.dirs.load(Ordering::Relaxed)),
        format_size(p.bytes.load(Ordering::Relaxed)),
        s.started.elapsed().as_secs_f32()
    )
}

fn delete_path(path: &Path, is_dir: bool, permanent: bool) -> Result<(), String> {
    let result = if permanent {
        if is_dir {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())
        } else {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        }
    } else {
        trash::delete(path).map_err(|e| e.to_string())
    };
    // A permanent delete can fail halfway; only report success if it's really gone.
    match result {
        Ok(()) if path.exists() => Err("some items could not be removed".into()),
        other => other,
    }
}

fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

fn list_drives() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        (b'C'..=b'Z')
            .map(|c| PathBuf::from(format!("{}:\\", c as char)))
            .filter(|p| p.exists())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/")]
    }
}

fn main() {
    let initial = std::env::args_os().nth(1).map(PathBuf::from);

    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(if elevation::is_elevated() {
                        "DirScope (Administrator)".into()
                    } else {
                        "DirScope".into()
                    }),
                    ..Default::default()
                }),
                focus: true,
                // Matches assets/linux/dirscope.desktop so Linux docks show the icon.
                app_id: Some("dirscope".into()),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    let mut app = DirScope::new(cx);
                    if let Some(path) = initial {
                        app.start_scan(path, cx);
                    }
                    app
                });
                let focus = view.read(cx).focus.clone();
                window.focus(&focus);
                // Ask once the window has painted, not while it's still being built.
                let weak = view.downgrade();
                window.on_next_frame(move |window, cx| {
                    weak.update(cx, |this, cx| this.offer_elevation(window, cx))
                        .ok();
                });
                view
            },
        )
        .unwrap();

        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
