//! Compare files: choose the older version, then the Compare panel lists the differences
//! (replaced blue, inserted green, deleted red), shades them on the newer document's pages, and
//! offers a report or the differences as comments.

use egui::{Align, Color32, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::compare::{Comparison, Kind};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, RightPanel, widgets};

type CompareOutput = Result<(Comparison, Vec<(usize, [f64; 4])>), String>;

/// At most this many pages of both documents together may be compared on the UI thread (tests
/// and wasm run inline whatever the count). Counting pages, not bytes: comparing reads every word
/// and rasterizes every page, and a small file can hold hundreds of pages.
const INLINE_COMPARE_PAGES: usize = 8;

/// A document as a comparison read it: its bytes and edit generation. A result computed from
/// anything else describes pages as they were, and is dropped instead of shown.
pub(crate) struct Anchor {
    bytes: std::sync::Arc<Vec<u8>>,
    generation: u64,
}

impl Anchor {
    fn of(doc: &pdfcraft_engine::Document) -> Self {
        Self { bytes: doc.bytes.clone(), generation: doc.edit_generation() }
    }

    /// Whether `doc` is still the document this anchor was taken from (edited, saved, reloaded
    /// and reopened all give it new bytes or a new generation).
    fn holds(&self, doc: &pdfcraft_engine::Document) -> bool {
        self.generation == doc.edit_generation() && std::sync::Arc::ptr_eq(&self.bytes, &doc.bytes)
    }
}

/// A comparison running off the UI thread. Comparing two large files reads every word and
/// rasterizes every page; doing that while painting froze the window.
pub(crate) struct CompareJob {
    rx: std::sync::mpsc::Receiver<CompareOutput>,
    old: DocId,
    new: DocId,
    /// The documents as the worker read them: its result is dropped if either is not the same
    /// document any more when it arrives.
    old_anchor: Anchor,
    new_anchor: Anchor,
}

/// The last comparison.
pub struct CompareState {
    pub old: DocId,
    pub new: DocId,
    /// The documents as the comparison read them (see [`Anchor`]): while either keeps its
    /// identity, this result describes it and its shading stays.
    old_anchor: Anchor,
    new_anchor: Anchor,
    pub result: Comparison,
    /// Regions that look different (page, user-space box), page n against page n.
    pub visual: Vec<(usize, [f64; 4])>,
    pub selected: Option<usize>,
    /// Side by side: the old file on the left, the new on the right, scrolled and zoomed
    /// together.
    pub side_by_side: bool,
    /// Which half leads the other (the one the pointer was over last): 0 old, 1 new.
    pub leader: usize,
    /// Each half's scroll offset last frame, to tell which one moved.
    pub prev_scroll: [egui::Vec2; 2],
    /// Show: whether the text changes are in the list and the shading, and whether the visual
    /// differences (counted, never listed) are shaded (Acrobat's Filter).
    pub show_text: bool,
    pub show_visual: bool,
}

/// Visual differences are shaded orange.
pub const VISUAL: Color32 = Color32::from_rgb(0xF0, 0x8C, 0x1A);

/// What the panel asks for.
pub enum PanelAction {
    Select(usize),
    Mark,
    Report,
    Clear,
    SideBySide(bool),
    Show { text: bool, visual: bool },
}

pub fn colour(kind: Kind) -> Color32 {
    let [r, g, b] = pdfcraft_engine::compare::colour(kind);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The Compare Files dialog: pick the older document. Returns (compare, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    ui.label(egui::RichText::new(tl!("Compare Files")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let Some((_, new)) = app.active_ids() else { return (false, true) };
    let others: Vec<(DocId, String)> =
        app.views.iter().filter(|v| v.id != new).filter_map(|v| app.session.get(v.id).map(|d| (d.id, d.name.clone()))).collect();
    if app.compare_old.is_none_or(|o| !others.iter().any(|(id, _)| *id == o)) {
        app.compare_old = others.first().map(|(id, _)| *id);
    }
    let new_name = app.session.get(new).map(|d| d.name.clone()).unwrap_or_default();
    egui::Grid::new("compare-files").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label(tl!("Old file:"));
        let shown =
            others.iter().find(|(id, _)| Some(*id) == app.compare_old).map_or(tl!("Open the older version first").to_string(), |(_, n)| n.clone());
        egui::ComboBox::from_id_salt("compare-old").selected_text(shown).width(320.0).show_ui(ui, |ui| {
            for (id, n) in &others {
                ui.selectable_value(&mut app.compare_old, Some(*id), n);
            }
        });
        ui.end_row();
        ui.label(tl!("New file:"));
        ui.label(egui::RichText::new(new_name).strong());
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Text is compared word by word; the differences are shown on the new file.")).small().color(t.text_muted));
    ui.add_space(10.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(app.compare_old.is_some(), |ui| widgets::pill_button(ui, tl!("Compare"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

/// The Compare panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &Option<CompareState>, id: DocId) -> Option<PanelAction> {
    let Some(s) = state.as_ref().filter(|s| s.new == id) else {
        ui.label(egui::RichText::new(tl!("Compare this file with an older version: Compare files… in All tools.")).color(t.text_muted));
        return None;
    };
    let mut action = None;
    let c = &s.result;
    if c.identical() {
        ui.label(tl!("No differences in the text."));
    }
    ui.horizontal_wrapped(|ui| {
        for k in [Kind::Replaced, Kind::Inserted, Kind::Deleted] {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().rect_filled(dot, egui::CornerRadius::same(2), colour(k));
            ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &c.count(k).to_string()), ("kind", tl!(k.label()))]));
        }
        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(dot, egui::CornerRadius::same(2), VISUAL);
        ui.label(crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &s.visual.len().to_string()), ("kind", tl!("Visual"))]));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Show:"));
        let (mut text, mut visual) = (s.show_text, s.show_visual);
        let text_changed = ui.add(widgets::checkbox(&mut text, tl!("Text"))).changed();
        if ui.add(widgets::checkbox(&mut visual, tl!("Visual"))).changed() || text_changed {
            action = Some(PanelAction::Show { text, visual });
        }
    });
    ui.add_space(6.0);
    // Side by side, or the new file alone.
    ui.horizontal(|ui| {
        if widgets::pill_button(ui, tl!("Side by side"), s.side_by_side).clicked() {
            action = Some(PanelAction::SideBySide(true));
        }
        if widgets::pill_button(ui, tl!("New file only"), !s.side_by_side).clicked() {
            action = Some(PanelAction::SideBySide(false));
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button(tl!("Mark as comments")).clicked() {
            action = Some(PanelAction::Mark);
        }
        if ui.button(tl!("Report…")).clicked() {
            action = Some(PanelAction::Report);
        }
        if ui.button(tl!("Clear")).clicked() {
            action = Some(PanelAction::Clear);
        }
    });
    ui.separator();
    for (i, ch) in c.changes.iter().enumerate().filter(|_| s.show_text).take(2000) {
        let text = match ch.kind {
            Kind::Replaced => format!("\"{}\" → \"{}\"", ch.old.text, ch.new.text),
            Kind::Inserted => format!("+ \"{}\"", ch.new.text),
            Kind::Deleted => format!("− \"{}\"", ch.old.text),
        };
        let selected = s.selected == Some(i);
        let r = ui.push_id(i, |ui| {
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(4.0, 34.0), egui::Sense::hover());
                ui.painter().rect_filled(bar, egui::CornerRadius::same(2), colour(ch.kind));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt(
                            tl!("{kind} · page {p}"),
                            &[("kind", tl!(ch.kind.label())), ("p", &(ch.new.page + 1).to_string())],
                        ))
                        .small()
                        .color(t.text_muted),
                    );
                    ui.selectable_label(selected, text)
                })
                .inner
            })
            .inner
        });
        if r.inner.clicked() {
            action = Some(PanelAction::Select(i));
        }
    }
    action
}

impl PdfCraftApp {
    /// Set gaps for the active document and both visible comparison panes, leaving other tabs alone.
    pub(crate) fn set_active_page_gaps(&mut self, show: bool) {
        let Some((index, id)) = self.active_ids() else { return };
        let partner = side_by_side(self, index).and_then(|(old, _)| self.views.get(old).map(|v| v.id));
        for view in &mut self.views {
            if view.id == id || partner == Some(view.id) {
                view.set_page_gaps(show);
            }
        }
    }

    /// Compare the chosen older document with the active one and show the differences. The work
    /// runs on a worker thread; [`Self::poll_compare`] installs the result.
    pub fn run_compare(&mut self) {
        let Some((_, new)) = self.active_ids() else { return };
        let Some(old) = self.compare_old else { return };
        if self.compare_job.is_some() {
            self.notify_tr("A comparison is already running");
            return;
        }
        let (Some(a), Some(b)) = (self.session.get(old), self.session.get(new)) else { return };
        let (old_bytes, old_password) = (a.bytes.clone(), a.password.clone());
        let (new_bytes, new_password) = (b.bytes.clone(), b.password.clone());
        let (old_anchor, new_anchor) = (Anchor::of(a), Anchor::of(b));
        let pages = a.info.pages.len() + b.info.pages.len();
        let (tx, rx) = std::sync::mpsc::channel();
        let work = move || {
            let run = || -> CompareOutput {
                let mut session = pdfcraft_engine::Session::new();
                let old_id = session.open("old", None, old_bytes, old_password.as_deref()).map_err(|e| e.to_string())?;
                let new_id = session.open("new", None, new_bytes, new_password.as_deref()).map_err(|e| e.to_string())?;
                let result = session.compare(old_id, new_id).map_err(|e| e.to_string())?;
                let visual = session.compare_visual(old_id, new_id, 72.0).unwrap_or_default();
                Ok((result, visual))
            };
            let _ = tx.send(run());
        };
        // A comparison reads every word and rasterizes every page. Tests and wasm (no threads)
        // run it now; otherwise only documents of few pages in all may run it on the UI thread,
        // because file size says nothing about the work: a small file can hold hundreds of pages.
        let inline = cfg!(target_arch = "wasm32") || self.run_inline || pages <= INLINE_COMPARE_PAGES;
        if inline {
            work();
            self.compare_job = Some(CompareJob { rx, old, new, old_anchor, new_anchor });
            self.poll_compare();
        } else if std::thread::Builder::new().name("pdfcraft-compare".into()).spawn(work).is_err() {
            self.notify_tr("Could not start the comparison");
        } else {
            self.compare_job = Some(CompareJob { rx, old, new, old_anchor, new_anchor });
            self.notify_tr("Comparing…");
        }
    }

    /// Drop the comparison (and its shading) once either document is not the one it was computed
    /// from: its list and shading describe pages as they were. The differences "Mark as comments"
    /// writes are the one exception — they are comments on the new file, not changes to the
    /// compared text or pages — so [`Self::compare_action`] re-anchors the comparison after them.
    fn refresh_compare(&mut self) {
        let stale = match self.compare.as_ref() {
            None => return,
            Some(s) => {
                let old_gone = self.session.get(s.old).is_none_or(|d| !s.old_anchor.holds(d));
                let new_gone = self.session.get(s.new).is_none_or(|d| !s.new_anchor.holds(d));
                old_gone || new_gone
            }
        };
        if !stale {
            return;
        }
        let cleared = self.compare.take().map(|s| (s.old, s.new));
        if let Some((old, new)) = cleared {
            for id in [old, new] {
                if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
                    v.compare_marks.clear();
                }
            }
        }
    }

    /// Install a finished comparison, if one is ready.
    pub(crate) fn poll_compare(&mut self) {
        self.refresh_compare();
        let Some(job) = self.compare_job.as_ref() else { return };
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the comparison worker stopped".to_string()),
        };
        let Some(job) = self.compare_job.take() else { return };
        // The result was read from both documents as they were: it is dropped, and said to be,
        // once either is not that document any more (edited, saved, reloaded or closed).
        let current =
            self.session.get(job.old).is_some_and(|d| job.old_anchor.holds(d)) && self.session.get(job.new).is_some_and(|d| job.new_anchor.holds(d));
        if !current {
            self.notify_tr("The documents changed while the comparison was running, so its result was dropped.");
            return;
        }
        match result {
            Ok((result, visual)) => {
                let n = result.changes.len();
                // Side by side when the old version is open in a tab and the window has the room.
                let side_by_side = self.views.iter().any(|v| v.id == job.old) && self.ctx.as_ref().is_none_or(|c| c.content_rect().width() >= 1000.0);
                self.compare = Some(CompareState {
                    old: job.old,
                    new: job.new,
                    old_anchor: job.old_anchor,
                    new_anchor: job.new_anchor,
                    result,
                    visual,
                    selected: None,
                    side_by_side,
                    leader: 1,
                    prev_scroll: [egui::Vec2::ZERO; 2],
                    show_text: true,
                    show_visual: true,
                });
                // The shading lands on those documents wherever their tabs are now: a tab can be
                // closed or dragged elsewhere while the worker runs.
                self.shade_compare();
                self.right = Some(RightPanel::Compare);
                self.notify(if n == 0 {
                    tl!("The text is the same").to_string()
                } else if n == 1 {
                    tl!("1 difference found").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} differences found"), &[("n", &n.to_string())])
                });
            }
            Err(e) => self.notify(e),
        }
    }

    pub(crate) fn compare_action(&mut self, a: PanelAction) {
        let Some(s) = self.compare.as_ref() else { return };
        let (old, new) = (s.old, s.new);
        match a {
            PanelAction::Select(i) => {
                let Some(ch) = s.result.changes.get(i) else { return };
                let rect = ch.new.rects.first().copied().or(ch.new.near).unwrap_or_default();
                let page = ch.new.page;
                let old_target = (ch.old.page, ch.old.rects.first().copied().or(ch.old.near).unwrap_or_default());
                if let Some(s) = self.compare.as_mut() {
                    s.selected = Some(i);
                }
                if let Some(v) = self.views.iter_mut().find(|v| v.id == new) {
                    v.go_to_page(page);
                    v.flash = Some((page, rect.map(|x| x as f32), 0.0));
                }
                // The old file's half jumps to its side of the change too.
                if let Some(oi) = self.views.iter().position(|v| v.id == old) {
                    let v = &mut self.views[oi];
                    v.go_to_page(old_target.0);
                    v.flash = Some((old_target.0, old_target.1.map(|x| x as f32), 0.0));
                }
            }
            PanelAction::SideBySide(on) => {
                if let Some(s) = self.compare.as_mut() {
                    s.side_by_side = on;
                    s.leader = 1;
                }
            }
            PanelAction::Show { text, visual } => {
                if let Some(s) = self.compare.as_mut() {
                    s.show_text = text;
                    s.show_visual = visual;
                }
                self.shade_compare();
            }
            PanelAction::Mark => match self.session.mark_differences(old, new) {
                Ok(n) => {
                    let info = self.session.get(new).map(|d| d.info.clone());
                    if let (Some(info), Some(v)) = (info, self.views.iter_mut().find(|v| v.id == new)) {
                        v.document_changed(&info);
                    }
                    // The marks are comments on the new file: the compared text and pages are
                    // the same ones, so the comparison stays and follows this write of its own.
                    if let (Some(s), Some(doc)) = (self.compare.as_mut(), self.session.get(new)) {
                        s.new_anchor = Anchor::of(doc);
                    }
                    self.notify(if n == 1 {
                        tl!("Added 1 comment").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Added {n} comments"), &[("n", &n.to_string())])
                    });
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Report => match self.session.compare_report(old, new) {
                Ok(bytes) => {
                    if let Err(e) = self.open_bytes("Compare Report.pdf", None, bytes.to_vec()) {
                        self.notify_error(e);
                    }
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Clear => {
                self.compare = None;
                for id in [old, new] {
                    if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
                        v.compare_marks.clear();
                    }
                }
                self.right = None;
            }
        }
    }
}

impl PdfCraftApp {
    /// Shade the comparison on the new file's pages and, when it is open, the old file's, with
    /// the kinds of change the filter shows. Both are found by document, wherever their tabs are.
    fn shade_compare(&mut self) {
        let Some(s) = self.compare.as_ref() else { return };
        let (old, new) = (s.old, s.new);
        let (old_marks, new_marks) = (shading(s, true), shading(s, false));
        if let Some(v) = self.views.iter_mut().find(|v| v.id == new) {
            v.compare_marks = new_marks;
        }
        if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
            v.compare_marks = old_marks;
        }
    }
}

/// One half's shading: its side of the text changes (the old file has no side of an insertion)
/// and the visual differences no text change there covers, so both versions read with their own
/// shading. Hidden kinds are left out after the overlap test, so hiding the text does not uncover
/// the visual differences a text change explains.
fn shading(s: &CompareState, old: bool) -> Vec<(usize, [f32; 4], Color32)> {
    let overlaps = |p: usize, r: [f32; 4], marks: &[(usize, [f32; 4], Color32)]| {
        marks.iter().any(|(mp, m, _)| *mp == p && m[0] <= r[2] && r[0] <= m[2] && m[1] <= r[3] && r[1] <= m[3])
    };
    let mut text = Vec::new();
    for ch in s.result.changes.iter().filter(|ch| !(old && ch.kind == Kind::Inserted)) {
        let side = if old { &ch.old } else { &ch.new };
        text.extend(side.rects.iter().map(|r| (side.page, r.map(|v| v as f32), colour(ch.kind))));
    }
    let mut visual = Vec::new();
    for (p, r) in &s.visual {
        let r = r.map(|v| v as f32);
        // The old file's half also skips a region overlapping one it already shades.
        if !overlaps(*p, r, &text) && !(old && overlaps(*p, r, &visual)) {
            visual.push((*p, r, VISUAL));
        }
    }
    let mut out = if s.show_text { text } else { Vec::new() };
    if s.show_visual {
        out.extend(visual);
    }
    out
}

/// The two halves of a side-by-side comparison, as view indexes (old, new), when the active
/// document is the new file of a comparison shown side by side and the old file is open too.
pub(crate) fn side_by_side(app: &PdfCraftApp, index: usize) -> Option<(usize, usize)> {
    let c = app.compare.as_ref().filter(|c| c.side_by_side)?;
    let new = app.views.get(index).filter(|v| v.id == c.new)?;
    let old = app.views.iter().position(|v| v.id == c.old)?;
    (old != index).then_some((old, app.views.iter().position(|v| v.id == new.id)?))
}

/// Side by side: the old file on the left, the new on the right, each a page view of its own
/// with its shading, scrolled and zoomed together. The half the pointer is over leads; the
/// other follows its zoom, layout and scroll offset.
pub(crate) fn compare_area(app: &mut PdfCraftApp, old: usize, new: usize, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let rect = ui.available_rect_before_wrap();
    let gap = 1.0;
    let half_w = ((rect.width() - gap) / 2.0).max(1.0);
    let caption_h = 22.0;
    let left = egui::Rect::from_min_size(rect.min, egui::vec2(half_w, rect.height()));
    let right = egui::Rect::from_min_size(egui::pos2(rect.min.x + half_w + gap, rect.min.y), egui::vec2(half_w, rect.height()));
    // Who leads: the half under the pointer (remembered), the new file at first.
    if let Some(p) = ui.input(|i| i.pointer.hover_pos())
        && let Some(c) = app.compare.as_mut()
    {
        if left.contains(p) {
            c.leader = 0;
        } else if right.contains(p) {
            c.leader = 1;
        }
    }
    let leader = app.compare.as_ref().map_or(1, |c| c.leader);
    let (lead, follow) = if leader == 0 { (old, new) } else { (new, old) };
    // The follower takes the leader's zoom and layout before either is drawn.
    let (zoom, fit, layout, continuous, page_gaps, rotation) = {
        let v = &app.views[lead];
        (v.zoom, v.fit, v.layout, v.continuous, v.show_page_gaps, v.rotation)
    };
    {
        let v = &mut app.views[follow];
        if v.zoom != zoom
            || v.fit != fit
            || v.layout != layout
            || v.continuous != continuous
            || v.show_page_gaps != page_gaps
            || v.rotation != rotation
        {
            v.zoom = zoom;
            v.fit = fit;
            v.layout = layout;
            v.continuous = continuous;
            v.show_page_gaps = page_gaps;
            v.rotation = rotation;
        }
    }
    let names = [old, new].map(|i| app.session.get(app.views[i].id).map(|d| d.name.clone()).unwrap_or_default());
    // Drawing a half can close its own tab (its snapshot bar's Close, #3915), which shifts or
    // removes the indexes below. Each half is found again by id, and the frame stops when one is gone.
    let ids = [app.views[old].id, app.views[new].id];
    let find = |app: &PdfCraftApp, id| app.views.iter().position(|v| v.id == id);
    ui.painter().vline(rect.min.x + half_w, rect.y_range(), egui::Stroke::new(gap, t.border));
    for (k, r) in [left, right].into_iter().enumerate() {
        // Both halves must still be open: once either has closed, the pair is over.
        let (Some(old_index), Some(new_index)) = (find(app, ids[0]), find(app, ids[1])) else { return };
        let index = if k == 0 { old_index } else { new_index };
        let caption = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), caption_h));
        ui.painter().rect_filled(caption, 0.0, t.panel);
        let label = crate::i18n::fmt(if k == 0 { tl!("Old file: {name}") } else { tl!("New file: {name}") }, &[("name", &names[k])]);
        ui.painter().text(
            egui::pos2(caption.min.x + 10.0, caption.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            t.text_muted,
        );
        let body = egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y + caption_h), r.max);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("compare-half", k)));
        child.set_clip_rect(body);
        crate::canvas::document_area(app, index, &mut child);
    }
    // After both drew: the leader's scroll, when it moved, is the follower's next offset.
    let (Some(old), Some(new)) = (find(app, ids[0]), find(app, ids[1])) else { return };
    let follow = if leader == 0 { new } else { old };
    let offsets = [app.views[old].last_scroll, app.views[new].last_scroll];
    if let Some(c) = app.compare.as_mut() {
        let lead_k = if leader == 0 { 0 } else { 1 };
        if (offsets[lead_k] - c.prev_scroll[lead_k]).length() > 0.5 && (offsets[1 - lead_k] - offsets[lead_k]).length() > 0.5 {
            app.views[follow].sync_scroll = Some(offsets[lead_k]);
            ui.ctx().request_repaint();
        }
        c.prev_scroll = offsets;
    }
}

#[cfg(test)]
mod tests {
    use super::{Anchor, CompareJob, CompareState, INLINE_COMPARE_PAGES, PanelAction};
    use crate::PdfCraftApp;
    use pdfcraft_engine::compare::Comparison;

    /// `n` pages of one word each: small on disk, but every page is work for a comparison, which
    /// reads every word and rasterizes every page.
    fn pages(n: usize) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Count {n} /Kids [{}] >>", (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        for i in 0..n {
            let content = format!("BT /F1 18 Tf 72 700 Td (Page {}) Tj ET", i + 1);
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
                5 + 2 * i
            ));
            objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// Two versions of one document that differ in one word.
    fn versions() -> (Vec<u8>, Vec<u8>) {
        let s = pdfcraft_engine::Session::new();
        (
            s.create_from_text("t", "Delivery within five days.").unwrap().to_vec(),
            s.create_from_text("t", "Delivery within three days.").unwrap().to_vec(),
        )
    }

    /// The comparison of two versions, as the worker thread computes it.
    fn comparison(old: &[u8], new: &[u8]) -> (Comparison, Vec<(usize, [f64; 4])>) {
        let mut session = pdfcraft_engine::Session::new();
        let a = session.open("old", None, old.to_vec().into(), None).unwrap();
        let b = session.open("new", None, new.to_vec().into(), None).unwrap();
        (session.compare(a, b).unwrap(), Vec::new())
    }

    fn app_with(old: Vec<u8>, new: Vec<u8>) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, old).unwrap();
        app.open_bytes("new.pdf", None, new).unwrap();
        app.compare_old = Some(app.views[0].id);
        app
    }

    /// A result that arrives after either document changed describes pages as they were: it is
    /// dropped, and said to be, instead of shown against the documents as they are.
    #[test]
    fn a_result_that_arrives_after_the_documents_changed_is_dropped() {
        let (old, new) = versions();
        let mut app = app_with(old.clone(), new.clone());
        let (old_id, new_id) = (app.views[0].id, app.views[1].id);
        // The worker's result is in flight while the new document is edited.
        let (tx, rx) = std::sync::mpsc::channel();
        app.compare_job = Some(CompareJob {
            rx,
            old: old_id,
            new: new_id,
            old_anchor: Anchor::of(app.session.get(old_id).unwrap()),
            new_anchor: Anchor::of(app.session.get(new_id).unwrap()),
        });
        assert!(app.apply_edit_in(new_id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        let _ = tx.send(Ok(comparison(&old, &new)));
        app.poll_compare();
        assert!(app.compare.is_none(), "the result describes the document as it was");
    }

    /// A result lands on the documents it was computed from, wherever their tabs are by then:
    /// a tab can be dragged elsewhere or closed while the worker runs, and a tab index captured
    /// at the start would shade an unrelated tab (or none).
    #[test]
    fn a_result_lands_on_its_documents_wherever_their_tabs_are() {
        let (old, new) = versions();
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, old.clone()).unwrap();
        app.open_bytes("other.pdf", None, pdfcraft_engine::Session::new().create_from_text("t", "Unrelated.").unwrap().to_vec()).unwrap();
        app.open_bytes("new.pdf", None, new.clone()).unwrap();
        let (old_id, new_id) = (app.views[0].id, app.views[2].id);
        let (tx, rx) = std::sync::mpsc::channel();
        app.compare_job = Some(CompareJob {
            rx,
            old: old_id,
            new: new_id,
            old_anchor: Anchor::of(app.session.get(old_id).unwrap()),
            new_anchor: Anchor::of(app.session.get(new_id).unwrap()),
        });
        // The tabs move and one closes while the comparison runs: [new, old, other] → [new, old].
        app.move_tab(2, 0);
        app.close_tab(2);
        let _ = tx.send(Ok(comparison(&old, &new)));
        app.poll_compare();
        let c: &CompareState = app.compare.as_ref().expect("both documents are unchanged, so the comparison is kept");
        assert_eq!((c.old, c.new), (old_id, new_id));
        assert!(!app.views.iter().find(|v| v.id == old_id).unwrap().compare_marks.is_empty(), "shaded on its old file");
        assert!(!app.views.iter().find(|v| v.id == new_id).unwrap().compare_marks.is_empty(), "and on its new file");
        assert_eq!(app.views.len(), 2);
    }

    /// Once either document changes, the comparison is not about them any more: it and its
    /// shading go.
    #[test]
    fn a_comparison_is_dropped_when_either_document_changes() {
        let (old, new) = versions();
        let mut app = app_with(old, new);
        app.run_compare();
        let (old_id, new_id) = {
            let c = app.compare.as_ref().expect("a document of few pages is compared on the UI thread");
            (c.old, c.new)
        };
        assert!(app.views.iter().any(|v| !v.compare_marks.is_empty()));
        app.apply_edit_in(new_id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
        app.poll_compare();
        assert!(app.compare.is_none());
        assert!(app.views.iter().all(|v| v.compare_marks.is_empty()), "and the shading goes with it");
        assert!(app.session.get(old_id).is_some());
    }

    /// "Mark as comments" writes comments on the new file: the compared text and pages are the
    /// same ones, so the comparison stays and follows this write of its own.
    #[test]
    fn marking_the_differences_keeps_the_comparison() {
        let (old, new) = versions();
        let mut app = app_with(old, new);
        app.run_compare();
        assert!(app.compare.is_some());
        app.compare_action(PanelAction::Mark);
        app.poll_compare();
        assert!(app.compare.is_some(), "comments do not change the compared text or pages");
    }

    /// A small file can hold hundreds of pages, and comparing reads every word and rasterizes
    /// every page: only documents of few pages run on the UI thread. A larger one runs off it
    /// and lands later, so the window stays responsive.
    #[test]
    fn a_many_page_comparison_runs_off_the_ui_thread() {
        const { assert!(INLINE_COMPARE_PAGES < 20) };
        let mut app = app_with(pages(20), pages(20));
        app.run_compare();
        assert!(app.compare.is_none(), "still running: not on the UI thread");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while app.compare.is_none() {
            app.poll_compare();
            assert!(std::time::Instant::now() < deadline, "the comparison never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // Few pages in all: the work is done before the first repaint.
        let mut small = app_with(pages(2), pages(2));
        small.run_compare();
        assert!(small.compare.is_some(), "small work runs inline");
    }
}
