//! Prepare for accessibility ▸ Check for accessibility: the Accessibility Checker Options dialog
//! and the Accessibility Checker panel (results by category, with Fix, Skip rule, Explain,
//! Check again and Show report on each rule).

use std::collections::BTreeSet;

use egui::{Align, Layout};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::DocId;
use pdfcraft_engine::a11y::{Category, Options, Report, Rule, Status};

use crate::theme::{self, Tokens};
use crate::{Dialog, PdfCraftApp, PropsTab, RightPanel, icons, widgets};

/// The options dialog's settings (kept for the session, like Acrobat's).
#[derive(Clone, Debug, PartialEq)]
pub struct A11yOptions {
    pub rules: BTreeSet<Rule>,
    pub all_pages: bool,
    /// 1-based page range when not all pages.
    pub from: usize,
    pub to: usize,
    /// Save the accessibility report when checking finishes.
    pub create_report: bool,
    /// Show the options when the checker starts.
    pub show_dialog: bool,
    /// The category whose rules the dialog lists.
    pub category: Category,
}

impl Default for A11yOptions {
    fn default() -> Self {
        Self {
            rules: Options::default().rules,
            all_pages: true,
            from: 1,
            to: 1,
            create_report: false,
            show_dialog: true,
            category: Category::Document,
        }
    }
}

/// The last check of a document, as the panel shows it.
#[derive(Clone, Debug, Default)]
pub struct A11yState {
    pub report: Option<(DocId, Report)>,
    /// The edit generation the report was computed from. A report of an older generation
    /// describes the document as it was before its edits, and the panel says so.
    pub report_generation: Option<u64>,
    /// The prior check of the same document with the same checker options.
    pub previous: Option<(DocId, Report)>,
    pub options: Option<Options>,
    /// A check is running (on the worker thread; the panel shows it).
    pub checking: bool,
    /// The saved report was asked for while the check that produces it is still running.
    pub(crate) report_when_done: bool,
    /// Collapsed categories and expanded rules.
    pub collapsed: BTreeSet<Category>,
    pub expanded: BTreeSet<Rule>,
    /// The rule whose explanation is showing.
    pub explain: Option<Rule>,
}

/// At most this many pages may be checked on the UI thread (tests and wasm run inline whatever
/// the count). Checking reads every page's text and tags, and a small file can hold hundreds of
/// pages.
const INLINE_A11Y_PAGES: usize = 8;

/// A document as a check read it: its bytes and edit generation. A result computed from
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

/// An accessibility check running off the UI thread. Checking a large document reads every
/// page's text and tags; doing that while painting froze the window.
pub struct A11yJob {
    rx: std::sync::mpsc::Receiver<Option<Report>>,
    id: DocId,
    options: Options,
    /// The document as the worker read it: its result is dropped if it is not the same
    /// document any more when it arrives.
    anchor: Anchor,
}

/// The check itself, off the UI thread: the editor's document model is not shareable, so the
/// worker reopens the working file and checks that. `None` when it could not be read.
fn check_report(bytes: std::sync::Arc<Vec<u8>>, name: String, password: Option<String>, options: Options) -> Option<Report> {
    let mut session = pdfcraft_engine::Session::new();
    let doc = session.open(name, None, bytes, password.as_deref()).ok()?;
    session.get(doc)?.accessibility_check(&options)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PanelAction {
    Fix(Rule),
    Skip(Rule),
    CheckAgain,
    ShowReport,
    Options,
    GoTo(usize, Option<[f64; 4]>),
    GoToTag(ObjRef, Option<usize>, Option<[f64; 4]>),
}

/// Rules with an automatic (or guided) fix.
pub(crate) fn fixable(rule: Rule) -> bool {
    matches!(rule, Rule::PrimaryLanguage | Rule::Title | Rule::TabOrder)
}

pub(crate) fn options_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
    let o = &mut app.a11y_options;
    o.to = o.to.clamp(1, pages);
    o.from = o.from.clamp(1, o.to);
    ui.label(egui::RichText::new(tl!("Accessibility Checker Options")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };
    group(ui, "Report Options", &mut |ui| {
        ui.add(widgets::checkbox(&mut o.create_report, tl!("Create accessibility report")));
    });
    group(ui, "Page Range", &mut |ui| {
        ui.horizontal(|ui| {
            widgets::radio_value(ui, &mut o.all_pages, true, tl!("All pages in document"));
            widgets::radio_value(ui, &mut o.all_pages, false, tl!("Pages from"));
            ui.add_enabled(!o.all_pages, egui::DragValue::new(&mut o.from).range(1..=pages));
            ui.label(tl!("to"));
            ui.add_enabled(!o.all_pages, egui::DragValue::new(&mut o.to).range(1..=pages));
        });
    });
    let on = o.rules.len();
    group(ui, &crate::i18n::fmt(tl!("Checking Options ({on} of 32 in all categories)"), &[("on", &on.to_string())]), &mut |ui| {
        ui.horizontal(|ui| {
            ui.label(tl!("Category:"));
            egui::ComboBox::from_id_salt("a11y-category").selected_text(tl!(o.category.label())).width(260.0).show_ui(ui, |ui| {
                for c in Category::ALL {
                    ui.selectable_value(&mut o.category, c, tl!(c.label()));
                }
            });
        });
        ui.add_space(4.0);
        for r in Rule::ALL.into_iter().filter(|r| r.category() == o.category) {
            let mut checked = o.rules.contains(&r);
            if ui.add(widgets::checkbox(&mut checked, tl!(r.description()))).changed() {
                if checked {
                    o.rules.insert(r);
                } else {
                    o.rules.remove(&r);
                }
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button(tl!("Select All")).clicked() {
                o.rules.extend(Rule::ALL.into_iter().filter(|r| r.category() == o.category));
            }
            if ui.button(tl!("Clear All")).clicked() {
                let c = o.category;
                o.rules.retain(|r| r.category() != c);
            }
        });
    });
    ui.add(widgets::checkbox(&mut o.show_dialog, tl!("Show this dialog when the Checker starts")));
    ui.add_space(10.0);
    let (mut start, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Start Checking"), true).clicked() {
                start = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (start, cancel)
}

/// The red of failed checks (as for invalid signatures).
const FAILED: egui::Color32 = egui::Color32::from_rgb(0xD7, 0x37, 0x3F);

fn status_icon(t: &Tokens, s: Status) -> (&'static str, egui::Color32) {
    match s {
        Status::Passed => ("circle-check", egui::Color32::from_rgb(0x2D, 0x9D, 0x4F)),
        Status::Failed => ("circle-x", FAILED),
        Status::Manual => ("circle-help", egui::Color32::from_rgb(0xD9, 0x8E, 0x04)),
        Status::Skipped => ("circle-dashed", t.text_faint),
    }
}

/// The Accessibility Checker panel.
pub(crate) fn panel(ui: &mut egui::Ui, t: &Tokens, state: &mut A11yState, doc: DocId, generation: u64) -> Option<PanelAction> {
    let mut action = None;
    let Some((_, report)) = state.report.as_ref().filter(|(d, _)| *d == doc) else {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(icons::image("accessibility", 32.0, t.text_faint));
            ui.add_space(6.0);
            if state.checking {
                ui.label(egui::RichText::new(tl!("Checking…")).color(t.text_muted));
            } else {
                ui.label(egui::RichText::new(tl!("This document hasn't been checked yet.")).color(t.text_muted));
                ui.add_space(6.0);
                if widgets::pill_button(ui, tl!("Check for accessibility"), true).clicked() {
                    action = Some(PanelAction::Options);
                }
            }
        });
        return action;
    };
    // An older report describes the document as it was when it was checked, not as it stands.
    let stale = state.report_generation.is_none_or(|g| g != generation);
    let failed = report.count(Status::Failed);
    let manual = report.count(Status::Manual);
    let skipped = report.count(Status::Skipped);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(if failed == 0 && manual == 0 && skipped == 0 {
                tl!("No issues found").to_string()
            } else if failed == 1 {
                tl!("1 issue").to_string()
            } else if failed > 1 {
                crate::i18n::fmt(tl!("{n} issues"), &[("n", &failed.to_string())])
            } else if manual > 0 {
                format!("{}: {manual}", tl!("Needs manual check"))
            } else {
                format!("{}: {skipped}", tl!("Skipped"))
            })
            .color(t.text_muted),
        );
        if state.checking {
            ui.label(egui::RichText::new(tl!("Checking…")).small().color(t.text_muted));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Check again"), false).clicked() {
                action = Some(PanelAction::CheckAgain);
            }
            if widgets::pill_button(ui, tl!("Report"), false).on_hover_text(tl!("Show the accessibility report")).clicked() {
                action = Some(PanelAction::ShowReport);
            }
        });
    });
    if stale {
        ui.label(
            egui::RichText::new(tl!("The document has changed since it was checked. Check again for current results.")).small().color(t.text_muted),
        );
    }
    if failed > 0 && manual > 0 {
        ui.label(egui::RichText::new(format!("{}: {manual}", tl!("Needs manual check"))).small().color(t.text_muted));
    }
    if let Some((_, previous)) = state.previous.as_ref().filter(|(d, _)| *d == doc) {
        egui::CollapsingHeader::new(tl!("Compare")).default_open(true).show(ui, |ui| {
            for status in [Status::Passed, Status::Failed, Status::Manual, Status::Skipped] {
                ui.label(format!("{}: {} → {}", tl!(status.label()), previous.count(status), report.count(status)));
            }
            for current in &report.results {
                let Some(prior) = previous.result(current.rule) else { continue };
                if prior.status != current.status || prior.findings.len() != current.findings.len() {
                    ui.label(format!(
                        "{}: {} ({}) → {} ({})",
                        tl!(current.rule.name()),
                        tl!(prior.status.label()),
                        prior.findings.len(),
                        tl!(current.status.label()),
                        current.findings.len()
                    ));
                }
            }
        });
    }
    ui.add_space(6.0);
    for c in Category::ALL {
        let rules: Vec<_> = report.in_category(c).collect();
        let issues = rules.iter().filter(|r| r.status == Status::Failed).count();
        let open = !state.collapsed.contains(&c);
        let head = if issues == 0 {
            tl!(c.label()).to_string()
        } else if issues == 1 {
            crate::i18n::fmt(tl!("{label} (1 issue)"), &[("label", tl!(c.label()))])
        } else {
            crate::i18n::fmt(tl!("{label} ({n} issues)"), &[("label", tl!(c.label())), ("n", &issues.to_string())])
        };
        let clicked = ui
            .horizontal(|ui| {
                let toggle = icons::button(
                    ui,
                    if open { "chevron-down" } else { "chevron-right" },
                    20.0,
                    false,
                    if open { tl!("Collapse") } else { tl!("Expand") },
                )
                .clicked();
                let color = if issues > 0 { FAILED } else { t.text };
                let l = ui.add(egui::Label::new(egui::RichText::new(&head).font(theme::semibold(13.0)).color(color)).sense(egui::Sense::click()));
                toggle || l.clicked()
            })
            .inner;
        if clicked {
            if open {
                state.collapsed.insert(c);
            } else {
                state.collapsed.remove(&c);
            }
        }
        if !open {
            continue;
        }
        for r in rules {
            let (icon, color) = status_icon(t, r.status);
            let row = ui
                .horizontal(|ui| {
                    ui.add_space(24.0);
                    ui.add(icons::image(icon, 15.0, color));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("{rule} - {status}"),
                                &[("rule", tl!(r.rule.name())), ("status", tl!(r.status.label()))],
                            ))
                            .font(theme::regular(12.5))
                            .color(t.text),
                        )
                        .sense(egui::Sense::click()),
                    )
                })
                .inner;
            if row.clicked() && !r.findings.is_empty() && !state.expanded.remove(&r.rule) {
                state.expanded.insert(r.rule);
            }
            row.context_menu(|ui| {
                ui.set_min_width(170.0);
                if ui.add_enabled(fixable(r.rule) && r.status == Status::Failed, egui::Button::new(tl!("Fix"))).clicked() {
                    action = Some(PanelAction::Fix(r.rule));
                    ui.close();
                }
                if ui.add_enabled(r.status != Status::Skipped, egui::Button::new(tl!("Skip Rule"))).clicked() {
                    action = Some(PanelAction::Skip(r.rule));
                    ui.close();
                }
                if ui.button(tl!("Explain")).clicked() {
                    state.explain = if state.explain == Some(r.rule) { None } else { Some(r.rule) };
                    ui.close();
                }
                ui.separator();
                if ui.button(tl!("Check Again")).clicked() {
                    action = Some(PanelAction::CheckAgain);
                    ui.close();
                }
                if ui.button(tl!("Show Report")).clicked() {
                    action = Some(PanelAction::ShowReport);
                    ui.close();
                }
                if ui.button(tl!("Options…")).clicked() {
                    action = Some(PanelAction::Options);
                    ui.close();
                }
            });
            if state.explain == Some(r.rule) {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
                    ui.label(egui::RichText::new(tl!(r.rule.explanation())).small().color(t.text_muted));
                });
            }
            if state.expanded.contains(&r.rule) {
                egui::Frame::NONE.inner_margin(egui::Margin { left: 46, right: 4, top: 0, bottom: 6 }).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for f in &r.findings {
                        // The checker hands over the sentence and the document's own words apart, so
                        // the sentence is said in the panel's language with those words still in it.
                        let said = f.said(&crate::i18n::words);
                        let l = ui.add(egui::Label::new(egui::RichText::new(said).small().color(t.text_muted)).sense(egui::Sense::click()));
                        if let Some(object) = f.object {
                            if l.on_hover_text(tl!("Tags")).clicked() {
                                action = Some(PanelAction::GoToTag(object, f.page, f.bbox));
                            }
                        } else if let Some(p) = f.page
                            && l.on_hover_text(tl!("Go to the page")).clicked()
                        {
                            action = Some(PanelAction::GoTo(p, f.bbox));
                        }
                    }
                });
            }
        }
    }
    action
}

impl PdfCraftApp {
    /// Prepare for accessibility ▸ Find untagged content.
    pub(crate) fn find_untagged_content(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let options = Options { rules: [Rule::TaggedContent, Rule::TaggedAnnotations, Rule::TaggedFormFields].into_iter().collect(), pages: None };
        let Some(report) = doc.accessibility_check(&options) else {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        };
        let failed = report.count(Status::Failed);
        self.a11y.previous = None;
        self.a11y.options = Some(options);
        self.a11y.report = Some((id, report));
        self.a11y.report_generation = Some(doc.edit_generation());
        self.right = Some(RightPanel::Accessibility);
        self.notify(if failed == 0 {
            tl!("No untagged content was found.").to_string()
        } else {
            crate::i18n::fmt(tl!("Found {n} untagged content problems."), &[("n", &failed.to_string())])
        });
    }

    /// Check for accessibility: the options first, unless they are turned off.
    pub(crate) fn start_accessibility_check(&mut self) {
        if self.a11y_options.show_dialog {
            self.dialog = Some(Dialog::AccessibilityOptions);
        } else {
            self.run_accessibility_check();
        }
    }

    /// Run the full check with the current options and show the results. A document of many
    /// pages is checked on a worker thread and lands through [`Self::poll_a11y`], so the
    /// window stays responsive; a check of a few pages is done before the frame is out.
    pub fn run_accessibility_check(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let o = &self.a11y_options;
        // The range was picked on whatever document had the options dialog last; with the dialog
        // turned off it reaches here unchecked, and a page past the end would select nothing.
        let to = o.to.clamp(1, doc.info.pages.len().max(1));
        let from = o.from.clamp(1, to);
        let options = Options { rules: o.rules.clone(), pages: (!o.all_pages).then(|| (from - 1..to).collect()) };
        if !doc.can_accessibility_check() {
            self.notify_tr("This document can't be checked: it couldn't be read for editing");
            return;
        }
        self.right = Some(RightPanel::Accessibility);
        let pages = options.pages.as_ref().map_or(doc.info.pages.len(), Vec::len);
        let inline = cfg!(target_arch = "wasm32") || self.run_inline || pages <= INLINE_A11Y_PAGES;
        if inline {
            match doc.accessibility_check(&options) {
                Some(report) => self.install_a11y_report(id, options, report),
                None => self.notify_tr("This document can't be checked: it couldn't be read for editing"),
            }
            return;
        }
        // A check that is still running is replaced: the newer request wins, and the older
        // worker's answer is dropped as stale when it arrives.
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = self.ctx.clone();
        let (work_bytes, work_name, work_password, work_options) = (doc.bytes.clone(), doc.name.clone(), doc.password.clone(), options.clone());
        let spawned = std::thread::Builder::new().name("pdfcraft-a11y".into()).spawn(move || {
            let report = check_report(work_bytes, work_name, work_password, work_options);
            // The receiver may be gone (a newer request replaced this one): nothing to report.
            let _ = tx.send(report);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        if spawned.is_err() {
            self.notify_tr("Could not start the accessibility check");
            return;
        }
        self.a11y_job = Some(A11yJob { rx, id, options, anchor: Anchor::of(doc) });
        self.a11y.checking = true;
    }

    /// Put a finished check in the panel: the current report becomes the previous one when the
    /// checker options did not change, the summary is announced, and the saved report is
    /// written when it was asked for.
    fn install_a11y_report(&mut self, id: DocId, options: Options, mut report: Report) {
        // Rules skipped by hand stay skipped when checking again.
        for r in report.results.iter_mut().filter(|r| self.a11y_skipped.contains(&(id, r.rule))) {
            r.status = Status::Skipped;
            r.findings.clear();
        }
        let failed = report.count(Status::Failed);
        let manual = report.count(Status::Manual);
        let skipped = report.count(Status::Skipped);
        self.a11y.previous =
            if self.a11y.options.as_ref() == Some(&options) { self.a11y.report.take().filter(|(prior_id, _)| *prior_id == id) } else { None };
        self.a11y.options = Some(options);
        self.a11y.report = Some((id, report));
        self.a11y.report_generation = self.session.get(id).map(|d| d.edit_generation());
        self.a11y.checking = false;
        self.right = Some(RightPanel::Accessibility);
        self.notify(if failed == 0 && manual == 0 && skipped == 0 {
            tl!("The checker found no problems in this document.").to_string()
        } else if failed == 1 {
            tl!("The checker found 1 problem which may prevent the document from being fully accessible.").to_string()
        } else if failed > 1 {
            crate::i18n::fmt(
                tl!("The checker found {n} problems which may prevent the document from being fully accessible."),
                &[("n", &failed.to_string())],
            )
        } else if manual > 0 {
            format!("{}: {manual}", tl!("Needs manual check"))
        } else {
            format!("{}: {skipped}", tl!("Skipped"))
        });
        if self.a11y_options.create_report || std::mem::take(&mut self.a11y.report_when_done) {
            self.show_accessibility_report();
        }
    }

    /// Install the result of a background check, if it has arrived and still describes the
    /// document it was asked about. Called once a frame while a check is pending.
    pub(crate) fn poll_a11y(&mut self) {
        use std::sync::mpsc::TryRecvError;
        let answer = match self.a11y_job.as_ref() {
            Some(job) => job.rx.try_recv(),
            None => return,
        };
        match answer {
            Ok(report) => {
                let job = self.a11y_job.take();
                if let (Some(job), Some(report)) = (job, report)
                    && let Some(doc) = self.session.get(job.id)
                    && job.anchor.holds(doc)
                {
                    self.install_a11y_report(job.id, job.options, report);
                } else {
                    // The document was edited, saved, reloaded or closed while the check ran:
                    // its answer describes pages as they were, so it is dropped, and the
                    // report it was to produce with it.
                    self.a11y.checking = false;
                    self.a11y.report_when_done = false;
                }
            }
            Err(TryRecvError::Empty) => {
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
            Err(TryRecvError::Disconnected) => {
                // The worker is gone without an answer (it could not be started, or it could
                // not reopen the file): the check is over, with nothing to show.
                self.a11y_job = None;
                self.a11y.checking = false;
            }
        }
    }

    /// Save the accessibility report (HTML) for the last check, checking first if needed.
    pub fn show_accessibility_report(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        if !self.a11y.report.as_ref().is_some_and(|(d, _)| *d == id) {
            let create = std::mem::replace(&mut self.a11y_options.create_report, false);
            self.run_accessibility_check();
            self.a11y_options.create_report = create;
            // The check is still running on the worker thread: write the report when it lands.
            self.a11y.report_when_done |= self.a11y.checking;
        }
        let Some((_, report)) = self.a11y.report.as_ref().filter(|(d, _)| *d == id) else { return };
        let Some(doc) = self.session.get(id) else { return };
        let (y, m, d) = self.session.today();
        // The saved report reads the way the interface reads, down to its `lang` attribute.
        let lang = crate::i18n::current();
        let html = pdfcraft_engine::a11y::report_html(report, &doc.name, &format!("{y}-{m:02}-{d:02}"), lang.code(), &crate::i18n::words);
        let stem = doc.name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let title = tl!("Save the accessibility report").to_string();
        self.write_files(&[(format!("{stem} Accessibility Report.html"), std::sync::Arc::new(html.into_bytes()))], &title);
    }

    /// Turn to a finding's page and, when the checker located it, flash its box there.
    fn a11y_show_on_page(&mut self, index: usize, page: usize, bbox: Option<[f64; 4]>) {
        let Some(view) = self.views.get_mut(index) else { return };
        view.go_to_page(page);
        if let Some(bbox) = bbox {
            view.flash = Some((page, bbox.map(|v| v as f32), 0.0));
        }
    }

    pub(crate) fn a11y_action(&mut self, index: usize, action: PanelAction) {
        match action {
            PanelAction::CheckAgain => self.run_accessibility_check(),
            PanelAction::Options => self.dialog = Some(Dialog::AccessibilityOptions),
            PanelAction::ShowReport => self.show_accessibility_report(),
            PanelAction::GoTo(p, bbox) => self.a11y_show_on_page(index, p, bbox),
            PanelAction::GoToTag(object, page, bbox) => {
                let Some((_, id)) = self.active_ids() else { return };
                let Some(tags) = self.session.get(id).map(|doc| self.tags_state.tags(doc)) else { return };
                if !tags.iter().any(|tag| tag.object == object) {
                    if let Some(page) = page {
                        self.a11y_show_on_page(index, page, bbox);
                    }
                    return;
                }
                if self.tags_state.select(id, &tags, object) {
                    self.right = Some(RightPanel::Tags);
                    if let Some(page) = page {
                        self.a11y_show_on_page(index, page, bbox);
                    }
                } else {
                    self.notify_tr("Save or revert changes before selecting another tag.");
                }
            }
            PanelAction::Skip(rule) => {
                if let Some((id, r)) = self.a11y.report.as_mut() {
                    self.a11y_skipped.insert((*id, rule));
                    if let Some(x) = r.results.iter_mut().find(|x| x.rule == rule) {
                        x.status = Status::Skipped;
                        x.findings.clear();
                    }
                }
            }
            PanelAction::Fix(rule) => {
                let Some((_, id)) = self.active_ids() else { return };
                match rule {
                    // The language is chosen in Document Properties ▸ Advanced.
                    Rule::PrimaryLanguage => self.dialog = Some(Dialog::Properties(PropsTab::Advanced)),
                    _ => {
                        let edit = self.session.get(id).map(|d| d.accessibility_fix(rule, None));
                        match edit {
                            Some(Ok(e)) => {
                                if !self.apply_edit(e) {
                                    return;
                                }
                                self.run_accessibility_check();
                                // The title can then be edited.
                                if rule == Rule::Title {
                                    self.dialog = Some(Dialog::Properties(PropsTab::Description));
                                }
                            }
                            Some(Err(e)) => self.notify_error(e),
                            None => {}
                        }
                    }
                }
            }
        }
    }
}

/// Add alternate text: the figures one by one.
#[derive(Clone, Default)]
pub struct AltDraft {
    pub doc: Option<DocId>,
    pub figures: Vec<pdfcraft_engine::a11y::Figure>,
    pub index: usize,
    pub texts: Vec<String>,
    pub decorative: Vec<bool>,
    /// The figure shown (index, picture).
    preview: Option<(usize, egui::TextureHandle)>,
}

impl std::fmt::Debug for AltDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AltDraft").field("figures", &self.figures.len()).field("index", &self.index).finish()
    }
}

/// Render a figure's area for the dialog, at the preview pane's device resolution.
fn figure_picture(ctx: &egui::Context, doc: &pdfcraft_engine::Document, f: &pdfcraft_engine::a11y::Figure) -> Option<egui::TextureHandle> {
    let (page, b) = (f.page?, f.bbox?);
    let info = doc.info.pages.get(page)?;
    let (u, v) = (info.user_to_view(b[0] as f32, b[1] as f32), info.user_to_view(b[2] as f32, b[3] as f32));
    let (x0, y0, x1, y1) = (u[0].min(v[0]), u[1].min(v[1]), u[0].max(v[0]), u[1].max(v[1]));
    let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
    let ppp = ctx.pixels_per_point().max(1.0);
    let scale = ((380.0 * ppp) / w).min((220.0 * ppp) / h).clamp(0.05, 32.0);
    let tile = pdfcraft_render::Tile {
        x: (x0 * scale).floor().max(0.0) as u32,
        y: (y0 * scale).floor().max(0.0) as u32,
        w: (w * scale).ceil().max(1.0) as u32,
        h: (h * scale).ceil().max(1.0) as u32,
    };
    let config = pdfcraft_render::RenderConfig { password: doc.password.as_deref().map(std::sync::Arc::from), ..Default::default() };
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), config);
    let out = r.render(pdfcraft_render::RenderRequest {
        page,
        kind: pdfcraft_render::RequestKind::Pixels,
        tile: Some(tile),
        scale,
        tag: 0,
        preview: false,
    });
    if out.error.is_some() || out.width == 0 {
        return None;
    }
    let img = egui::ColorImage::from_rgba_premultiplied([out.width as usize, out.height as usize], &out.rgba);
    Some(ctx.load_texture("alt-figure", img, egui::TextureOptions::LINEAR))
}

pub(crate) fn alt_body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let ctx = ui.ctx().clone();
    let doc = app.alt_draft.doc.and_then(|id| app.session.get(id));
    let d = &mut app.alt_draft;
    ui.label(egui::RichText::new(tl!("Set Alternate Text")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let n = d.figures.len();
    if n == 0 {
        ui.label(egui::RichText::new(tl!("This document has no tagged figures.")).color(t.text_muted));
        let mut cancel = false;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| cancel = widgets::pill_button(ui, tl!("Close"), true).clicked())
        });
        return (false, cancel);
    }
    d.index = d.index.min(n - 1);
    let i = d.index;
    if d.preview.as_ref().is_none_or(|(k, _)| *k != i)
        && let Some(doc) = doc
    {
        d.preview = figure_picture(&ctx, doc, &d.figures[i]).map(|tex| (i, tex));
    }
    let page = d.figures[i].page.map(|p| crate::i18n::fmt(tl!(" on page {p}"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
    ui.label(
        egui::RichText::new(crate::i18n::fmt(tl!("Figure {i} of {n}{page}"), &[("i", &(i + 1).to_string()), ("n", &n.to_string()), ("page", &page)]))
            .color(t.text_muted),
    );
    ui.add_space(6.0);
    egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        let (area, _) = ui.allocate_exact_size(egui::vec2(380.0, 220.0), egui::Sense::hover());
        match &d.preview {
            Some((_, tex)) => {
                let s = tex.size_vec2();
                let k = (area.width() / s.x).min(area.height() / s.y).min(2.0);
                let fit = egui::Rect::from_center_size(area.center(), s * k);
                ui.painter().image(tex.id(), fit, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => {
                ui.painter().text(area.center(), egui::Align2::CENTER_CENTER, tl!("No preview"), theme::regular(13.0), t.text_faint);
            }
        }
    });
    ui.add_space(8.0);
    ui.add_enabled_ui(!d.decorative[i], |ui| {
        let l = ui.label(tl!("Alternate text"));
        ui.add(crate::widgets::block(&mut d.texts[i]).desired_rows(3).desired_width(380.0).hint_text(tl!("Describe the figure"))).labelled_by(l.id);
    });
    ui.add(widgets::checkbox(&mut d.decorative[i], tl!("Decorative figure")));
    ui.add_space(10.0);
    let (mut save, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        if ui.add_enabled_ui(i > 0, |ui| icons::button(ui, "chevron-left", 28.0, false, tl!("Previous figure"))).inner.clicked() {
            d.index -= 1;
        }
        if ui.add_enabled_ui(i + 1 < n, |ui| icons::button(ui, "chevron-right", 28.0, false, tl!("Next figure"))).inner.clicked() {
            d.index += 1;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Save & Close"), true).clicked() {
                save = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        });
    });
    (save, cancel)
}

impl PdfCraftApp {
    /// Prepare for accessibility ▸ Tag form fields.
    pub(crate) fn autotag_form_fields(&mut self) {
        self.apply_edit(pdfcraft_engine::Edit::AutoTagFormFields);
        self.choose_right_panel(Some(RightPanel::Tags));
    }

    /// Prepare for accessibility ▸ Add alternate text.
    pub(crate) fn start_alt_text(&mut self) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let figures = doc.figures();
        self.alt_draft = AltDraft {
            doc: Some(id),
            texts: figures.iter().map(|f| f.alt.clone().unwrap_or_default()).collect(),
            decorative: vec![false; figures.len()],
            figures,
            index: 0,
            preview: None,
        };
        self.dialog = Some(Dialog::AltText);
    }

    /// Save & Close: one undo step for every change.
    pub(crate) fn save_alt_text(&mut self) {
        let d = std::mem::take(&mut self.alt_draft);
        let mut edits = Vec::new();
        for (k, f) in d.figures.iter().enumerate() {
            if d.decorative[k] {
                edits.push(pdfcraft_engine::Edit::MarkDecorative { figure: f.obj.num });
            } else if d.texts[k].trim() != f.alt.as_deref().unwrap_or("").trim() {
                edits.push(pdfcraft_engine::Edit::SetAltText { figure: f.obj.num, alt: Some(d.texts[k].clone()) });
            }
        }
        if !edits.is_empty() {
            self.apply_edit(pdfcraft_engine::Edit::Batch { label: "Set alternate text".into(), edits });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{A11yJob, INLINE_A11Y_PAGES, Options, check_report};
    use crate::PdfCraftApp;

    /// `n` pages of one word each: small on disk, but every page is work for a check, which
    /// reads every page's text and tags.
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

    fn app_with(n: usize) -> PdfCraftApp {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a11y.pdf", None, pages(n)).unwrap();
        app
    }

    #[test]
    fn a_few_pages_are_checked_before_the_frame_is_out() {
        let mut app = app_with(2);
        app.run_accessibility_check();
        let id = app.views[0].id;
        assert!(app.a11y_job.is_none(), "no worker for a document of few pages");
        assert!(!app.a11y.checking, "done in the frame that asked");
        assert!(app.a11y.report.as_ref().is_some_and(|(d, _)| *d == id), "the report is in");
        assert_eq!(app.a11y.report_generation, Some(app.session.get(id).unwrap().edit_generation()));
        // A check of the same document with the same options keeps the older one to compare.
        assert!(app.a11y.previous.is_none(), "nothing was checked before");
        app.run_accessibility_check();
        assert!(app.a11y.previous.is_some(), "the first check became the previous one");
    }

    #[test]
    fn a_many_page_document_is_checked_off_the_ui_thread() {
        const { assert!(INLINE_A11Y_PAGES < 20) };
        let mut app = app_with(20);
        app.run_accessibility_check();
        assert!(app.a11y.report.is_none(), "still running: not on the UI thread");
        assert!(app.a11y.checking, "the panel shows the check is running");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while app.a11y.report.is_none() {
            app.poll_a11y();
            assert!(std::time::Instant::now() < deadline, "the check never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let id = app.views[0].id;
        assert!(app.a11y_job.is_none(), "the job is done");
        assert!(!app.a11y.checking);
        assert_eq!(app.a11y.report_generation, Some(app.session.get(id).unwrap().edit_generation()));
    }

    /// A result computed from anything but the document as it stands (edited, saved, reloaded
    /// and reopened all give it new bytes or a new generation) is dropped, not shown.
    #[test]
    fn a_result_that_arrives_after_the_document_changed_is_dropped() {
        let mut app = app_with(2);
        let id = app.views[0].id;
        let (bytes, generation) = {
            let doc = app.session.get(id).unwrap();
            (doc.bytes.clone(), doc.edit_generation())
        };
        let options = Options::default();
        let report = check_report(bytes.clone(), "a11y.pdf".into(), None, options.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(report).unwrap();
        // The document was edited while the check ran, so the anchor no longer holds.
        app.a11y_job = Some(A11yJob { rx, id, options, anchor: super::Anchor { bytes, generation: generation + 1 } });
        app.a11y.checking = true;
        app.poll_a11y();
        assert!(app.a11y.report.is_none(), "the stale result is not shown");
        assert!(!app.a11y.checking, "the check is over either way");
        assert!(app.a11y_job.is_none());
    }

    #[test]
    fn a_result_for_the_document_as_it_stands_is_installed() {
        let mut app = app_with(2);
        let id = app.views[0].id;
        let (bytes, generation) = {
            let doc = app.session.get(id).unwrap();
            (doc.bytes.clone(), doc.edit_generation())
        };
        let options = Options::default();
        let report = check_report(bytes.clone(), "a11y.pdf".into(), None, options.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(report).unwrap();
        app.a11y_job = Some(A11yJob { rx, id, options, anchor: super::Anchor { bytes, generation } });
        app.a11y.checking = true;
        app.poll_a11y();
        assert!(app.a11y.report.as_ref().is_some_and(|(d, _)| *d == id), "the report landed");
        assert!(!app.a11y.checking);
        assert_eq!(app.a11y.report_generation, Some(generation));
    }

    #[test]
    fn an_edit_marks_the_report_stale() {
        let mut app = app_with(2);
        app.run_accessibility_check();
        let id = app.views[0].id;
        assert_eq!(app.a11y.report_generation, Some(app.session.get(id).unwrap().edit_generation()));
        assert!(app.apply_edit_in(id, pdfcraft_engine::Edit::InsertBlankPage { at: 1, width: 200.0, height: 100.0 }));
        assert_ne!(
            app.a11y.report_generation,
            Some(app.session.get(id).unwrap().edit_generation()),
            "the report now describes the document as it was"
        );
    }

    /// A second document checked does not take the first document's report with it.
    #[test]
    fn a_report_stays_with_its_document() {
        let mut app = app_with(2);
        app.run_accessibility_check();
        let first = app.views[0].id;
        app.open_bytes("other.pdf", None, pages(2)).unwrap();
        let second = app.views[1].id;
        assert_ne!(first, second);
        // The panel filters by document: the first document's report is not the second's.
        assert!(app.a11y.report.as_ref().is_some_and(|(d, _)| *d == first));
        assert!(!app.a11y.checking);
        assert_eq!(app.a11y.report_generation, Some(app.session.get(first).unwrap().edit_generation()));
    }

    #[test]
    fn the_worker_reports_a_document_it_cannot_reopen_as_over() {
        let mut app = app_with(2);
        let id = app.views[0].id;
        // A worker that answers None (the file could not be reopened for checking) ends the
        // check without a report instead of holding the panel in its progress state.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(None).unwrap();
        let options = Options::default();
        app.a11y_job = Some(A11yJob { rx, id, options, anchor: super::Anchor { bytes: std::sync::Arc::new(Vec::new()), generation: 0 } });
        app.a11y.checking = true;
        app.poll_a11y();
        assert!(app.a11y_job.is_none());
        assert!(!app.a11y.checking, "no report, but not stuck checking either");
    }
}
