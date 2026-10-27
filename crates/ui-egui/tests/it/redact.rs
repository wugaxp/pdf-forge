//! Redact a PDF in the real shell (egui_kittest): the Redact tool, Redact pages, the apply
//! confirmation and Clear all.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("form.pdf", None, include_bytes!("../data/form.pdf").to_vec()).unwrap();
        app.set_option("zoom", "150").unwrap();
        app
    });
    crate::support::settle(&mut h);
    h
}

fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).expect("on screen");
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, y * k)
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> usize {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().redaction_marks()
}

#[test]
fn marking_applying_and_clearing() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.mark"));
    h.run_steps(2);
    h.get_by_label("Redact a PDF");
    // Draw a box in an empty part of the page.
    let (a, b) = (at(&h, 40.0, 280.0), at(&h, 200.0, 320.0));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(a + (b - a) * (k as f32 / 4.0));
        h.run_steps(1);
    }
    h.drop_at(b);
    h.run_steps(4);
    assert_eq!(marks(&h), 1);
    // Redact pages ▸ current page.
    h.state_mut().execute("redact.pages");
    h.run_steps(2);
    h.get_by_label("Mark Page Range");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Clear all, then undo it.
    h.get_by_label("Clear all").click();
    h.run_steps(3);
    assert_eq!(marks(&h), 0);
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(marks(&h), 2);
    // Redact all asks first; Apply removes the marks (and the page's fields under them).
    h.get_by_label("Redact all").click();
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::RedactApply));
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!((doc.redaction_marks(), doc.can_undo()), (0, Some("Apply redactions")));
    assert!(doc.form.is_empty(), "the whole page was redacted, fields included");
}

#[test]
fn redaction_codes_become_the_overlay_text() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.get_by_label("Redaction Tool Properties");
    h.get_by_label("Use overlay text").click();
    h.run_steps(2);
    h.state_mut().redact_prefs.overlay = "CONFIDENTIAL".into();
    h.get_by_label("Redaction code:").click();
    h.run_steps(2);
    h.get_by_label("(b)(7)(C)").click();
    h.run_steps(2);
    h.get_by_label("(b)(6)").click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(2);
    let prefs = &h.state().redact_prefs;
    assert_eq!(prefs.overlay_text(), "(b)(6), (b)(7)(C)", "codes in the set's order");
    let pdfcraft_engine::Edit::AddAnnotation(a) = prefs.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert!(matches!(a.shape, pdfcraft_engine::Shape::Redact { ref overlay, .. } if overlay == "(b)(6), (b)(7)(C)"));
    h.state_mut().redact_prefs.use_code = false;
    assert_eq!(h.state().redact_prefs.overlay_text(), "CONFIDENTIAL", "custom text is kept while codes are used");
}

/// The Redaction Tool Properties are the next run's too, like every other tool's defaults
/// (Acrobat's "Make Properties Default" without the box): fill, overlay text or codes, the
/// text's look, and the mark's outline colour and opacity all come back from the settings.
#[test]
fn redaction_properties_survive_a_restart() {
    let mut h = harness();
    {
        let p = &mut h.state_mut().redact_prefs;
        p.fill = None;
        p.use_overlay = true;
        p.use_code = true;
        p.code_set = pdfcraft_engine::REDACTION_CODE_SETS[0];
        p.codes = vec!["(b)(6)", "(b)(7)(C)"];
        p.overlay = "CONFIDENTIAL".into();
        p.look.font = pdfcraft_engine::OverlayFont::Courier;
        p.look.size = 12.0;
        p.look.color = [0.0, 0.0, 1.0];
        p.look.align = 2;
        p.look.repeat = true;
        p.outline = [0.0, 0.5, 0.0];
        p.hover_fill = [1.0, 1.0, 0.0];
        p.opacity = 0.4;
    }
    // The defaults are what OK with "Make properties default" ticked leaves: go through the
    // dialog.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Make properties default");
    h.get_by_label("OK").click();
    h.run_steps(2);
    let json = h.state().persist();
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&json);
    let p = &fresh.redact_prefs;
    assert_eq!(p.fill, None, "no colour is a saved choice, not a missing one");
    assert_eq!(p.overlay_text(), "(b)(6), (b)(7)(C)");
    assert_eq!(p.overlay, "CONFIDENTIAL");
    assert_eq!(p.look.font, pdfcraft_engine::OverlayFont::Courier);
    assert_eq!((p.look.size, p.look.color, p.look.align, p.look.repeat), (12.0, [0.0, 0.0, 1.0], 2, true));
    assert_eq!((p.outline, p.hover_fill, p.opacity), ([0.0, 0.5, 0.0], [1.0, 1.0, 0.0], 0.4));
    // The next mark takes the outline and opacity.
    let pdfcraft_engine::Edit::AddAnnotation(a) = p.mark(0, vec![pdfcraft_engine::rect_quad([10.0, 10.0, 50.0, 30.0])], "T") else {
        panic!("a mark adds an annotation")
    };
    assert_eq!((a.style.color, a.style.opacity, a.style.fill), ([0.0, 0.5, 0.0], 0.4, None));
    // Settings are untrusted: a bad colour, an unknown code set and a code outside the set are
    // dropped, the rest kept.
    let mut odd = PdfCraftApp::new();
    odd.restore(
        r#"{"redact_prefs":{"outline":[2,0,0],"opacity":0.7,"code_set":"nope","codes":["(b)(6)","bogus"],"use_overlay":true,"use_code":true}}"#,
    );
    let p = &odd.redact_prefs;
    assert_eq!((p.outline, p.opacity), ([0.89, 0.13, 0.13], 0.7));
    assert_eq!(p.code_set.id, pdfcraft_engine::REDACTION_CODE_SETS[0].id);
    assert_eq!(p.overlay_text(), "(b)(6)");
}

/// "Make properties default" off: OK keeps the values for this run only, the saved defaults
/// stay what they were; Cancel puts back what the dialog opened with.
#[test]
fn redaction_properties_for_this_run_only_and_cancel() {
    let mut h = harness();
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(6);
    h.get_by_label("Redaction mark appearance");
    h.get_by_label("Outline colour:");
    h.get_by_label("Fill colour:");
    h.get_by_label("Opacity:");
    h.get_by_label("Make properties default").click();
    h.run_steps(3);
    assert!(!h.state().redact_prefs.remember);
    h.state_mut().redact_prefs.outline = [0.0, 0.0, 1.0];
    h.get_by_label("OK").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "used for this run");
    assert_eq!(h.state().redact_defaults.outline, [0.89, 0.13, 0.13], "not made the default");
    let mut fresh = PdfCraftApp::new();
    fresh.restore(&h.state().persist());
    assert_eq!(fresh.redact_prefs.outline, [0.89, 0.13, 0.13], "the next run starts from the saved defaults");
    // Cancel discards the edits made while the dialog was open.
    assert!(h.state_mut().execute("redact.properties"));
    h.run_steps(2);
    h.state_mut().redact_prefs.opacity = 0.2;
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert_eq!(h.state().redact_prefs.opacity, 1.0, "cancelled");
    assert_eq!(h.state().redact_prefs.outline, [0.0, 0.0, 1.0], "the run's values stay");
}

#[test]
fn redacting_an_imported_word_list() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Call Ada today\nAda is private\nPublic line").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.run_steps(2);
    h.get_by_label("Multiple words or phrases");
    let list = b"  Ada \n\nprivate\nAda\nabsent\n".to_vec();
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("names.txt".into(), list)]);
    let d = &h.state().redact_search;
    assert_eq!((d.mode, d.words.as_str()), (pdfcraft_ui_egui::RedactSearchMode::Words, "Ada\nprivate\nabsent"));
    h.run_steps(2);
    h.get_by_label("3 word(s) or phrase(s)");
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.outcome), (3, Some(pdfcraft_ui_egui::RedactSearchOutcome::Marked(3))));
    let too_big = vec![b'a'; 2 << 20];
    h.state_mut().use_files(pdfcraft_ui_egui::FilePurpose::RedactWords, vec![("huge.txt".into(), too_big)]);
    assert_eq!(h.state().redact_search.words, "Ada\nprivate\nabsent", "an oversized list changes nothing");
}

#[test]
fn redaction_search_can_match_case_and_whole_words() {
    let memo = pdfcraft_engine::Session::new().create_from_text("memo", "Ada ada Adaline\nAda beta betamax").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("memo.pdf", None, memo.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.state_mut().redact_search.text = "Ada".into();
    h.run_steps(2);
    h.get_by_label("Whole words only").click();
    h.get_by_label("Case-sensitive").click();
    h.run_steps(2);
    assert!(h.state().redact_search.whole_words && h.state().redact_search.case_sensitive);
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.outcome), (2, Some(pdfcraft_ui_egui::RedactSearchOutcome::Marked(2))));
}

/// `n` pages of "Ada is private": tiny on disk, but every page is work for a search, which reads
/// every page's text.
fn many_pages(n: usize) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for i in 0..n {
        let content = "BT /F1 18 Tf 72 700 Td (Ada is private) Tj ET";
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

fn many_page_harness(n: usize) -> Harness<'static, PdfCraftApp> {
    Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("big.pdf", None, many_pages(n)).unwrap();
        app
    })
}

/// A search runs off the UI thread when a document has many pages, however small the file is:
/// the page count says how much work reading every page's text is, not the bytes on disk. A
/// document of few pages is searched at once, on the UI thread.
#[test]
fn a_search_of_many_pages_runs_off_the_ui_thread() {
    let mut h = many_page_harness(20);
    h.run_steps(4);
    h.state_mut().redact_search.text = "Ada".into();
    h.state_mut().redact_search();
    assert!(h.state().redact_search.outcome.is_none(), "still scanning: not on the UI thread");
    // The marks land when the scan is through. Wait by a deadline, not a frame count, which a
    // loaded machine can use up before the worker is done.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().redact_search.outcome.is_none() {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(std::time::Instant::now() < deadline, "the search never finished");
    }
    assert_eq!(h.state().redact_search.outcome, Some(pdfcraft_ui_egui::RedactSearchOutcome::Marked(20)));
    assert_eq!(marks(&h), 20);
    // Two pages: small work is done before the first repaint.
    let mut small = many_page_harness(2);
    small.run_steps(4);
    small.state_mut().redact_search.text = "Ada".into();
    small.state_mut().redact_search();
    assert_eq!(small.state().redact_search.outcome, Some(pdfcraft_ui_egui::RedactSearchOutcome::Marked(2)), "small work runs inline");
}

/// A search whose document changed while it ran has its marks thrown away — and says that is
/// what happened: "0 matches" would claim the document was searched and had none.
#[test]
fn a_search_whose_document_changed_reports_that_instead_of_no_matches() {
    let mut h = many_page_harness(20);
    h.run_steps(4);
    h.state_mut().redact_search.text = "Ada".into();
    h.state_mut().redact_search();
    assert!(h.state().redact_search.outcome.is_none(), "the scan is off the UI thread");
    // The document changes before the scan is through (no frame runs in between, so the result
    // cannot land first).
    h.state_mut().apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().redact_search.outcome.is_none() {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(std::time::Instant::now() < deadline, "the search never finished");
    }
    assert_eq!(h.state().redact_search.outcome, Some(pdfcraft_ui_egui::RedactSearchOutcome::DocumentChanged));
    assert_eq!(marks(&h), 0, "marks read from an older document are not applied");
}

/// One scan at a time: a second search while one runs is refused rather than dropping the first
/// one's result while its worker keeps reading the whole document.
#[test]
fn a_second_search_waits_for_the_first() {
    let mut h = many_page_harness(20);
    h.run_steps(4);
    h.state_mut().redact_search.text = "Ada".into();
    h.state_mut().redact_search();
    h.state_mut().redact_search();
    assert_eq!(h.state().toast.clone().map(|(m, _)| m), Some("A search is already running".to_string()));
    assert!(h.state().redact_search.outcome.is_none());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while h.state().redact_search.outcome.is_none() {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(std::time::Instant::now() < deadline, "the search never finished");
    }
    assert_eq!(h.state().redact_search.outcome, Some(pdfcraft_ui_egui::RedactSearchOutcome::Marked(20)), "the first search's marks land");
}

#[test]
fn removing_hidden_information_and_sanitizing() {
    let mut h = harness();
    h.state_mut().execute("protect.remove_hidden");
    h.run_steps(2);
    h.get_by_label("Remove hidden information");
    {
        let d = &h.state().hidden_draft;
        let fields = d.found.iter().find(|f| f.0 == pdfcraft_engine::Hidden::FormFields).unwrap();
        assert_eq!((fields.1, fields.2), (5, true), "five fields, checked");
    }
    h.get_by_label("Remove").click();
    h.run_steps(3);
    {
        let s = h.state();
        let doc = s.session.get(s.views[0].id).unwrap();
        assert!(doc.form.is_empty());
        assert_eq!(doc.can_undo(), Some("Remove hidden information"));
    }
    h.state_mut().execute("redact.sanitize");
    h.run_steps(2);
    h.get_by_label("Sanitize").click();
    h.run_steps(3);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Sanitize document"));
}
