//! Redaction and the structure tree: tags must not keep what was removed from the page.
//!
//! Marked content (MCIDs) that applying redactions changed is found by comparing each MCID's
//! operators before and after. Elements owning such content lose their alternate text, actual
//! text and expansion (which may spell out the removed words);
//! references to marked content that is now empty are dropped from the element.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::RedactError;

/// The marked content of one content stream. Every operator inside some MCID is serialized once
/// into `ops`, and each MCID holds the ranges of `ops` its sequences span (marked-content
/// operators are not written, so a sequence is one range however deeply it nests), with whether
/// anything is drawn inside it. Copying the operators into every enclosing MCID instead would
/// hold `depth` copies of nested content.
#[derive(Default)]
struct Marked {
    ops: Vec<u8>,
    ids: HashMap<i64, (Vec<Range<usize>>, bool)>,
}

impl Marked {
    /// The bytes of `ranges`, which are ranges of `ops`.
    fn bytes<'a>(&'a self, ranges: &'a [Range<usize>]) -> impl Iterator<Item = &'a u8> + 'a {
        ranges.iter().flat_map(|r| self.ops.get(r.clone()).unwrap_or_default())
    }
}

/// Most bytes [`touched`] compares for one stream. Nested sequences share their operators, so the
/// comparisons can add up to the content's size times its nesting depth; past this, the MCIDs
/// left are taken as changed (their descriptions go) rather than compared.
const MAX_COMPARED: usize = 1 << 28;

fn by_mcid(doc: &Document, resources: &Dict, data: &[u8]) -> Marked {
    let mut out = Marked::default();
    // Open sequences: the MCID with where its content starts in `ops` and the drawing operators
    // written before it, or `None` for a sequence without one.
    let mut stack: Vec<Option<(i64, usize, usize)>> = Vec::new();
    let (mut open_ids, mut drawn) = (0usize, 0usize);
    let close = |out: &mut Marked, open: Option<(i64, usize, usize)>, drawn: usize| {
        if let Some((id, start, drawn_before)) = open {
            let end = out.ops.len();
            let e = out.ids.entry(id).or_default();
            e.0.push(start..end);
            e.1 |= drawn > drawn_before;
        }
    };
    for op in pdfcraft_content::parse(data).ops {
        match op.op.as_slice() {
            b"BDC" => {
                let id = match op.operands.get(1) {
                    Some(Object::Name(n)) => resources.get(b"Properties").and_then(|p| {
                        let props = doc.resolve(p);
                        let prop = doc.resolve(props.as_dict()?.get(n)?);
                        prop.as_dict()?.int(b"MCID")
                    }),
                    Some(p) => doc.resolve(p).as_dict().and_then(|d| d.int(b"MCID")),
                    _ => None,
                };
                if let Some(id) = id {
                    out.ids.entry(id).or_default();
                    open_ids += 1;
                }
                stack.push(id.map(|id| (id, out.ops.len(), drawn)));
                continue;
            }
            b"BMC" => {
                stack.push(None);
                continue;
            }
            b"EMC" => {
                if let Some(open) = stack.pop() {
                    open_ids = open_ids.saturating_sub(usize::from(open.is_some()));
                    close(&mut out, open, drawn);
                }
                continue;
            }
            _ => {}
        }
        if open_ids == 0 {
            continue;
        }
        let draws = match op.op.as_slice() {
            b"Tj" | b"'" | b"\"" => op.operands.last().and_then(Object::as_string).is_some_and(|s| !s.bytes.is_empty()),
            b"TJ" => {
                op.operands.first().and_then(Object::as_array).is_some_and(|a| a.iter().any(|x| x.as_string().is_some_and(|s| !s.bytes.is_empty())))
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"sh" | b"Do" | b"BI" => true,
            _ => false,
        };
        pdfcraft_content::write_op(&op, &mut out.ops);
        drawn += usize::from(draws);
    }
    // Sequences the stream leaves open run to its end.
    while let Some(open) = stack.pop() {
        close(&mut out, open, drawn);
    }
    out
}

/// The MCIDs whose content changed between `before` and `after`, and those left empty.
pub(crate) fn touched(doc: &Document, resources: &Dict, before: &[u8], after: &[u8]) -> (HashSet<i64>, HashSet<i64>) {
    let (b, a) = (by_mcid(doc, resources, before), by_mcid(doc, resources, after));
    let mut changed = HashSet::new();
    let mut empty = HashSet::new();
    let mut budget = MAX_COMPARED;
    let len = |ranges: &[Range<usize>]| ranges.iter().map(Range::len).fold(0usize, usize::saturating_add);
    for (id, (old, _)) in &b.ids {
        match a.ids.get(id) {
            Some((new, draws)) => {
                let n = len(old);
                let same = n == len(new) && n <= budget && {
                    budget -= n;
                    b.bytes(old).eq(a.bytes(new))
                };
                if !same {
                    changed.insert(*id);
                }
                if !draws {
                    empty.insert(*id);
                }
            }
            None => {
                changed.insert(*id);
                empty.insert(*id);
            }
        }
    }
    (changed, empty)
}

/// The content stream a cleanup replaces: a page's own (rewritten stream by rewritten stream),
/// or one form XObject's. Marked content is named bare (a number, or an `/MCR` with no `/Stm`)
/// in a page's content, and through `/Stm` in either.
pub(crate) enum Streams<'a> {
    /// The page content streams this pass replaced: each old object and its rewrite. Every
    /// `/MCR` naming one is repointed at the rewrite, so the old stream is not kept reachable
    /// (its bytes still hold what the redaction removed).
    Page(&'a [(ObjRef, ObjRef)]),
    /// The form XObject stream this pass replaced: the old object and its rewrite.
    Form(ObjRef, ObjRef),
}

impl Streams<'_> {
    /// The rewrite of the stream `r` names, if this pass replaced it.
    fn replacement(&self, r: ObjRef) -> Option<(ObjRef, ObjRef)> {
        match self {
            Streams::Page(pairs) => pairs.iter().find(|(old, _)| *old == r).copied(),
            Streams::Form(old, new) => (r == *old).then_some((*old, *new)),
        }
    }

    /// Marked content can be named bare (a number, or an `/MCR` with no `/Stm`): the page's own
    /// content is written that way, a form's never is.
    fn page_content(&self) -> bool {
        matches!(self, Streams::Page(_))
    }

    /// Whether any `/MCR` may need repointing.
    fn has_replaced(&self) -> bool {
        match self {
            Streams::Page(pairs) => !pairs.is_empty(),
            Streams::Form(..) => true,
        }
    }
}

/// The marked content a kid names — its `/MCID`, when the kid is this cleanup's to change:
/// `streams` holds its content stream, or the kid names it bare and it is the page's own.
fn mcid_of(doc: &Document, o: &Object, page: Option<ObjRef>, target: ObjRef, streams: &Streams<'_>) -> Option<i64> {
    match &*doc.resolve(o) {
        Object::Int(n) if page == Some(target) && streams.page_content() => Some(*n),
        Object::Dict(m) if m.name(b"Type") == Some(b"MCR") => {
            let pg = m.get(b"Pg").and_then(Object::as_ref).or(page);
            let mine = match m.get(b"Stm").and_then(Object::as_ref) {
                Some(stm) => streams.replacement(stm).is_some(),
                // No `/Stm`: the marked content is in the page's own content stream.
                None => streams.page_content(),
            };
            (pg == Some(target) && mine).then(|| m.int(b"MCID")).flatten()
        }
        _ => None,
    }
}

/// Whether a kid names a content stream this cleanup replaces through `/Stm` or `/StmOwn`, so
/// its references must follow the rewrite (or, for an element written in place, the operation
/// must be refused: only rewriting what holds the element could change it).
fn names_replaced(doc: &Document, o: &Object, streams: &Streams<'_>) -> bool {
    let resolved = doc.resolve(o);
    let Some(m) = resolved.as_dict() else { return false };
    [&b"Stm"[..], b"StmOwn"].iter().any(|key| m.get(key).and_then(Object::as_ref).is_some_and(|r| streams.replacement(r).is_some()))
}

/// Repoint `/Stm` and `/StmOwn` at the rewritten stream they name.
fn remap_stm(m: &mut Dict, streams: &Streams<'_>) {
    for key in [&b"Stm"[..], b"StmOwn"] {
        if let Some(r) = m.get(key).and_then(Object::as_ref)
            && let Some((_, new)) = streams.replacement(r)
        {
            m.set(key.to_vec(), Object::Ref(new));
        }
    }
}

/// Clean the elements owning `changed` marked content on `page`, and repoint the marked-content
/// references to the replaced `streams`; returns how many changed.
pub(crate) fn clean(
    doc: &mut Document,
    page: ObjRef,
    streams: Streams<'_>,
    changed: &HashSet<i64>,
    empty: &HashSet<i64>,
) -> Result<usize, RedactError> {
    clean_within(doc, page, streams, changed, empty, MAX_STRUCT_REFS)
}

/// [`clean`], visiting at most `limit` structure-tree references. Fail-closed (#3302): when the
/// cleanup can't be shown complete, it is [`RedactError::Tags`] and the redaction isn't made. That
/// is a tree too large to walk in full (it could hide an element that repeats the redacted text),
/// an element whose inherited page can't be found (its ancestry too deep, or looping), and an
/// element written in place that would need changing (only the object holding it could be).
pub(crate) fn clean_within(
    doc: &mut Document,
    page: ObjRef,
    streams: Streams<'_>,
    changed: &HashSet<i64>,
    empty: &HashSet<i64>,
    limit: usize,
) -> Result<usize, RedactError> {
    if changed.is_empty() && !streams.has_replaced() {
        return Ok(0);
    }
    let Some(root) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"StructTreeRoot").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()))
    else {
        return Ok(0);
    };
    // Indirect elements with the page they inherit. Elements are reached from the root's /K, and
    // also from the parent tree and the ID tree: one listed only there still owns marked content
    // (#3302). The stack pops /K first, so an element reached through /K gets the page it inherits
    // from its ancestors before a tree reaches it without one.
    let mut todo: Vec<(Object, Option<ObjRef>, Above)> =
        [&b"IDTree"[..], b"ParentTree", b"K"].iter().filter_map(|key| root.get(key).cloned()).map(|k| (k, None, Above::default())).collect();
    let mut seen = HashSet::new();
    let mut pages = HashMap::new();
    let mut parents = Parents::new();
    let mut hits: Vec<(ObjRef, Dict, Option<ObjRef>, bool)> = Vec::new();
    // Whether marked content `x` (a kid on page `pg`) is one this cleanup must change.
    let touches = |doc: &Document, x: &Object, pg: Option<ObjRef>| {
        // A kid naming a replaced stream must be repointed; written in place, only rewriting
        // what holds the element could do that, so the operation is refused.
        names_replaced(doc, x, &streams) || mcid_of(doc, x, pg, page, &streams).is_some_and(|m| changed.contains(&m) || empty.contains(&m))
    };
    while let Some((k, pg, above)) = todo.pop() {
        match k {
            Object::Array(a) => todo.extend(a.into_iter().map(|x| (x, pg, above))),
            // A number or name tree node written in place.
            Object::Dict(d) if !d.contains(b"S") => todo.extend(tree_node_entries(&d).into_iter().map(|x| (x, pg, above))),
            // An element written in place: its kids are followed, but if it owns marked content
            // this cleanup changes, only rewriting what holds it could change it.
            Object::Dict(d) => {
                let pg = match d.get(b"Pg").and_then(Object::as_ref).or(pg) {
                    Some(p) => Some(p),
                    None => inherited_page(doc, &d, &mut pages)?,
                };
                let kids = kids_of(doc, &d);
                if kids.iter().any(|x| touches(doc, x, pg)) {
                    return Err(RedactError::Tags);
                }
                let above = above.through(&d);
                todo.extend(kids.into_iter().map(|x| (x, pg, above)));
            }
            Object::Ref(r) => {
                parents.record(r, above);
                if !seen.insert(r) {
                    continue;
                }
                if seen.len() > limit {
                    return Err(RedactError::Tags);
                }
                let object = doc.get(r);
                if let Object::Array(a) = &*object {
                    todo.extend(a.iter().cloned().map(|x| (x, pg, above)));
                    continue;
                }
                let Some(d) = object.as_dict().cloned() else { continue };
                if !d.contains(b"S") {
                    // A parent tree or ID tree node: its values may be elements.
                    todo.extend(tree_node_entries(&d).into_iter().map(|x| (x, pg, above)));
                    continue;
                }
                let pg = match d.get(b"Pg").and_then(Object::as_ref).or(pg) {
                    Some(p) => Some(p),
                    None => inherited_page(doc, &d, &mut pages)?,
                };
                let kids = kids_of(doc, &d);
                let sensitive = kids.iter().any(|x| mcid_of(doc, x, pg, page, &streams).is_some_and(|m| changed.contains(&m)));
                let remap = kids.iter().any(|x| names_replaced(doc, x, &streams));
                if sensitive || remap {
                    hits.push((r, d.clone(), pg, sensitive));
                }
                todo.extend(kids.into_iter().map(|x| (x, pg, Above::element(r))));
            }
            _ => {}
        }
    }
    let sensitive: Vec<ObjRef> = hits.iter().filter(|h| h.3).map(|h| h.0).collect();
    let mut n = 0;
    for (r, d, pg, sensitive) in hits {
        let kids = d.get(b"K").map(|k| doc.resolve(k));
        let array = kids.as_ref().is_some_and(|k| matches!(&**k, Object::Array(_)));
        let list = match kids.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => d.get(b"K").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let mut keep = Vec::new();
        for mut kid in list {
            let mcid = mcid_of(doc, &kid, pg, page, &streams);
            if mcid.is_some_and(|m| empty.contains(&m)) {
                continue;
            }
            if names_replaced(doc, &kid, &streams) {
                if let Some(reference) = kid.as_ref() {
                    doc.update_dict(reference, |m| remap_stm(m, &streams))?;
                } else if let Object::Dict(m) = &mut kid {
                    remap_stm(m, &streams);
                }
            }
            keep.push(kid);
        }
        doc.update_dict(r, |e| {
            if sensitive {
                for key in [&b"Alt"[..], b"ActualText", b"E"] {
                    e.remove(key);
                }
            }
            if array {
                e.set(b"K".to_vec(), Object::Array(keep));
            } else if let Some(k) = keep.into_iter().next() {
                e.set(b"K".to_vec(), k);
            } else {
                e.remove(b"K");
            }
        })?;
        n += usize::from(sensitive && doc.get(r).as_dict() != Some(&d));
    }
    // An element above may describe the changed content too (`/ActualText` on a span's parent
    // covers the span, §14.9.4).
    n += clear_ancestors(doc, &sensitive, &parents, limit)?;
    Ok(n)
}

/// Where the walk reached an object from: the nearest element above it that is an object of its
/// own, and whether an element written in place between them carries a description (which only
/// rewriting the object holding it could remove).
#[derive(Clone, Copy, Default)]
struct Above {
    element: Option<ObjRef>,
    described_in_place: bool,
}

impl Above {
    fn element(r: ObjRef) -> Self {
        Self { element: Some(r), described_in_place: false }
    }

    /// Below an element written in place.
    fn through(self, d: &Dict) -> Self {
        Self { described_in_place: self.described_in_place || DESCRIPTIONS.iter().any(|k| d.contains(k)), ..self }
    }
}

/// The keys of an element that describe its content.
const DESCRIPTIONS: [&[u8]; 3] = [b"Alt", b"ActualText", b"E"];

/// Each element's parents as the walk found them: through `/K`, which `/P` may not agree with.
struct Parents(HashMap<ObjRef, Vec<Above>>);

impl Parents {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn record(&mut self, r: ObjRef, above: Above) {
        if above.element.is_some() || above.described_in_place {
            self.0.entry(r).or_default().push(above);
        }
    }
}

/// Remove the descriptions of every element above `elements`, through both `/P` and the walk's
/// `parents`; returns how many elements changed. [`RedactError::Tags`] when an element written in
/// place above them carries one, or when there are more than `limit` ancestors.
fn clear_ancestors(doc: &mut Document, elements: &[ObjRef], parents: &Parents, limit: usize) -> Result<usize, RedactError> {
    let mut todo: Vec<ObjRef> = elements.to_vec();
    let mut seen: HashSet<ObjRef> = elements.iter().copied().collect();
    let mut cleared = 0;
    while let Some(r) = todo.pop() {
        let mut up: Vec<ObjRef> = doc.get(r).as_dict().and_then(|d| d.get(b"P")).and_then(Object::as_ref).into_iter().collect();
        for above in parents.0.get(&r).into_iter().flatten() {
            if above.described_in_place {
                return Err(RedactError::Tags);
            }
            up.extend(above.element);
        }
        for a in up {
            if !seen.insert(a) {
                continue;
            }
            if seen.len() > limit {
                return Err(RedactError::Tags);
            }
            // The structure tree root (no /S) ends the climb.
            let Some(d) = doc.get(a).as_dict().filter(|d| d.contains(b"S")).cloned() else { continue };
            if DESCRIPTIONS.iter().any(|k| d.contains(k)) {
                doc.update_dict(a, |e| {
                    for key in DESCRIPTIONS {
                        e.remove(key);
                    }
                })?;
                cleared += 1;
            }
            todo.push(a);
        }
    }
    Ok(cleared)
}

/// The page an element inherits (§14.7.4.4): the nearest `/Pg` among its ancestors, following
/// `/P`. `Ok(None)` when none has one; [`RedactError::Tags`] when the lookup gives up (more than
/// [`MAX_PARENTS`] ancestors, or a loop), which isn't the same as having no page. `pages` keeps
/// each ancestor's answer, so siblings don't climb the same chain again.
fn inherited_page(doc: &Document, d: &Dict, pages: &mut HashMap<ObjRef, Option<ObjRef>>) -> Result<Option<ObjRef>, RedactError> {
    let mut chain: Vec<ObjRef> = Vec::new();
    let mut parent = d.get(b"P").and_then(Object::as_ref);
    let found = loop {
        let Some(r) = parent else { break None };
        if let Some(known) = pages.get(&r) {
            break *known;
        }
        if chain.len() >= MAX_PARENTS || chain.contains(&r) {
            return Err(RedactError::Tags);
        }
        chain.push(r);
        let object = doc.get(r);
        let Some(p) = object.as_dict() else { break None };
        if let Some(pg) = p.get(b"Pg").and_then(Object::as_ref) {
            break Some(pg);
        }
        parent = p.get(b"P").and_then(Object::as_ref);
    };
    for r in chain {
        pages.insert(r, found);
    }
    Ok(found)
}

/// An element's kids: its `/K` array, or the one kid it has.
fn kids_of(doc: &Document, d: &Dict) -> Vec<Object> {
    match d.get(b"K").map(|k| doc.resolve(k)) {
        Some(o) => match &*o {
            Object::Array(a) => a.clone(),
            _ => d.get(b"K").cloned().into_iter().collect(),
        },
        None => Vec::new(),
    }
}

/// The most ancestors [`inherited_page`] looks through; deeper trees, and cycles, go unanswered.
const MAX_PARENTS: usize = 256;

/// Whether `o` is an object reference (`/Type /OBJR`, direct or indirect) to one of `gone`.
fn refers_to(doc: &Document, o: &Object, gone: &HashSet<ObjRef>) -> bool {
    doc.resolve(o)
        .as_dict()
        .is_some_and(|d| d.name(b"Type") == Some(b"OBJR") && d.get(b"Obj").and_then(Object::as_ref).is_some_and(|r| gone.contains(&r)))
}

/// What a number or name tree node leads on to: its `/Nums` or `/Names` array (keys and values
/// alike; the walk ignores numbers and strings) and its `/Kids`.
fn tree_node_entries(d: &Dict) -> Vec<Object> {
    [&b"Nums"[..], b"Names", b"Kids"].iter().filter_map(|key| d.get(key).cloned()).collect()
}

/// Drop the structure tree's object references (OBJR) to `gone`, annotations removed from their
/// pages (#2447). A tagged PDF's tree points at each link and comment it tags; left there, the
/// reference keeps the annotation, its URL or text included, in a full rewrite after the page
/// no longer lists it. Returns how many references were dropped.
///
/// Only elements that are objects of their own are edited: changing a structure element written in
/// place (a direct dictionary in a `/K` array or a tree) means rewriting the object that holds it,
/// so when one of those refers to a removed annotation it is [`RedactError::Tags`] (fail-closed).
/// Producers rarely write elements that way.
pub(crate) fn drop_object_refs(doc: &mut Document, gone: &HashSet<ObjRef>) -> Result<usize, RedactError> {
    drop_object_refs_within(doc, gone, MAX_STRUCT_REFS)
}

/// The most structure-tree references [`drop_object_refs`] visits.
const MAX_STRUCT_REFS: usize = 2_000_000;

/// [`drop_object_refs`], visiting at most `limit` references. Fail-closed: a tree too large to
/// walk in full could keep a reference the walk never reached, and an element written in place
/// can only be changed by rewriting what holds it, so either is [`RedactError::Tags`] rather than
/// a cleanup that leaves the annotation reachable (removing only the tree would not: an outline
/// item's `/SE` or a link's `/SD` still reaches its elements).
pub(crate) fn drop_object_refs_within(doc: &mut Document, gone: &HashSet<ObjRef>, limit: usize) -> Result<usize, RedactError> {
    if gone.is_empty() {
        return Ok(0);
    }
    let Some(root) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|c| c.get(b"StructTreeRoot").map(|s| doc.resolve(s)).and_then(|s| s.as_dict().cloned()))
    else {
        return Ok(0);
    };
    // Elements are reached from the root's /K, and also from the parent tree and the ID tree:
    // one listed only there (left behind by an editor, say) still points at its annotation.
    let mut todo: Vec<(Object, Above)> =
        [&b"K"[..], b"ParentTree", b"IDTree"].iter().filter_map(|key| root.get(key).cloned()).map(|k| (k, Above::default())).collect();
    let mut seen = HashSet::new();
    let mut parents = Parents::new();
    let mut hits: Vec<ObjRef> = Vec::new();
    while let Some((k, above)) = todo.pop() {
        match k {
            Object::Array(a) => todo.extend(a.into_iter().map(|x| (x, above))),
            // A structure element written in place: its kids are followed; one that tags a
            // removed annotation can't be changed here (see above).
            Object::Dict(d) if d.contains(b"S") => {
                let kids = kids_of(doc, &d);
                if kids.iter().any(|x| refers_to(doc, x, gone)) {
                    return Err(RedactError::Tags);
                }
                let above = above.through(&d);
                todo.extend(kids.into_iter().map(|x| (x, above)));
            }
            // A number or name tree node written in place (§7.9.6, §7.9.7): its values and kids.
            Object::Dict(d) => todo.extend(tree_node_entries(&d).into_iter().map(|x| (x, above))),
            Object::Ref(r) => {
                parents.record(r, above);
                if !seen.insert(r) {
                    continue;
                }
                if seen.len() > limit {
                    return Err(RedactError::Tags);
                }
                let object = doc.get(r);
                if let Object::Array(a) = &*object {
                    todo.extend(a.iter().cloned().map(|x| (x, above)));
                    continue;
                }
                let Some(d) = object.as_dict() else { continue };
                if !d.contains(b"S") {
                    // A parent tree or ID tree node: its values may be elements.
                    todo.extend(tree_node_entries(d).into_iter().map(|x| (x, above)));
                    continue;
                }
                let kids = kids_of(doc, d);
                if kids.iter().any(|x| refers_to(doc, x, gone)) {
                    hits.push(r);
                }
                todo.extend(kids.into_iter().map(|x| (x, Above::element(r))));
            }
            _ => {}
        }
    }
    let mut n = 0;
    for &r in &hits {
        let Some(d) = doc.get(r).as_dict().cloned() else { continue };
        let kids = d.get(b"K").map(|k| doc.resolve(k));
        let array = kids.as_ref().is_some_and(|k| matches!(&**k, Object::Array(_)));
        let list: Vec<Object> = match kids.as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => d.get(b"K").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        let before = list.len();
        let keep: Vec<Object> = list.into_iter().filter(|x| !refers_to(doc, x, gone)).collect();
        n += before - keep.len();
        doc.update_dict(r, |e| {
            if array {
                e.set(b"K".to_vec(), Object::Array(keep.clone()));
            } else if let Some(k) = keep.first().cloned() {
                e.set(b"K".to_vec(), k);
            } else {
                e.remove(b"K");
            }
            // An element that lost an annotation keeps no text describing it, even with other
            // kids left: a link's alternate text often repeats the URL, an annotation's its
            // comment. Elements above it may describe it too.
            for key in [&b"Alt"[..], b"ActualText", b"T", b"E"] {
                e.remove(key);
            }
        })?;
    }
    clear_ancestors(doc, &hits, &parents, limit)?;
    Ok(n)
}

/// Bytes held for comparing marked content, for tests that bound it.
#[cfg(test)]
pub(crate) fn retained_for_comparison(doc: &Document, resources: &Dict, data: &[u8]) -> usize {
    let marked = by_mcid(doc, resources, data);
    marked.ops.len() + marked.ids.values().map(|(ranges, _)| ranges.len() * std::mem::size_of::<Range<usize>>()).sum::<usize>()
}
