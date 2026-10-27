//! Redacted content must leave the file: no other reference may keep it reachable by a full save.

use super::*;

/// Whether `secret` survives a full save of `doc`, in the file's bytes or in a decoded stream.
pub(super) fn full_save_holds(doc: &Document, secret: &[u8]) -> bool {
    let bytes = write_full(doc, &SaveOptions::default()).unwrap();
    let found = |data: &[u8]| data.windows(secret.len()).any(|w| w == secret);
    if found(&bytes) {
        return true;
    }
    let saved = Document::open(Arc::new(bytes)).unwrap();
    saved.scan_objects().into_iter().any(|(_, o)| matches!(&*o, Object::Stream(s) if s.decoded().is_ok_and(|d| found(&d))))
}

#[test]
fn a_redacted_field_listed_in_the_calculation_order_leaves_the_saved_file() {
    let mut doc = one_page(b"", "", vec![]);
    pdfcraft_forms::add_field(&mut doc, 0, [10.0, 50.0, 100.0, 70.0], &pdfcraft_forms::NewField::Text { multiline: false }, Some("ssn")).unwrap();
    pdfcraft_forms::set_value(&mut doc, "ssn", &pdfcraft_forms::FieldValue::Text("SSN-078-05-1120".into())).unwrap();
    let field = pdfcraft_forms::fields(&doc).swap_remove(0).obj;
    let root = doc.root().unwrap();
    let af = doc.get(root).as_dict().unwrap().reference(b"AcroForm").unwrap();
    doc.update_dict(af, |d| d.set(b"CO".to_vec(), Object::Array(vec![Object::Ref(field)]))).unwrap();
    mark(&mut doc, 0, &[[0.0, 40.0, 120.0, 80.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().fields, 1);
    assert!(!full_save_holds(&doc, b"SSN-078-05-1120"), "the field's value is still reachable");
}

/// A surviving form shares the page's resource dictionary, which lists the image the mark
/// removes; the form never draws it.
#[test]
fn a_redacted_image_does_not_survive_in_a_surviving_forms_resources() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Font << /F1 5 0 R >> /XObject << /Im 8 0 R /Fm 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 7 0 R", b"BT /F1 10 Tf 10 200 Td (Public) Tj ET"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(content(&doc, 0).contains("Do"), "the form is still drawn");
    assert!(!full_save_holds(&doc, b"IMAGESECRET"), "the removed image is still reachable");
}

/// The same form drawing the image itself keeps it: the page still shows it there.
#[test]
fn an_image_a_surviving_form_draws_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        FONT.replace("95 0 R", "6 0 R").into_bytes(),
        widths(),
        b"<< /Font << /F1 5 0 R >> /XObject << /Im 8 0 R /Fm 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 7 0 R", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the image the form still draws outside the mark");
}

/// The surviving form draws the image through a nested form without /Resources of its own,
/// which draws through the outer form's: the image is kept.
#[test]
fn an_image_a_surviving_forms_nested_form_draws_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources 5 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q /Fm Do"),
        b"<< /XObject << /Im 6 0 R /Fm 7 0 R /Inner 8 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources 5 0 R", b"/Inner Do"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the image the nested form still draws outside the mark");
}

/// Two pages share one resource dictionary, and the second draws the image only through a form
/// without /Resources: redacting the image on the first page keeps it for the second.
#[test]
fn an_image_another_page_draws_through_a_form_without_resources_is_kept() {
    let mut doc = pdf(vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 5 0 R /Resources 7 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 6 0 R /Resources 7 0 R >>".to_vec(),
        stream("", b"q 100 0 0 100 0 0 cm /Im Do Q"),
        stream("", b"/Inner Do"),
        b"<< /XObject << /Im 8 0 R /Inner 9 0 R >> >>".to_vec(),
        stream("/Type /XObject /Subtype /Image /Width 11 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", b"IMAGESECRET"),
        stream("/Type /XObject /Subtype /Form /BBox [0 0 300 300]", b"q 50 0 0 50 200 200 cm /Im Do Q"),
    ]);
    mark(&mut doc, 0, &[[0.0, 0.0, 120.0, 120.0]], "");
    assert_eq!(apply(&mut doc, None).unwrap().images_removed, 1);
    assert!(full_save_holds(&doc, b"IMAGESECRET"), "the second page still draws the image through its form");
}

/// The reference to a page's first content stream, whether `/Contents` names one stream or,
/// as a rewrite leaves it, an array of them.
fn first_content_ref(doc: &Document) -> ObjRef {
    let page = pdfcraft_model::pages(doc).swap_remove(0);
    let contents = page.dict.get(b"Contents").cloned().expect("the page has content");
    match contents {
        Object::Ref(r) => r,
        Object::Array(a) => a.first().and_then(Object::as_ref).expect("the page's content array holds a stream"),
        _ => panic!("the page's content is not a stream reference"),
    }
}

/// A tagged PDF names its page's content stream from the structure tree: the `/MCR`'s `/Stm`
/// and `/StmOwn` must follow the rewritten stream, or the old one — the removed text still in
/// its bytes — keeps it in the saved file. The element's descriptions repeat the word too.
#[test]
fn a_tagged_mcr_reference_follows_the_rewritten_content_stream() {
    for indirect in [false, true] {
        // The marked content is named in place (a direct `/MCR` kid) or as an object of its own.
        let kid = if indirect { "8 0 R".to_string() } else { "<< /Type /MCR /Pg 3 0 R /Stm 4 0 R /StmOwn 4 0 R /MCID 0 >>".to_string() };
        let objs = vec![
            b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>"
                .to_vec(),
            stream("", b"/P << /MCID 0 >> BDC BT /F1 14 Tf 20 150 Td (Codename Bluefin) Tj ET BT /F1 14 Tf 20 50 Td (Visible words) Tj ET EMC"),
            b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [7 0 R]] >> /ParentTreeNextKey 1 >>".to_vec(),
            b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
            format!("<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K {kid} /ActualText (Codename Bluefin) /Alt (Codename Bluefin) >>").into_bytes(),
            b"<< /Type /MCR /Pg 3 0 R /Stm 4 0 R /StmOwn 4 0 R /MCID 0 >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
        ];
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[10.0, 140.0, 290.0, 170.0]], "");
        let r = apply(&mut doc, None).unwrap();
        assert!(r.glyphs > 0 && r.tags >= 1, "indirect: {indirect}, {r:?}");
        // The reference follows the rewrite, `drawn` where the page now draws the marked content.
        let contents = first_content_ref(&doc);
        assert_ne!(contents, ObjRef::new(4, 0), "the page draws a rewritten stream (indirect: {indirect})");
        let mcr = if indirect {
            doc.get(ObjRef::new(8, 0)).as_dict().cloned().unwrap()
        } else {
            let element = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap();
            doc.resolve(element.get(b"K").unwrap()).as_dict().cloned().unwrap()
        };
        assert_eq!(mcr.get(b"Stm").and_then(Object::as_ref), Some(contents), "indirect: {indirect}, {mcr:?}");
        assert_eq!(mcr.get(b"StmOwn").and_then(Object::as_ref), Some(contents), "indirect: {indirect}, {mcr:?}");
        // Neither the old stream nor the element's descriptions keep the removed words.
        assert!(!full_save_holds(&doc, b"Codename Bluefin"), "the removed text is still reachable (indirect: {indirect})");
        assert!(full_save_holds(&doc, b"Visible words"), "the search sees the saved content (indirect: {indirect})");
    }
}

/// Sanitize removes hidden text the same way, and the tagged tree follows its rewritten stream
/// just as a redaction's does.
#[test]
fn hidden_text_removal_repairs_a_tagged_mcr_reference_too() {
    let objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R /MarkInfo << /Marked true >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /StructParents 0 /Resources << /Font << /F1 9 0 R >> >> >>".to_vec(),
        stream("", b"/P << /MCID 0 >> BDC BT /F1 14 Tf 20 150 Td 3 Tr (Codename Bluefin) Tj 0 Tr (Visible words) Tj ET EMC"),
        b"<< /Type /StructTreeRoot /K 6 0 R /ParentTree << /Nums [0 [7 0 R]] >> /ParentTreeNextKey 1 >>".to_vec(),
        b"<< /Type /StructElem /S /Document /P 5 0 R >>".to_vec(),
        b"<< /Type /StructElem /S /P /P 6 0 R /Pg 3 0 R /K << /Type /MCR /Pg 3 0 R /Stm 4 0 R /MCID 0 >> /Alt (Codename Bluefin) >>".to_vec(),
        b"<< >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec(),
    ];
    let mut doc = pdf(objs);
    let done = crate::sanitize::remove_hidden(&mut doc, &[crate::sanitize::Hidden::HiddenText]).unwrap();
    assert!(done.contains(&(crate::sanitize::Hidden::HiddenText, 16)), "{done:?}");
    let contents = first_content_ref(&doc);
    assert_ne!(contents, ObjRef::new(4, 0), "the page draws a rewritten stream");
    let element = doc.get(ObjRef::new(7, 0)).as_dict().cloned().unwrap();
    let mcr = doc.resolve(element.get(b"K").unwrap()).as_dict().cloned().unwrap();
    assert_eq!(mcr.get(b"Stm").and_then(Object::as_ref), Some(contents), "{mcr:?}");
    assert!(!full_save_holds(&doc, b"Codename Bluefin"), "the hidden text is still reachable");
    assert!(full_save_holds(&doc, b"Visible words"), "the search sees the saved content");
}

/// A second reference to the replaced content stream — a name tree value, a catalog key or a
/// marked-content reference no element's `/K` reaches — keeps the removed text in the saved
/// file whatever the page shows. The redaction is refused rather than reporting success with
/// the text still reachable, and a refused document is left as it was.
#[test]
fn a_second_reference_to_a_replaced_content_stream_refuses_the_redaction() {
    let seconds: [&[u8]; 4] =
        [b"/Names << /Names [(spare) 4 0 R] >>", b"/PieceInfo 4 0 R", b"/PCShare [4 0 R]", b"/PCRef << /Type /MCR /Pg 3 0 R /Stm 4 0 R /MCID 0 >>"];
    for second in seconds {
        let mut catalog = b"<< /Type /Catalog /Pages 2 0 R ".to_vec();
        catalog.extend_from_slice(second);
        catalog.extend_from_slice(b" >>");
        let objs = vec![
            catalog,
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
            stream("", b"BT /F1 10 Tf 10 100 Td (AB1234CD) Tj ET"),
            FONT.replace("95 0 R", "6 0 R").into_bytes(),
            widths(),
        ];
        let mut doc = pdf(objs);
        mark(&mut doc, 0, &[[20.0, 95.0, 40.0, 110.0]], "");
        let before = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(matches!(apply(&mut doc, None), Err(RedactError::ReplacedStreamReachable)), "{second:?}");
        assert_eq!(write_full(&doc, &SaveOptions::default()).unwrap(), before, "the refused document is unchanged ({second:?})");
    }
}
