//! Bridging the COS object model and `pdfcraft-crypt`: reading `/Encrypt` dictionaries and
//! decrypting / encrypting the strings and streams of an object (§7.6.2).
//!
//! Not encrypted, per the spec: the `/Encrypt` dictionary itself, cross-reference streams, the
//! trailer `/ID`, objects inside object streams (the object stream is encrypted as a whole),
//! the `/Contents` of signature dictionaries, and metadata streams when `/EncryptMetadata` is
//! false. Streams naming a crypt filter (`/Crypt` in `/Filter`) use that filter; embedded files
//! use `/EFF`.

use pdfcraft_crypt::{EncryptDict, Method, SecurityHandler, StreamKind};

use crate::{Dict, Document, Object, PdfString, Stream};

fn bytes_of(doc: &Document, d: &Dict, key: &[u8]) -> Vec<u8> {
    d.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_string().map(|s| s.bytes.clone())).unwrap_or_default()
}

/// Read the handler parameters from an `/Encrypt` dictionary.
pub(crate) fn encrypt_dict(doc: &Document, d: &Dict) -> EncryptDict {
    let int = |k: &[u8], default: i64| d.get(k).map(|v| doc.resolve(v)).and_then(|v| v.as_int()).unwrap_or(default);
    let name = |k: &[u8]| d.get(k).and_then(|v| v.as_name()).map(|n| n.to_vec()).unwrap_or_else(|| b"Identity".to_vec());
    let mut crypt_filters = Vec::new();
    if let Some(cf) = d.get(b"CF").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()) {
        for (k, v) in cf.iter() {
            let Some(f) = doc.resolve(v).as_dict().cloned() else { continue };
            let method = match f.name(b"CFM") {
                Some(b"V2") => Method::Rc4,
                Some(b"AESV2") => Method::Aes128,
                Some(b"AESV3") => Method::Aes256,
                _ => Method::Identity,
            };
            crypt_filters.push((k.clone(), method));
        }
    }
    let v = int(b"V", 0);
    // V4: the key length comes from the standard crypt filter's /Length (bytes, or bits in
    // some files); /Length on the dictionary is often a stale 40. A length that isn't
    // positive means nothing and reads as if absent.
    let cf_len = d
        .get(b"CF")
        .map(|c| doc.resolve(c))
        .and_then(|c| c.as_dict().and_then(|c| c.get(b"StdCF").map(|f| doc.resolve(f))))
        .and_then(|f| f.as_dict().and_then(|f| f.int(b"Length")))
        .filter(|n| *n > 0);
    let length_bits = match (v, cf_len) {
        (5.., _) => 256,
        (4, Some(n)) if n <= 32 => n * 8,
        (4, Some(n)) => n,
        (4, None) => 128,
        _ => int(b"Length", 40),
    };
    EncryptDict {
        filter: d.name(b"Filter").map(|n| n.to_vec()).unwrap_or_default(),
        v,
        r: int(b"R", 0),
        length_bits,
        o: bytes_of(doc, d, b"O"),
        u: bytes_of(doc, d, b"U"),
        oe: bytes_of(doc, d, b"OE"),
        ue: bytes_of(doc, d, b"UE"),
        perms: bytes_of(doc, d, b"Perms"),
        p: int(b"P", -1) as i32,
        encrypt_metadata: !matches!(d.get(b"EncryptMetadata"), Some(Object::Bool(false))),
        crypt_filters,
        stm_f: name(b"StmF"),
        str_f: name(b"StrF"),
        ef_f: d.get(b"EFF").and_then(|v| v.as_name()).map(|n| n.to_vec()).unwrap_or_default(),
    }
}

/// The crypt filter a stream names in its `/Filter` chain (`/Crypt` + `/DecodeParms /Name`).
fn named_crypt_filter(dict: &Dict) -> Option<Vec<u8>> {
    // `/Crypt`'s place in the filter chain, taken on the raw entries: `/DecodeParms` lines up
    // with `/Filter` entry by entry, and a chain may hold entries that are not names (an
    // indirect one this reader cannot follow, say) which must not shift the index.
    let i = match dict.get(b"Filter") {
        Some(Object::Name(n)) if n.as_slice() == b"Crypt" => 0,
        Some(Object::Array(a)) => a.iter().position(|n| n.as_name() == Some(b"Crypt"))?,
        _ => return None,
    };
    let parms = match dict.get(b"DecodeParms") {
        Some(Object::Dict(d)) if i == 0 => Some(d.clone()),
        Some(Object::Array(a)) => a.get(i).and_then(|p| p.as_dict().cloned()),
        _ => None,
    };
    Some(parms.and_then(|p| p.name(b"Name").map(|n| n.to_vec())).unwrap_or_else(|| b"Identity".to_vec()))
}

/// A signature dictionary (field signature, document timestamp or usage-rights signature),
/// whose `/Contents` is never encrypted (ISO 32000-2 §7.6.2, §12.8.1): the signer writes a
/// placeholder, hashes the file around it and patches the value in place, so encrypting it
/// would make signing impossible. Identified as other readers do: by `/Type`, or by a string
/// `/Contents` alongside an array `/ByteRange` (`/Type` is optional). A signature dictionary
/// has no `/Subtype`; a dictionary with one (an annotation, whose `/Contents` is the comment
/// text) or with another `/Type` is never exempt, whatever else it carries.
fn is_signature_dict(d: &Dict) -> bool {
    if d.get(b"Subtype").is_some() {
        return false;
    }
    match d.get(b"Type") {
        Some(_) => matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp")),
        None => matches!(d.get(b"Contents"), Some(Object::String(_))) && matches!(d.get(b"ByteRange"), Some(Object::Array(_))),
    }
}

/// Remove the `/Crypt` entry from a stream's filter chain, with its decode parameters.
fn drop_crypt_filter(dict: &mut Dict) {
    let i = match dict.get(b"Filter") {
        Some(Object::Array(a)) => a.iter().position(|n| n.as_name() == Some(b"Crypt")),
        Some(Object::Name(n)) if n.as_slice() == b"Crypt" => Some(0),
        _ => None,
    };
    let Some(i) = i else { return };
    let mut filters = match dict.get(b"Filter") {
        Some(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    if i < filters.len() {
        filters.remove(i);
    }
    // A lone parameter dictionary belongs to the first filter (as `named_crypt_filter` reads it).
    match dict.get(b"DecodeParms").cloned() {
        Some(Object::Array(mut parms)) if i < parms.len() => {
            parms.remove(i);
            dict.set(b"DecodeParms".to_vec(), Object::Array(parms));
        }
        Some(Object::Dict(_)) if i == 0 => {
            dict.remove(b"DecodeParms");
        }
        _ => {}
    }
    if filters.is_empty() {
        dict.remove(b"Filter");
        if matches!(dict.get(b"DecodeParms"), Some(Object::Array(p)) if p.is_empty()) {
            dict.remove(b"DecodeParms");
        }
    } else {
        dict.set(b"Filter".to_vec(), Object::Array(filters));
    }
}

/// What [`transform`] does to an object.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pass {
    /// Decrypt, with the handler the document was opened with.
    Decrypt,
    /// Encrypt with the handler the document was opened with: streams keep their crypt filters.
    Encrypt,
    /// Encrypt with protection applied by `set_encryption`. A stream's crypt filter came from
    /// the source's `/Encrypt`; unless the new one defines that name it is dropped, so the
    /// stream follows the new policy instead of reading as `/Identity` and staying in clear.
    EncryptReplacing,
}

/// Decrypt or encrypt every string and stream in an indirect object, except a signature
/// dictionary's `/Contents` (see [`is_signature_dict`]).
pub(crate) fn transform(h: &SecurityHandler, o: &Object, num: u32, generation: u16, pass: Pass) -> Object {
    let decrypt = pass == Pass::Decrypt;
    let strings = |s: &PdfString| {
        let bytes = if decrypt { h.decrypt_string(num, generation, &s.bytes) } else { h.encrypt_string(num, generation, &s.bytes) };
        PdfString { bytes, hex: s.hex || !decrypt }
    };
    walk(o, &strings, h, num, generation, pass)
}

fn walk(o: &Object, strings: &dyn Fn(&PdfString) -> PdfString, h: &SecurityHandler, num: u32, generation: u16, pass: Pass) -> Object {
    let decrypt = pass == Pass::Decrypt;
    match o {
        Object::String(s) => Object::String(strings(s)),
        Object::Array(a) => Object::Array(a.iter().map(|x| walk(x, strings, h, num, generation, pass)).collect()),
        Object::Dict(d) if is_signature_dict(d) => Object::Dict(
            d.iter()
                .map(|(k, v)| match (k.as_slice(), v) {
                    (b"Contents", Object::String(_)) => (k.clone(), v.clone()),
                    _ => (k.clone(), walk(v, strings, h, num, generation, pass)),
                })
                .collect(),
        ),
        Object::Dict(d) => Object::Dict(d.iter().map(|(k, v)| (k.clone(), walk(v, strings, h, num, generation, pass))).collect()),
        Object::Stream(s) => {
            let mut dict: Dict = s.dict.iter().map(|(k, v)| (k.clone(), walk(v, strings, h, num, generation, pass))).collect();
            let ty = s.dict.name(b"Type");
            // Cross-reference streams are never encrypted (§7.5.8), but only reading may trust the
            // label: a stream a producer calls /XRef stays plaintext if writing believes it, and the
            // writer builds its own cross-reference streams outside this transform.
            if (decrypt && ty == Some(b"XRef")) || (ty == Some(b"Metadata") && !h.encrypts_metadata()) {
                return Object::Stream(Stream { dict, raw: s.raw.clone() });
            }
            let mut named = named_crypt_filter(&s.dict);
            if pass == Pass::EncryptReplacing && named.as_deref().is_some_and(|n| !h.dict().crypt_filters.iter().any(|(f, _)| f == n)) {
                drop_crypt_filter(&mut dict);
                named = None;
            }
            let kind = match (&named, ty) {
                (Some(n), _) => StreamKind::Named(n),
                (None, Some(b"EmbeddedFile")) => StreamKind::EmbeddedFile,
                _ => StreamKind::Normal,
            };
            let raw = if decrypt { h.decrypt_stream(num, generation, &s.raw, kind) } else { h.encrypt_stream(num, generation, &s.raw, kind) };
            Object::Stream(Stream { dict, raw: raw.into() })
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parms(name: &str) -> Object {
        let mut p = Dict::new();
        p.set(b"Name".to_vec(), Object::name(name));
        Object::Dict(p)
    }

    /// `/DecodeParms` lines up with `/Filter` entry by entry: an entry that is not a name (one
    /// this reader cannot follow, say) must not shift the crypt filter's place, or the stream is
    /// read under another filter's name — or as `Identity`, leaving it in clear.
    #[test]
    fn the_named_crypt_filter_reads_the_decode_parms_of_its_own_place() {
        let mut one = Dict::new();
        one.set(b"Filter".to_vec(), Object::name("Crypt"));
        one.set(b"DecodeParms".to_vec(), parms("MyCF"));
        assert_eq!(named_crypt_filter(&one), Some(b"MyCF".to_vec()));
        // A mixed chain: `/Crypt` is the second entry, and so is its parameter dictionary.
        let mut mixed = Dict::new();
        mixed.set(b"Filter".to_vec(), Object::Array(vec![Object::Null, Object::name("Crypt")]));
        mixed.set(b"DecodeParms".to_vec(), Object::Array(vec![parms("First"), parms("Second")]));
        assert_eq!(named_crypt_filter(&mixed), Some(b"Second".to_vec()), "the raw entries' index, not the names'");
        // Dropping the selector agrees with reading it: the same place comes out of both arrays.
        let mut dropped = mixed.clone();
        drop_crypt_filter(&mut dropped);
        let filters = dropped.get(b"Filter").and_then(Object::as_array).cloned().unwrap_or_default();
        let left = dropped.get(b"DecodeParms").and_then(Object::as_array).cloned().unwrap_or_default();
        assert_eq!((filters.len(), left.len()), (1, 1), "{dropped:?}");
        assert_eq!(left[0].as_dict().and_then(|p| p.name(b"Name")), Some(b"First".as_slice()));
        // `/Crypt` with no parameters names `Identity`; a chain without `/Crypt` names nothing.
        let mut bare = Dict::new();
        bare.set(b"Filter".to_vec(), Object::Array(vec![Object::name("Crypt")]));
        assert_eq!(named_crypt_filter(&bare), Some(b"Identity".to_vec()));
        let mut none = Dict::new();
        none.set(b"Filter".to_vec(), Object::Array(vec![Object::name("FlateDecode")]));
        assert_eq!(named_crypt_filter(&none), None);
        assert_eq!(named_crypt_filter(&Dict::new()), None);
    }
}
