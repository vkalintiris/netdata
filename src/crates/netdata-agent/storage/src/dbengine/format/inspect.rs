//! `dbengine-inspect`: what a tier directory holds, read with this crate's readers. The report keeps the keys and
//! rules of the checkers the C files were verified with (`ndinspect.py` `tier_report`, `totals.py`, `buildv2.py` in
//! the fixtures' `tools/`); those rules are the tools', not C's, and agree with C on valid files. Brief
//! `knowledge/brief-dbengine-s0.md` §5 in the status repository.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::crc::crc32;
use super::descriptor::{
    DESCRIPTOR_SIZE, PAGE_TYPE_ARRAY_32BIT, PAGE_TYPE_ARRAY_TIER1, PAGE_TYPE_GORILLA_32BIT,
    PageDescriptor,
};
use super::extent::{self, Extent, PageSlot};
use super::journal_v1::{self, Event};
use super::journal_v2::{
    self, HEADER_SIZE, Header, PAGE_HEADER_SIZE, PAGE_SIZE, PageEntry, PageHeader, Retention,
};
use super::page::{self, gorilla, tier1};
use super::{BLOCK_SIZE, FileKind, ReadAt, file_name, parse_file_name, tier_dir_name};
use crate::storage_number::SN_EMPTY_SLOT;

/// A data file of a tier directory and its journals.
#[derive(Debug, Clone)]
pub struct TierFile {
    pub fileno: u32,
    pub ndf: PathBuf,
    pub njf: PathBuf,
    pub njfv2: PathBuf,
}

/// The `datafile-1-*.ndf` of a tier directory in file-number order; none when the directory does not exist.
pub fn tier_files(dir: &Path) -> io::Result<Vec<TierFile>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in entries {
        let name = e?.file_name();
        let Some((1, fileno)) = name
            .to_str()
            .and_then(|n| parse_file_name(FileKind::Datafile, n))
        else {
            continue;
        };
        out.push(TierFile {
            fileno,
            ndf: dir.join(file_name(FileKind::Datafile, 1, fileno)),
            njf: dir.join(file_name(FileKind::Journal, 1, fileno)),
            njfv2: dir.join(file_name(FileKind::JournalV2, 1, fileno)),
        });
    }
    out.sort_by_key(|f| f.fileno);
    Ok(out)
}

/// The tier directories an argument names: a cache directory's `dbengine*` tiers, or itself (tier from its name).
pub fn tier_dirs(dir: &Path) -> Vec<(usize, PathBuf)> {
    if dir.join(tier_dir_name(0)).is_dir() {
        return (0..5)
            .map(|t| (t, dir.join(tier_dir_name(t))))
            .filter(|(t, d)| *t < 3 || d.is_dir())
            .collect();
    }
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let tier = name
        .strip_prefix("dbengine-tier")
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    vec![(tier, dir.to_path_buf())]
}

/// Python's clamped slice.
fn slice(b: &[u8], from: usize, to: usize) -> &[u8] {
    let to = to.min(b.len());
    &b[from.min(to)..to]
}

fn zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn counts<K: ToString>(c: &BTreeMap<K, u64>) -> Value {
    Value::Object(c.iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A superblock's fields as `ndinspect.py` reports them.
fn superblock(path: &Path, datafile: bool) -> io::Result<Value> {
    let mut sb = Vec::new();
    File::open(path)?
        .take(BLOCK_SIZE as u64)
        .read_to_end(&mut sb)?;
    let field = |from, to| {
        let b = slice(&sb, from, to);
        b.split(|&x| x == 0).next().unwrap_or(b)
    };
    let (magic, version) = (field(0, 32), field(32, 48));
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    Ok(if datafile {
        json!({
            "magic": text(magic),
            "version": text(version),
            "tier": sb.get(48).copied(),
            "rest_zero": zero(slice(&sb, 49, sb.len())),
            "magic_pad_zero": zero(slice(&sb, magic.len(), 32)),
        })
    } else {
        json!({
            "magic": text(magic),
            "version": text(version),
            "rest_zero": zero(slice(&sb, 48, sb.len())),
        })
    })
}

/// A journal transaction where the replay found one, its fields read as the tool reads them.
#[derive(Debug)]
struct Tx {
    id: u64,
    crc_ok: bool,
    reserved: u32,
    payload_length: usize,
    extent_offset: u64,
    extent_size: u32,
    pages: u8,
    descriptors: Vec<PageDescriptor>,
    tail_zero: bool,
}

/// The journal's bytes and its transactions: every record the replay met other than a corrupt one.
fn journal(path: &Path) -> io::Result<Option<(Vec<u8>, Vec<Tx>)>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let replay = journal_v1::replay(&bytes[..], bytes.len() as u64)?;
    let mut txs = Vec::new();
    for e in &replay.events {
        let (pos, id, crc_ok) = match *e {
            Event::Corrupt { .. } => continue,
            Event::CrcFailed { pos, id } => (pos, id, false),
            Event::UnknownType { pos, id, .. }
            | Event::CorruptPayload { pos, id }
            | Event::StoreData { pos, id, .. } => (pos, id, true),
        };
        let pos = pos as usize;
        let payload_length = usize::from(u16::from_le_bytes([bytes[pos + 13], bytes[pos + 14]]));
        let payload = slice(&bytes, pos + 15, pos + 15 + payload_length);
        let pages = payload.get(12).copied().unwrap_or(0);
        let descriptors = (0..usize::from(pages))
            .filter_map(|i| PageDescriptor::decode(payload.get(13 + i * DESCRIPTOR_SIZE..)?))
            .collect();
        let after = pos + 15 + payload_length + 4;
        let block_end = (pos / BLOCK_SIZE + 1) * BLOCK_SIZE;
        txs.push(Tx {
            id,
            crc_ok,
            reserved: u32_at(&bytes, pos + 1).unwrap_or(0),
            payload_length,
            extent_offset: u64_at(payload, 0).unwrap_or(0),
            extent_size: u32_at(payload, 8).unwrap_or(0),
            pages,
            descriptors,
            tail_zero: zero(slice(&bytes, after, block_end)),
        });
    }
    Ok(Some((bytes, txs)))
}

/// The extent a transaction points at and whether its padding to the next block is zero; `None` when it cannot be
/// read.
fn read_extent(
    ndf: &File,
    ndf_size: u64,
    tx: &Tx,
) -> Option<(Result<Extent, extent::HeaderInvalid>, bool)> {
    let mut b = vec![0u8; tx.extent_size as usize];
    ndf.read_exact_at(&mut b, tx.extent_offset).ok()?;
    let end = tx.extent_offset + u64::from(tx.extent_size);
    let pad_to = end.div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64;
    let mut pad = vec![0u8; pad_to.min(ndf_size).saturating_sub(end) as usize];
    ndf.read_exact_at(&mut pad, end).ok()?;
    Some((extent::decode(&b), zero(&pad)))
}

/// `ndinspect.py`'s `tier_report()`: `{dir, files[]}`.
pub fn tier_report(dir: &Path) -> io::Result<Value> {
    let files = tier_files(dir)?
        .iter()
        .map(file_report)
        .collect::<io::Result<Vec<_>>>()?;
    Ok(json!({"dir": dir.display().to_string(), "files": files}))
}

fn file_report(f: &TierFile) -> io::Result<Value> {
    let ndf = File::open(&f.ndf)?;
    let ndf_size = ndf.size()?;
    let mut rep = Map::new();
    rep.insert("fileno".into(), json!(format!("{:010}", f.fileno)));
    rep.insert("ndf_size".into(), json!(ndf_size));
    rep.insert("ndf_sb".into(), superblock(&f.ndf, true)?);
    let (jbytes, txs) = journal(&f.njf)?.unwrap_or_default();
    let njf_sb = if f.njf.exists() {
        superblock(&f.njf, false)?
    } else {
        Value::Null
    };
    rep.insert("njf_sb".into(), njf_sb);
    rep.insert("njf_size".into(), json!(jbytes.len()));
    rep.insert("tx_count".into(), json!(txs.len()));
    let ids: Vec<Value> = txs.iter().map(|t| json!(t.id)).collect();
    let mut tx_ids = ids[..ids.len().min(3)].to_vec();
    tx_ids.push(json!("..."));
    tx_ids.extend_from_slice(&ids[ids.len().saturating_sub(3)..]);
    rep.insert("tx_ids".into(), Value::Array(tx_ids));
    rep.insert("tx_crc_all_ok".into(), json!(txs.iter().all(|t| t.crc_ok)));
    rep.insert(
        "tx_tail_zero".into(),
        json!(txs.iter().all(|t| t.tail_zero)),
    );
    rep.insert(
        "tx_reserved_zero".into(),
        json!(txs.iter().all(|t| t.reserved == 0)),
    );
    let expected = |t: &Tx| t.payload_length == 13 + DESCRIPTOR_SIZE * usize::from(t.pages);
    rep.insert(
        "tx_payload_len_expected".into(),
        json!(txs.iter().all(expected)),
    );

    let (mut crc_ok, mut descr_match, mut size_exact, mut pad_zero) = (true, true, true, true);
    let (mut aligned, mut len_ok, mut contiguous) = (true, true, true);
    let (mut compression, mut types, mut per_extent) =
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut prev_end = BLOCK_SIZE as u64;
    for t in &txs {
        let end = t.extent_offset + u64::from(t.extent_size);
        aligned &= t.extent_offset.is_multiple_of(BLOCK_SIZE as u64);
        contiguous &= t.extent_offset == prev_end;
        prev_end = end.div_ceil(BLOCK_SIZE as u64) * BLOCK_SIZE as u64;
        let Some((e, pad)) = read_extent(&ndf, ndf_size, t) else {
            (crc_ok, descr_match, size_exact) = (false, false, false);
            continue;
        };
        pad_zero &= pad;
        let Ok(e) = e else {
            (crc_ok, descr_match, size_exact) = (false, false, false);
            continue;
        };
        crc_ok &= e.crc_ok;
        descr_match &= e.descriptors == t.descriptors;
        *compression.entry(e.compression).or_insert(0) += 1;
        *per_extent.entry(e.descriptors.len()).or_insert(0) += 1;
        let pages_len: usize = e.descriptors.iter().map(|d| d.page_length as usize).sum();
        len_ok &= e.payload_len() == pages_len;
        for d in &e.descriptors {
            *types.entry(d.page_type).or_insert(0) += 1;
        }
    }
    for (k, v) in [
        ("extent_crc_all_ok", json!(crc_ok)),
        ("extent_descr_eq_journal", json!(descr_match)),
        ("extent_size_exact", json!(size_exact)),
        ("extent_pad_zero", json!(pad_zero)),
        ("extent_offsets_4k", json!(aligned)),
        ("extents_contiguous", json!(contiguous)),
        ("last_extent_end", json!(prev_end)),
        ("ndf_size_eq_last_end", json!(prev_end == ndf_size)),
        ("uncompressed_eq_sum_page_len", json!(len_ok)),
        ("compression", counts(&compression)),
        ("page_types", counts(&types)),
        ("pages_per_extent", counts(&per_extent)),
    ] {
        rep.insert(k.into(), v);
    }
    if f.njfv2.exists() {
        v2_report(&fs::read(&f.njfv2)?, jbytes.len(), &txs, &mut rep);
    }
    Ok(Value::Object(rep))
}

/// A metric of a v2 file as the report walks it: its list entry, its page header's fields, its page list.
struct V2Metric {
    uuid: [u8; 16],
    entries: u32,
    page_offset: u32,
    delta_start_s: u32,
    delta_end_s: u32,
    update_every_s: u32,
    hdr_uuid_match: bool,
    uuid_offset: u32,
    pages: Vec<PageEntry>,
}

/// `ndinspect.py`'s `journal_v2()` and its cross-checks against the v1 journal.
fn v2_report(data: &[u8], njf_size: usize, txs: &[Tx], rep: &mut Map<String, Value>) {
    let Some(hb) = data
        .get(..HEADER_SIZE)
        .and_then(|b| <&[u8; HEADER_SIZE]>::try_from(b).ok())
    else {
        rep.insert("v2_header".into(), Value::Null);
        return;
    };
    let h = Header::decode(hb);
    rep.insert(
        "v2_header".into(),
        json!({
            "magic": format!("{:#x}", h.magic),
            "start_time_ut": h.start_time_ut,
            "end_time_ut": h.end_time_ut,
            "extent_count": h.extent_count,
            "extent_offset": h.extent_offset,
            "metric_count": h.metric_count,
            "metric_offset": h.metric_offset,
            "page_count": h.page_count,
            "page_offset": h.page_offset,
            "extent_trailer_offset": h.extent_trailer_offset,
            "metric_trailer_offset": h.metric_trailer_offset,
            "journal_v1_file_size": h.journal_v1_file_size,
            "journal_v2_file_size": h.journal_v2_file_size,
            "data_ptr": u64_at(data, 64),
            "file_size": data.len(),
        }),
    );
    let len = data.len();
    let crc_ok =
        |from: usize, to: usize, at: usize| u32_at(data, at) == Some(crc32(slice(data, from, to)));
    let (eo, eto) = (h.extent_offset as usize, h.extent_trailer_offset as usize);
    let (mo, mto) = (h.metric_offset as usize, h.metric_trailer_offset as usize);
    let po = h.page_offset as usize;
    let exts: Vec<_> =
        journal_v2::extents(slice(data, eo, eo + h.extent_count as usize * 16)).collect();
    let entries: Vec<_> =
        journal_v2::metrics(slice(data, mo, mo + h.metric_count as usize * 36)).collect();
    let (mut page_header_ok, mut page_list_ok, mut sorted, mut total_pages) =
        (true, true, true, 0u64);
    let mut last_end = po;
    let mut metrics = Vec::new();
    for m in &entries {
        let at = m.page_offset as usize;
        let ph = match data.get(at..at + PAGE_HEADER_SIZE) {
            Some(b) => PageHeader::decode(b.try_into().unwrap_or(&[0; PAGE_HEADER_SIZE])),
            None => PageHeader::default(),
        };
        page_header_ok &= ph.crc == ph.compute_crc() && data.len() >= at + PAGE_HEADER_SIZE;
        let list_at = at + PAGE_HEADER_SIZE;
        let list_end = list_at + ph.entries as usize * PAGE_SIZE;
        let pages: Vec<_> = journal_v2::pages(slice(data, list_at, list_end)).collect();
        page_list_ok &= crc_ok(list_at, list_end, list_end);
        sorted &= pages
            .windows(2)
            .all(|w| w[0].delta_start_s <= w[1].delta_start_s);
        total_pages += u64::from(ph.entries);
        last_end = last_end.max(list_end + 4);
        metrics.push(V2Metric {
            uuid: m.uuid,
            entries: m.entries,
            page_offset: m.page_offset,
            delta_start_s: m.delta_start_s,
            delta_end_s: m.delta_end_s,
            update_every_s: m.update_every_s,
            hdr_uuid_match: ph.uuid == m.uuid,
            uuid_offset: ph.uuid_offset,
            pages,
        });
    }
    let all_pages = || metrics.iter().flat_map(|m| &m.pages);
    rep.insert(
        "v2_checks".into(),
        json!({
            "pad_4096_zero": zero(slice(data, HEADER_SIZE, BLOCK_SIZE)),
            "header_crc_ok": crc_ok(0, HEADER_SIZE, len.wrapping_sub(4)),
            "extent_trailer_offset_expected": eto == eo + h.extent_count as usize * 16,
            "extent_crc_ok": crc_ok(eo, eto, eto),
            "metric_offset_after_extent_trailer": mo == eto + 4,
            "metric_trailer_offset_expected": mto == mo + h.metric_count as usize * 36,
            "metric_crc_ok": crc_ok(mo, mto, mto),
            "metrics_sorted_memcmp": entries.windows(2).all(|w| w[0].uuid < w[1].uuid),
            "page_offset_after_metric_trailer": po == mto + 4,
            "page_header_crc_ok": page_header_ok,
            "page_list_crc_ok": page_list_ok,
            "page_lists_sorted_by_start": sorted,
            "page_count_matches": total_pages == u64::from(h.page_count),
            "v2_size_field_eq_file_size": h.journal_v2_file_size as usize == len,
            "slack_after_pages": len as i64 - 4 - last_end as i64,
            "slack_all_zero": zero(slice(data, last_end, len.saturating_sub(4))),
            "page_length_type_zero": all_pages().all(|p| p.page_length == 0 && p.page_type == 0),
            "extent_file_index_zero": exts.iter().all(|e| e.file_index == 0),
        }),
    );

    let v2_ext: Vec<_> = exts
        .iter()
        .map(|e| (e.datafile_offset, e.datafile_size, e.pages))
        .collect();
    let mut v1_ext: Vec<_> = txs
        .iter()
        .map(|t| (t.extent_offset, t.extent_size, t.pages))
        .collect();
    rep.insert("v2_extents_eq_v1_order".into(), json!(v2_ext == v1_ext));
    let mut v2_sorted = v2_ext;
    v2_sorted.sort_unstable();
    v1_ext.sort_unstable();
    rep.insert("v2_extents_eq_v1_sorted".into(), json!(v2_sorted == v1_ext));
    rep.insert(
        "v2_v1size_eq_njf".into(),
        json!(h.journal_v1_file_size as usize == njf_size),
    );
    // every v1 descriptor as (start, end, transaction index) against the header's start, without validation
    let start_s = (h.start_time_ut / 1_000_000) as i64;
    let mut v1_pages: HashMap<[u8; 16], Vec<(i64, i64, i64)>> = HashMap::new();
    for (ti, t) in txs.iter().enumerate() {
        for d in &t.descriptors {
            let st = (d.start_time_ut / 1_000_000) as i64;
            let et = if d.page_type == PAGE_TYPE_GORILLA_32BIT {
                st + i64::from(d.gorilla_delta_s())
            } else {
                (d.end_time_ut() / 1_000_000) as i64
            };
            v1_pages
                .entry(d.uuid)
                .or_default()
                .push((st - start_s, et - start_s, ti as i64));
        }
    }
    let (mut mismatch, mut ues) = (0u64, BTreeMap::new());
    for m in &metrics {
        let mut a: Vec<_> = m
            .pages
            .iter()
            .map(|p| {
                (
                    i64::from(p.delta_start_s),
                    i64::from(p.delta_end_s),
                    i64::from(p.extent_index),
                )
            })
            .collect();
        a.sort_unstable();
        let mut b = v1_pages.get(&m.uuid).cloned().unwrap_or_default();
        b.sort_unstable();
        mismatch += u64::from(a != b);
        for p in &m.pages {
            *ues.entry(p.update_every_s).or_insert(0) += 1;
        }
    }
    rep.insert(
        "v2_pages_eq_v1_descriptors(start,end,extent_index)".into(),
        json!(mismatch == 0),
    );
    rep.insert("v2_metrics_mismatch".into(), json!(mismatch));
    rep.insert("v2_page_update_every_values".into(), counts(&ues));
    let (first, first_pages) = match metrics.first() {
        Some(m) => (
            json!({
                "uuid": hex(&m.uuid),
                "entries": m.entries,
                "page_offset": m.page_offset,
                "delta_start_s": m.delta_start_s,
                "delta_end_s": m.delta_end_s,
                "update_every_s": m.update_every_s,
                "hdr_uuid_match": m.hdr_uuid_match,
                "uuid_offset": m.uuid_offset,
            }),
            m.pages
                .iter()
                .take(3)
                .map(|p| {
                    json!([
                        p.delta_start_s,
                        p.delta_end_s,
                        p.extent_index,
                        p.update_every_s,
                        p.page_length,
                        p.page_type
                    ])
                })
                .collect(),
        ),
        None => (Value::Null, Vec::new()),
    };
    rep.insert("v2_first_metric".into(), first);
    rep.insert(
        "v2_first_metric_first_pages".into(),
        Value::Array(first_pages),
    );
}

/// The checks of a file report that failed: every `false`, except `extent_pad_zero` (C leaves the padding as it
/// finds it), and a non-zero `v2_metrics_mismatch`.
pub fn failed_checks(file_report: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        let Value::Object(m) = v else {
            return;
        };
        for (k, v) in m {
            match v {
                Value::Bool(false) if k != "extent_pad_zero" => out.push(k.clone()),
                Value::Object(_) => walk(v, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(file_report, &mut out);
    if file_report["v2_metrics_mismatch"]
        .as_u64()
        .is_some_and(|n| n != 0)
    {
        out.push("v2_metrics_mismatch".into());
    }
    out
}

/// Every page of a tier in journal order, with its bytes as its extent holds them (empty where it holds none).
pub fn for_each_page(dir: &Path, mut f: impl FnMut(&PageDescriptor, &[u8])) -> io::Result<()> {
    for tf in tier_files(dir)? {
        let Some((_, txs)) = journal(&tf.njf)? else {
            continue;
        };
        let ndf = File::open(&tf.ndf)?;
        let ndf_size = ndf.size()?;
        for t in &txs {
            let Some((Ok(e), _)) = read_extent(&ndf, ndf_size, t) else {
                continue;
            };
            for (i, d) in e.descriptors.iter().enumerate() {
                match e.page(i) {
                    PageSlot::Page(bytes) => f(d, bytes),
                    PageSlot::Skipped | PageSlot::Empty => f(d, &[]),
                }
            }
        }
    }
    Ok(())
}

/// The storage numbers of a gorilla page with its buffer counts, `None` for an invalid chain.
fn gorilla_values(bytes: &[u8]) -> Option<(Vec<u32>, usize, usize)> {
    let dp = gorilla::from_disk(bytes)?;
    let mut r = gorilla::Reader::new(&dp.buffers);
    let values = std::iter::from_fn(|| r.read()).collect();
    let trailing = bytes.len() / gorilla::BUFFER_SIZE - dp.buffers.len();
    Some((values, dp.buffers.len(), trailing))
}

/// `totals.py`'s counters of one tier; like its `Counter`, a key exists once counted, even by 0.
pub fn tier_totals(dir: &Path) -> io::Result<Value> {
    let mut c: BTreeMap<&str, u64> = BTreeMap::new();
    let mut metrics = HashSet::new();
    for_each_page(dir, |d, bytes| {
        let mut add = |k, n: usize| *c.entry(k).or_insert(0) += n as u64;
        metrics.insert(d.uuid);
        add("pages", 1);
        add("page_bytes", d.page_length as usize);
        let empty = |v: &[u32]| v.iter().filter(|&&x| x == SN_EMPTY_SLOT).count();
        match d.page_type {
            PAGE_TYPE_GORILLA_32BIT => {
                let (values, buffers, trailing) = gorilla_values(bytes).unwrap_or_default();
                add("gorilla_pages", 1);
                add("gorilla_buffers", buffers);
                add("gorilla_trailing_buffers", trailing);
                add("slots", values.len());
                add("empty_slots", empty(&values));
                if values.len() != d.gorilla_entries() as usize {
                    add("gorilla_entries_mismatch", 1);
                }
            }
            PAGE_TYPE_ARRAY_32BIT => {
                let values = page::array32_decode(bytes).unwrap_or_default();
                add("raw_pages", 1);
                add("slots", values.len());
                add("empty_slots", empty(&values));
            }
            PAGE_TYPE_ARRAY_TIER1 => {
                let records = tier1::decode(bytes).unwrap_or_default();
                let count =
                    |f: fn(&tier1::Tier1Record) -> bool| records.iter().filter(|r| f(r)).count();
                add("tier1_pages", 1);
                add("records", records.len());
                add("records_nan_sum", count(|r| r.sum.is_nan()));
                add("records_count0", count(|r| r.count == 0));
                add(
                    "records_nan_count1",
                    count(|r| r.sum.is_nan() && r.count == 1),
                );
                add(
                    "records_count_sum",
                    records.iter().map(|r| usize::from(r.count)).sum(),
                );
                add(
                    "records_anomaly_sum",
                    records.iter().map(|r| usize::from(r.anomaly_count)).sum(),
                );
            }
            _ => add("unknown_pages", 1),
        }
    })?;
    c.insert("metrics", metrics.len() as u64);
    Ok(json!(c))
}

/// `totals.py` over a cache directory: tiers 0 to 2 always, 3 and 4 when present.
pub fn cache_totals(dir: &Path) -> io::Result<Value> {
    let mut out = Map::new();
    for (tier, d) in tier_dirs(dir) {
        out.insert(tier.to_string(), tier_totals(&d)?);
    }
    Ok(Value::Object(out))
}

/// A v2 file rebuilt from its journal and compared with the one on disk (`buildv2.py`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebuild {
    pub njf: PathBuf,
    pub mine: usize,
    pub theirs: usize,
    /// Offsets where the two differ, within the shorter one.
    pub diffs: Vec<usize>,
}

impl Rebuild {
    pub fn identical(&self) -> bool {
        self.mine == self.theirs && self.diffs.is_empty()
    }

    /// `IDENTICAL <njf> <len>` or `DIFF <njf> <mine> <theirs> first diffs at [..] count N`.
    pub fn line(&self) -> String {
        let njf = self.njf.display();
        if self.identical() {
            return format!("IDENTICAL {njf} {}", self.mine);
        }
        let first: Vec<_> = self.diffs.iter().take(8).map(|d| d.to_string()).collect();
        format!(
            "DIFF {njf} {} {} first diffs at [{}] count {}",
            self.mine,
            self.theirs,
            first.join(", "),
            self.diffs.len()
        )
    }
}

/// The rebuild of every v2 file of a tier: the startup rebuild with no known metrics and no future check.
pub fn rebuild_v2(dir: &Path) -> io::Result<Vec<Rebuild>> {
    let mut out = Vec::new();
    for tf in tier_files(dir)? {
        if !tf.njfv2.exists() {
            continue;
        }
        let j = fs::read(&tf.njf)?;
        let replay = journal_v1::replay(&j[..], j.len() as u64)?;
        let mine = journal_v2::from_v1(&replay, j.len() as u64, 0, &mut Retention::default())
            .unwrap_or_default();
        let theirs = fs::read(&tf.njfv2)?;
        let diffs = mine
            .iter()
            .zip(&theirs)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        out.push(Rebuild {
            njf: tf.njf,
            mine: mine.len(),
            theirs: theirs.len(),
            diffs,
        });
    }
    Ok(out)
}

/// The summary line of a file report.
pub fn file_line(rep: &Value, rebuild: Option<&Rebuild>) -> String {
    let n = |k: &str| rep[k].as_u64().unwrap_or(0);
    let ok = |k: &str| {
        if rep[k].as_bool() == Some(true) {
            "ok"
        } else {
            "FAILED"
        }
    };
    let names = |k: &str, name: fn(&str) -> String| {
        let Some(m) = rep[k].as_object() else {
            return String::new();
        };
        m.iter()
            .map(|(k, v)| format!("{} {v}", name(k)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let compression = |k: &str| {
        match k {
            "0" => "none",
            "1" => "lz4",
            "2" => "zstd",
            other => other,
        }
        .to_string()
    };
    let per_extent = names("pages_per_extent", |k| format!("{k} pages x"));
    let mut line = format!(
        "{} ndf {} njf {} tx {} crc {} | extents {}, {}, crc {}",
        rep["fileno"].as_str().unwrap_or(""),
        n("ndf_size"),
        n("njf_size"),
        n("tx_count"),
        ok("tx_crc_all_ok"),
        names("compression", compression),
        per_extent.replace("x ", "x"),
        ok("extent_crc_all_ok"),
    );
    if let Some(h) = rep.get("v2_header").filter(|h| h.is_object()) {
        let v2_failed = rep.get("v2_checks").map(failed_checks).unwrap_or_default();
        line += &format!(
            " | v2 E{} M{} P{} {} B {}",
            h["extent_count"],
            h["metric_count"],
            h["page_count"],
            h["file_size"],
            if v2_failed.is_empty() { "ok" } else { "FAILED" }
        );
        if let Some(r) = rebuild {
            line += if r.identical() {
                ", rebuild identical"
            } else {
                ", rebuild DIFFERS"
            };
        }
    }
    let failed = failed_checks(rep);
    if !failed.is_empty() {
        line += &format!(" | failed: {}", failed.join(", "));
    }
    line
}

/// The summary line of a tier's totals.
pub fn tier_line(tier: usize, totals: &Value) -> String {
    let n = |k: &str| totals[k].as_u64();
    let mut parts = Vec::new();
    if let Some(slots) = n("slots") {
        parts.push(format!(
            "{slots} slots ({} empty)",
            n("empty_slots").unwrap_or(0)
        ));
    }
    if let Some(buffers) = n("gorilla_buffers") {
        parts.push(format!("{buffers} gorilla buffers"));
    }
    if let Some(records) = n("records") {
        parts.push(format!("{records} records"));
    }
    if parts.is_empty() {
        parts.push("no pages".into());
    }
    format!("tier {tier} points: {}", parts.join(", "))
}

/// A metric's pages, one line per page and one per point (debug output).
pub fn dump(dir: &Path, uuid: &[u8; 16], out: &mut Vec<String>) -> io::Result<()> {
    for_each_page(dir, |d, bytes| {
        if &d.uuid != uuid {
            return;
        }
        out.push(format!(
            "page type {} start_ut {} length {} tail {}",
            d.page_type,
            d.start_time_ut,
            d.page_length,
            hex(&d.tail)
        ));
        let values = match d.page_type {
            PAGE_TYPE_GORILLA_32BIT => gorilla_values(bytes).map(|v| v.0),
            PAGE_TYPE_ARRAY_32BIT => page::array32_decode(bytes),
            _ => None,
        };
        if let Some(values) = values {
            out.extend(
                values
                    .iter()
                    .map(|&v| format!("  {v:#010x} {}", crate::storage_number::unpack(v))),
            );
        } else if d.page_type == PAGE_TYPE_ARRAY_TIER1 {
            let records = tier1::decode(bytes).unwrap_or_default();
            out.extend(records.iter().map(|r| {
                format!(
                    "  sum {} min {} max {} count {} anomalies {}",
                    r.sum, r.min, r.max, r.count, r.anomaly_count
                )
            }));
        }
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::super::descriptor::PageDescriptor;
    use super::super::journal_v1::StoreData;
    use super::super::superblock;
    use super::*;
    use crate::storage_number::{SN_DEFAULT_FLAGS, pack};

    /// Writes a one-file tier with the encoders: `extents` of raw pages, each after the last, their journal, and
    /// the v2 file C would build.
    fn tier(dir: &Path, extents: &[Vec<(u8, u64, Vec<u32>)>]) {
        let mut ndf = superblock::encode_datafile().to_vec();
        let mut njf = superblock::encode_journal().to_vec();
        for (id, pages) in (1u64..).zip(extents) {
            let pages: Vec<_> = pages
                .iter()
                .map(|(uuid, start, values)| {
                    let end = start + values.len() as u64 - 1;
                    let d = PageDescriptor::array(
                        PAGE_TYPE_ARRAY_32BIT,
                        [*uuid; 16],
                        values.len() as u32 * 4,
                        start * 1_000_000,
                        end * 1_000_000,
                    );
                    (d, page::array32_encode(values))
                })
                .collect();
            let refs: Vec<_> = pages.iter().map(|(d, b)| (*d, &b[..])).collect();
            let e = extent::encode(&refs, extent::COMPRESSION_ZSTD);
            let data = StoreData {
                extent_offset: ndf.len() as u64,
                extent_size: e.size_bytes as u32,
                descriptors: pages.iter().map(|(d, _)| *d).collect(),
            };
            ndf.extend_from_slice(&e.bytes);
            njf.extend_from_slice(&journal_v1::encode_transaction(id, &data));
        }
        let name = |kind| dir.join(file_name(kind, 1, 1));
        fs::write(name(FileKind::Datafile), &ndf).unwrap();
        fs::write(name(FileKind::Journal), &njf).unwrap();
        let replay = journal_v1::replay(&njf[..], njf.len() as u64).unwrap();
        let v2 =
            journal_v2::from_v1(&replay, njf.len() as u64, 0, &mut Retention::default()).unwrap();
        fs::File::create(name(FileKind::JournalV2))
            .unwrap()
            .write_all(&v2)
            .unwrap();
    }

    fn values(n: u32, empty_at: u32) -> Vec<u32> {
        (0..n)
            .map(|i| {
                if i == empty_at {
                    SN_EMPTY_SLOT
                } else {
                    pack(f64::from(i), SN_DEFAULT_FLAGS)
                }
            })
            .collect()
    }

    #[test]
    fn an_encoded_tier_passes_every_check() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join(tier_dir_name(0));
        fs::create_dir(&dir).unwrap();
        tier(
            &dir,
            &[
                vec![(2, 1000, values(10, 3)), (1, 1000, values(5, 9))],
                vec![(1, 1005, values(5, 9))],
            ],
        );
        let report = tier_report(&dir).unwrap();
        let file = &report["files"][0];
        assert_eq!(failed_checks(file), Vec::<String>::new(), "{file:#}");
        assert_eq!(file["tx_ids"], json!([1, 2, "...", 1, 2]));
        assert_eq!(
            file["compression"],
            json!({"0": 2}),
            "pages this small are stored uncompressed"
        );
        assert_eq!(file["pages_per_extent"], json!({"1": 1, "2": 1}));
        assert_eq!(file["v2_header"]["magic"], json!("0x1230317"));
        assert_eq!(file["v2_first_metric"]["uuid"], json!(hex(&[1; 16])));
        assert_eq!(
            file["v2_first_metric_first_pages"],
            json!([[0, 4, 0, 1, 0, 0], [5, 9, 1, 1, 0, 0]])
        );
        assert_eq!(file["v2_page_update_every_values"], json!({"1": 3}));
        let rebuilds = rebuild_v2(&dir).unwrap();
        assert!(rebuilds[0].identical(), "{}", rebuilds[0].line());
        let line = file_line(file, rebuilds.first());
        assert!(line.ends_with("B ok, rebuild identical"), "{line}");

        let totals = cache_totals(cache.path()).unwrap();
        assert_eq!(
            totals["0"],
            json!({"empty_slots": 1, "metrics": 2, "page_bytes": 80, "pages": 3, "raw_pages": 3, "slots": 20})
        );
        assert_eq!(totals["1"], json!({"metrics": 0}));
        assert_eq!(
            tier_line(0, &totals["0"]),
            "tier 0 points: 20 slots (1 empty)"
        );
        let mut out = Vec::new();
        dump(&dir, &[2; 16], &mut out).unwrap();
        assert_eq!(out.len(), 11);
    }

    #[test]
    fn broken_files_fail_their_checks() {
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path();
        tier(dir, &[vec![(1, 1000, values(5, 9))]]);
        let v2 = dir.join(file_name(FileKind::JournalV2, 1, 1));
        let mut b = fs::read(&v2).unwrap();
        // the pad byte of the only page list entry: covered by its CRC, compared by nothing else
        let at = b.len() - 9;
        b[at] ^= 1;
        fs::write(&v2, &b).unwrap();
        let file = &tier_report(dir).unwrap()["files"][0];
        assert_eq!(failed_checks(file), ["page_list_crc_ok"]);
        let r = &rebuild_v2(dir).unwrap()[0];
        assert!(
            r.line().starts_with("DIFF ") && r.line().ends_with(&format!("[{at}] count 1")),
            "{}",
            r.line()
        );
    }

    #[test]
    fn tier_dirs_of_a_cache_and_of_a_tier() {
        let cache = tempfile::tempdir().unwrap();
        for t in [0, 1, 3] {
            fs::create_dir(cache.path().join(tier_dir_name(t))).unwrap();
        }
        let tiers: Vec<_> = tier_dirs(cache.path())
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(tiers, [0, 1, 2, 3]);
        assert_eq!(tier_dirs(&cache.path().join("dbengine-tier3"))[0].0, 3);
    }
}
